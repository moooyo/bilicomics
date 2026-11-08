use std::{
    collections::HashSet,
    fs::{self, OpenOptions},
    io::{Cursor, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime},
};

use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};

static NEXT_TEMPORARY: AtomicU64 = AtomicU64::new(0);

/// A bounded, single-application-process cache. Payloads use digest-only filenames.
pub struct FileCache {
    root: PathBuf,
    maximum_bytes: u64,
    connection: Connection,
}

#[derive(Clone, Debug)]
struct Entry {
    key: String,
    filename: String,
    size: u64,
    pinned: bool,
}

impl FileCache {
    pub fn open(root: impl AsRef<Path>, maximum_bytes: u64) -> Result<Self> {
        ensure!(
            maximum_bytes > 0 && maximum_bytes <= i64::MAX as u64,
            "invalid cache budget"
        );
        fs::create_dir_all(root.as_ref()).context("create file cache directory")?;
        let root = root
            .as_ref()
            .canonicalize()
            .context("resolve file cache directory")?;
        let index = root.join("cache-index.sqlite3");
        if let Ok(metadata) = fs::symlink_metadata(&index) {
            ensure!(
                !metadata.file_type().is_symlink(),
                "cache index must not be a symlink"
            );
        }
        let connection = Connection::open(&index).context("open cache index")?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS cache_entries (
                key TEXT PRIMARY KEY NOT NULL,
                filename TEXT NOT NULL,
                size INTEGER NOT NULL CHECK(size >= 0),
                last_access INTEGER NOT NULL,
                pinned INTEGER NOT NULL DEFAULT 0
            );
            CREATE INDEX IF NOT EXISTS cache_lru ON cache_entries(pinned,last_access);
        ",
        )?;
        #[cfg(unix)]
        {
            set_private_permissions(&root, 0o700)?;
            set_private_permissions(&index, 0o600)?;
        }
        let cache = Self {
            root,
            maximum_bytes,
            connection,
        };
        cache.recover()?;
        cache.evict_to_budget()?;
        Ok(cache)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn maximum_bytes(&self) -> u64 {
        self.maximum_bytes
    }

    pub fn usage(&self) -> Result<u64> {
        Ok(self.connection.query_row(
            "SELECT COALESCE(SUM(size),0) FROM cache_entries",
            [],
            |row| row.get::<_, i64>(0),
        )? as u64)
    }

    /// Snapshot logical identities for application-owned recovery and reconciliation.
    pub fn keys(&self) -> Result<Vec<String>> {
        Ok(self.entries()?.into_iter().map(|entry| entry.key).collect())
    }

    pub fn pinned_keys(&self) -> Result<Vec<String>> {
        Ok(self
            .entries()?
            .into_iter()
            .filter(|entry| entry.pinned)
            .map(|entry| entry.key)
            .collect())
    }

    pub fn get(&self, key: &str) -> Result<Option<PathBuf>> {
        let Some(entry) = self.entry(key)? else {
            return Ok(None);
        };
        let path = self.object_path(&entry.filename)?;
        match fs::symlink_metadata(&path) {
            Ok(metadata)
                if metadata.is_file()
                    && !metadata.file_type().is_symlink()
                    && metadata.len() == entry.size =>
            {
                if !verify_digest(&path, &entry.filename, entry.size)? {
                    self.connection
                        .execute("DELETE FROM cache_entries WHERE key=?1", [key])?;
                    return Ok(None);
                }
                self.connection.execute("UPDATE cache_entries SET last_access=(SELECT COALESCE(MAX(last_access),0)+1 FROM cache_entries) WHERE key=?1", [key])?;
                Ok(Some(path))
            }
            Ok(_) => {
                self.connection
                    .execute("DELETE FROM cache_entries WHERE key=?1", [key])?;
                Ok(None)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.connection
                    .execute("DELETE FROM cache_entries WHERE key=?1", [key])?;
                Ok(None)
            }
            Err(error) => Err(error).context("inspect cached object"),
        }
    }

    pub fn put(&self, key: &str, bytes: &[u8]) -> Result<PathBuf> {
        self.put_with_extension(key, bytes, "bin")
    }

    /// Store a PNG that has already passed the caller's image-decoding policy.
    pub fn put_png(&self, key: &str, bytes: &[u8]) -> Result<PathBuf> {
        ensure!(
            bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
            "cached PNG has an invalid signature"
        );
        self.put_with_extension(key, bytes, "png")
    }

    /// Decode within explicit limits, resize, and quantize to 16 grayscale values.
    pub fn put_cover(&self, key: &str, bytes: &[u8], width: u32, height: u32) -> Result<PathBuf> {
        ensure!(
            width > 0 && height > 0 && width <= 2048 && height <= 2048,
            "invalid thumbnail geometry"
        );
        ensure!(
            bytes.len() <= 16 * 1024 * 1024,
            "cover download exceeds size limit"
        );
        let mut reader = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(8192);
        limits.max_image_height = Some(8192);
        limits.max_alloc = Some(64 * 1024 * 1024);
        reader.limits(limits);
        let mut grayscale = reader
            .decode()
            .context("decode upstream cover")?
            .thumbnail(width, height)
            .to_luma8();
        for pixel in grayscale.pixels_mut() {
            pixel[0] = (((pixel[0] as u16 + 8) / 17).min(15) * 17) as u8;
        }
        let mut output = Cursor::new(Vec::new());
        image::DynamicImage::ImageLuma8(grayscale)
            .write_to(&mut output, image::ImageFormat::Png)?;
        self.put_with_extension(key, output.get_ref(), "png")
    }

    pub fn set_pinned(&self, key: &str, pinned: bool) -> Result<bool> {
        Ok(self.connection.execute(
            "UPDATE cache_entries SET pinned=?2 WHERE key=?1",
            params![key, pinned],
        )? > 0)
    }

    pub fn remove(&self, key: &str) -> Result<bool> {
        let Some(entry) = self.entry(key)? else {
            return Ok(false);
        };
        ensure!(!entry.pinned, "active cache object is pinned");
        self.connection
            .execute("DELETE FROM cache_entries WHERE key=?1", [key])?;
        self.remove_object(&entry.filename)?;
        Ok(true)
    }

    pub fn clear_unpinned(&self) -> Result<u64> {
        let entries = self.entries()?;
        let mut removed = 0;
        for entry in entries.into_iter().filter(|entry| !entry.pinned) {
            if self.remove(&entry.key)? {
                removed += entry.size;
            }
        }
        Ok(removed)
    }

    fn put_with_extension(&self, key: &str, bytes: &[u8], extension: &str) -> Result<PathBuf> {
        ensure!(!key.is_empty() && !key.contains('\0'), "invalid cache key");
        let size = bytes.len() as u64;
        ensure!(
            size <= self.maximum_bytes,
            "cache object exceeds the storage budget"
        );
        let filename = format!("{}-{}.{}", digest(key.as_bytes()), digest(bytes), extension);
        let destination = self.object_path(&filename)?;
        let old = self.entry(key)?;
        let old_size = old.as_ref().map_or(0, |entry| entry.size);
        let required = self.usage()?.saturating_sub(old_size).saturating_add(size);
        let mut remaining = required;
        let mut victims = Vec::new();
        for entry in self.entries()? {
            if remaining <= self.maximum_bytes {
                break;
            }
            if entry.key == key || entry.pinned {
                continue;
            }
            remaining = remaining.saturating_sub(entry.size);
            victims.push(entry);
        }
        ensure!(
            remaining <= self.maximum_bytes,
            "pinned cache objects leave insufficient storage"
        );

        let temporary = self.root.join(format!(
            ".tmp-{}-{}",
            std::process::id(),
            NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed)
        ));
        let write_result = (|| -> Result<()> {
            let mut output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                output.set_permissions(fs::Permissions::from_mode(0o600))?;
            }
            output.write_all(bytes)?;
            output.sync_all()?;
            drop(output);
            if let Ok(metadata) = fs::symlink_metadata(&destination) {
                ensure!(
                    metadata.is_file() && !metadata.file_type().is_symlink(),
                    "cache destination is not a regular file"
                );
                ensure!(
                    metadata.len() == size,
                    "content-addressed cache object has invalid size"
                );
                ensure!(
                    verify_digest(&destination, &filename, size)?,
                    "content-addressed cache object is corrupt"
                );
                fs::remove_file(&temporary)?;
            } else {
                fs::rename(&temporary, &destination).context("commit cache object")?;
            }
            Ok(())
        })();
        if write_result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        write_result?;

        let transaction = self.connection.unchecked_transaction()?;
        transaction.execute("INSERT INTO cache_entries (key,filename,size,last_access,pinned) VALUES (?1,?2,?3,(SELECT COALESCE(MAX(last_access),0)+1 FROM cache_entries),?4) ON CONFLICT(key) DO UPDATE SET filename=excluded.filename,size=excluded.size,last_access=excluded.last_access", params![key,filename,size as i64,old.as_ref().is_some_and(|entry| entry.pinned)])?;
        for victim in &victims {
            transaction.execute("DELETE FROM cache_entries WHERE key=?1", [&victim.key])?;
        }
        transaction.commit()?;
        for victim in victims {
            self.remove_object(&victim.filename)?;
        }
        if let Some(old) = old.filter(|entry| entry.filename != filename) {
            self.remove_object(&old.filename)?;
        }
        Ok(destination)
    }

    fn entry(&self, key: &str) -> Result<Option<Entry>> {
        Ok(self
            .connection
            .query_row(
                "SELECT * FROM cache_entries WHERE key=?1",
                [key],
                read_entry,
            )
            .optional()?)
    }

    fn entries(&self) -> Result<Vec<Entry>> {
        let mut statement = self
            .connection
            .prepare("SELECT * FROM cache_entries ORDER BY last_access,key")?;
        let rows = statement.query_map([], read_entry)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    fn object_path(&self, filename: &str) -> Result<PathBuf> {
        ensure!(valid_filename(filename), "invalid cache object filename");
        Ok(self.root.join(filename))
    }

    fn remove_object(&self, filename: &str) -> Result<()> {
        let path = self.object_path(filename)?;
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error).context("remove stale cache object"),
        }
    }

    fn evict_to_budget(&self) -> Result<()> {
        let mut usage = self.usage()?;
        for entry in self.entries()? {
            if usage <= self.maximum_bytes {
                break;
            }
            if entry.pinned {
                continue;
            }
            self.remove(&entry.key)?;
            usage = usage.saturating_sub(entry.size);
        }
        ensure!(
            usage <= self.maximum_bytes,
            "pinned cache objects exceed the configured budget"
        );
        Ok(())
    }

    fn recover(&self) -> Result<()> {
        let entries = self.entries()?;
        let mut known = HashSet::new();
        for entry in entries {
            let path = self.object_path(&entry.filename)?;
            let valid = match fs::symlink_metadata(&path) {
                Ok(metadata) => {
                    metadata.is_file()
                        && !metadata.file_type().is_symlink()
                        && metadata.len() == entry.size
                        && verify_digest(&path, &entry.filename, entry.size)?
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
                Err(error) => return Err(error).context("inspect cache during recovery"),
            };
            if valid {
                known.insert(entry.filename);
            } else {
                self.connection
                    .execute("DELETE FROM cache_entries WHERE key=?1", [&entry.key])?;
            }
        }
        for item in fs::read_dir(&self.root)? {
            let item = item?;
            let filename = item.file_name().to_string_lossy().into_owned();
            if valid_filename(&filename) && !known.contains(&filename) {
                self.remove_object(&filename)?;
            } else if filename.starts_with(".tmp-") {
                let stale = item
                    .metadata()?
                    .modified()
                    .ok()
                    .and_then(|modified| SystemTime::now().duration_since(modified).ok())
                    .is_some_and(|age| age >= Duration::from_secs(24 * 60 * 60));
                if stale {
                    fs::remove_file(item.path())?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(unix)]
fn set_private_permissions(path: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    match fs::set_permissions(path, fs::Permissions::from_mode(mode)) {
        Ok(()) => Ok(()),
        Err(error) if permission_mode_unavailable(path, &error) => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(unix)]
fn permission_mode_unavailable(path: &Path, error: &std::io::Error) -> bool {
    #[cfg(target_os = "linux")]
    {
        use std::{ffi::CString, os::unix::ffi::OsStrExt};
        let Ok(path) = CString::new(path.as_os_str().as_bytes()) else {
            return false;
        };
        let mut information = std::mem::MaybeUninit::<libc::statfs>::uninit();
        // SAFETY: statfs writes one aligned structure and retains no pointer.
        if unsafe { libc::statfs(path.as_ptr(), information.as_mut_ptr()) } != 0 {
            return false;
        }
        // SAFETY: a successful statfs call initialized the structure above.
        let magic = unsafe { information.assume_init() }.f_type as u64;
        is_permissionless_fat(magic, error.raw_os_error())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (path, error);
        false
    }
}

#[cfg(target_os = "linux")]
fn is_permissionless_fat(magic: u64, error: Option<i32>) -> bool {
    // FAT and exFAT cannot enforce POSIX modes. Other failures remain errors.
    matches!(magic, 0x4d44 | 0x2011bab0)
        && error.is_some_and(|code| matches!(code, libc::EPERM | libc::EOPNOTSUPP))
}

fn read_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<Entry> {
    Ok(Entry {
        key: row.get("key")?,
        filename: row.get("filename")?,
        size: row.get::<_, i64>("size")? as u64,
        pinned: row.get("pinned")?,
    })
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn verify_digest(path: &Path, filename: &str, size: u64) -> Result<bool> {
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(size.saturating_add(1))
        .read_to_end(&mut bytes)?;
    Ok(bytes.len() as u64 == size && digest(&bytes) == filename[65..129])
}

fn valid_filename(filename: &str) -> bool {
    let Some(stem) = filename
        .strip_suffix(".bin")
        .or_else(|| filename.strip_suffix(".png"))
    else {
        return false;
    };
    stem.len() == 129
        && stem.as_bytes()[64] == b'-'
        && stem.bytes().enumerate().all(|(index, byte)| {
            index == 64 || byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn permission_fallback_requires_fat_and_an_unsupported_mode_error() {
        for magic in [0x4d44, 0x2011bab0] {
            assert!(is_permissionless_fat(magic, Some(libc::EPERM)));
            assert!(is_permissionless_fat(magic, Some(libc::EOPNOTSUPP)));
            assert!(!is_permissionless_fat(magic, Some(libc::EACCES)));
            assert!(!is_permissionless_fat(magic, Some(libc::EIO)));
            assert!(!is_permissionless_fat(magic, None));
        }
        assert!(!is_permissionless_fat(0xef53, Some(libc::EPERM)));
    }

    fn temporary_root() -> PathBuf {
        std::env::temp_dir().join(format!(
            "bilicomics-cache-{}-{}",
            std::process::id(),
            NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn keys_cannot_escape_cache_directory() -> Result<()> {
        let root = temporary_root();
        let cache = FileCache::open(&root, 100)?;
        let path = cache.put("../../outside/private.txt", b"cover")?;
        assert_eq!(path.parent(), Some(cache.root()));
        assert!(valid_filename(path.file_name().unwrap().to_str().unwrap()));
        assert!(!root.parent().unwrap().join("outside/private.txt").exists());
        drop(cache);
        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn lru_eviction_respects_accesses_and_pins() -> Result<()> {
        let root = temporary_root();
        let cache = FileCache::open(&root, 9)?;
        cache.put("first", b"111")?;
        cache.put("second", b"222")?;
        cache.put("third", b"333")?;
        cache.set_pinned("first", true)?;
        cache.get("second")?;
        cache.put("fourth", b"444")?;
        assert!(cache.get("first")?.is_some());
        assert!(cache.get("second")?.is_some());
        assert!(cache.get("third")?.is_none());
        assert_eq!(cache.usage()?, 9);
        drop(cache);
        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn failed_replacement_preserves_previous_entry() -> Result<()> {
        let root = temporary_root();
        let cache = FileCache::open(&root, 6)?;
        cache.put("active", b"111")?;
        cache.set_pinned("active", true)?;
        let old_path = cache.put("replace", b"222")?;
        assert!(cache.put("replace", b"3333").is_err());
        assert_eq!(cache.get("replace")?, Some(old_path.clone()));
        assert_eq!(fs::read(old_path)?, b"222");
        drop(cache);
        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn restart_removes_orphans_and_missing_index_entries() -> Result<()> {
        let root = temporary_root();
        {
            let cache = FileCache::open(&root, 100)?;
            let path = cache.put("missing", b"111")?;
            fs::remove_file(path)?;
            let orphan = format!("{}-{}.bin", digest(b"orphan"), digest(b"body"));
            fs::write(cache.root().join(orphan), b"body")?;
        }
        let cache = FileCache::open(&root, 100)?;
        assert!(cache.get("missing")?.is_none());
        assert_eq!(cache.usage()?, 0);
        let objects = fs::read_dir(cache.root())?
            .filter_map(|entry| entry.ok())
            .filter(|entry| valid_filename(&entry.file_name().to_string_lossy()))
            .count();
        assert_eq!(objects, 0);
        drop(cache);
        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn corrupt_same_size_object_is_not_returned() -> Result<()> {
        let root = temporary_root();
        let cache = FileCache::open(&root, 100)?;
        let path = cache.put("cover", b"good")?;
        fs::write(path, b"evil")?;
        assert!(cache.get("cover")?.is_none());
        assert_eq!(cache.usage()?, 0);
        drop(cache);
        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn thumbnails_fit_reserved_geometry_and_use_only_sixteen_gray_levels() -> Result<()> {
        let root = temporary_root();
        let cache = FileCache::open(&root, 1024 * 1024)?;
        let source = image::RgbImage::from_fn(6, 4, |x, y| {
            image::Rgb([(x * 41) as u8, (y * 63) as u8, 151])
        });
        let mut encoded = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(source).write_to(&mut encoded, image::ImageFormat::Png)?;
        let path = cache.put_cover("thumbnail", encoded.get_ref(), 3, 3)?;
        let result = image::open(path)?.to_luma8();
        assert_eq!(result.dimensions(), (3, 2));
        assert!(result.pixels().all(|pixel| pixel[0] % 17 == 0));
        drop(cache);
        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn cache_does_not_follow_object_symlinks() -> Result<()> {
        let root = temporary_root();
        let external = root.with_extension("external");
        fs::write(&external, b"111")?;
        let cache = FileCache::open(&root, 100)?;
        let path = cache.put("key", b"111")?;
        fs::remove_file(&path)?;
        std::os::unix::fs::symlink(&external, &path)?;
        assert!(cache.get("key")?.is_none());
        assert_eq!(fs::read(&external)?, b"111");
        drop(cache);
        fs::remove_dir_all(root)?;
        fs::remove_file(external)?;
        Ok(())
    }
}
