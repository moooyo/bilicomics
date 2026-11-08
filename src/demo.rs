//! Bundled design fixtures, materialized only inside the caller's demo directory.
//!
//! This module has no source client, account session, or network dependency.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, Metadata, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::domain::{Comic, ComicDetail, Episode, EpisodeAccess, ImageIndex, PageDescriptor};

include!(concat!(env!("OUT_DIR"), "/demo_assets.rs"));

const MAX_ASSET_BYTES: usize = 32 * 1024 * 1024;
const MAX_TOTAL_BYTES: usize = 128 * 1024 * 1024;
static NEXT_TEMPORARY: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct DemoPanel {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Clone, Debug)]
pub struct DemoData {
    pub comics: Vec<Comic>,
    pub details: Vec<ComicDetail>,
    pub indices: Vec<ImageIndex>,
    /// Ordered local paths, keyed by episode identity.
    pub pages: BTreeMap<String, Vec<PathBuf>>,
    /// Local cover paths, keyed by comic identity.
    pub covers: BTreeMap<String, PathBuf>,
    pub account_name: String,
    /// Exact catalog strings and presentation fixtures from the supplied design.
    pub fixture: Value,
    /// Explicit design regions, keyed by stable page descriptor identity.
    pub panels: BTreeMap<String, Vec<DemoPanel>>,
    /// The design's independent vertical strip, outside the paged episode index.
    pub strip: Option<PathBuf>,
    pub strip_panels: Vec<DemoPanel>,
}

struct TemplatePage {
    number: u64,
    filename: String,
    path: PathBuf,
    width: u32,
    height: u32,
    digest: String,
    panels: Vec<DemoPanel>,
}

struct TemplateAssets {
    pages: Vec<TemplatePage>,
    strip: Option<PathBuf>,
    strip_panels: Vec<DemoPanel>,
}

/// The application must pass its isolated demo child, never a real account root.
pub fn load(data_dir: &Path) -> Result<DemoData> {
    let fixture: Value = serde_json::from_slice(embedded("catalog.json")?)
        .context("Read the bundled design catalog")?;
    let manifest: Value = serde_json::from_slice(embedded("manifest.json")?)
        .context("Read the bundled design image manifest")?;
    ensure!(fixture["demo_only"] == true && manifest["demo_only"] == true,
        "Only explicitly isolated demo assets can be loaded");
    let assets = materialize(data_dir)?;
    let TemplateAssets { pages: templates, strip, strip_panels } = load_templates(&manifest, &assets)?;
    ensure!(!templates.is_empty(), "The demo needs at least one bundled page image");

    let mut episodes_by_comic = BTreeMap::<String, Vec<Episode>>::new();
    let mut episode_ids = BTreeSet::new();
    for raw in array(&fixture, "episodes")? {
        let id = identity(raw, "id")?;
        ensure!(episode_ids.insert(id.clone()), "Duplicate design episode identity");
        let comic_id = identity(raw, "comic_id")?;
        let ordinal = raw["ordinal"].as_u64().context("A design episode needs an ordinal")?;
        let access = match raw["access"].as_str().unwrap_or_default() {
            "free" => EpisodeAccess::Free,
            "owned" => EpisodeAccess::Owned,
            "temporary" => EpisodeAccess::Temporary,
            "locked" => EpisodeAccess::Locked,
            _ => EpisodeAccess::Unknown,
        };
        let episode = Episode {
            id,
            comic_id: comic_id.clone(),
            title: text(raw, "title"),
            short_title: ordinal.to_string(),
            order: ordinal as f64,
            access,
            // The design's month/day label is not a verified entitlement expiry.
            expires_at: None,
            pay_gold: raw["coin_price"].as_f64(),
            read: raw["read"].as_bool().unwrap_or(false),
            extra: mark_fixture(raw.clone())?,
        };
        episodes_by_comic.entry(comic_id).or_default().push(episode);
    }
    for episodes in episodes_by_comic.values_mut() {
        episodes.sort_by(|left, right| left.order.total_cmp(&right.order));
    }

    let mut comics = Vec::new();
    let mut details = Vec::new();
    let mut covers = BTreeMap::new();
    let mut comic_ids = BTreeSet::new();
    for raw in array(&fixture, "comics")? {
        let id = identity(raw, "id")?;
        ensure!(comic_ids.insert(id.clone()), "Duplicate design comic identity");
        let cover_file = text(raw, "cover_file");
        let cover = assets.get(&cover_file).context("A design cover was not bundled")?;
        image::image_dimensions(cover).context("Read a materialized design cover")?;
        covers.insert(id.clone(), cover.clone());
        let episodes_provided = episodes_by_comic.contains_key(&id);
        let episodes = if let Some(episodes) = episodes_by_comic.get(&id) {
            episodes.clone()
        } else if let Some(episode) = known_current_episode(raw, &id)? {
            ensure!(episode_ids.insert(episode.id.clone()), "Duplicate design episode identity");
            vec![episode]
        } else {
            Vec::new()
        };
        let chapter_count = raw["total_episodes"].as_u64().unwrap_or(0);
        ensure!(chapter_count <= u64::from(u32::MAX), "Invalid design chapter count");
        let read_episode = raw["read_episode"].as_u64().unwrap_or(0);
        let last = episodes.iter().find(|episode| episode.order == read_episode as f64);
        let latest = episodes.iter().find(|episode| episode.order == chapter_count as f64);
        let comic = Comic {
            id,
            title: text(raw, "title"),
            author: text(raw, "author"),
            // Cover transport stays separate from the local fixture path map.
            cover_url: String::new(),
            description: text(raw, "description"),
            genre: text(raw, "genre"),
            latest_episode_id: latest.map(|episode| episode.id.clone()),
            latest_episode_title: latest.map(|episode| episode.title.clone()).unwrap_or_default(),
            latest_order: chapter_count as f64,
            chapter_count: chapter_count as u32,
            progress: progress(raw, chapter_count),
            favorite: raw["shelf_order"].as_u64().is_some(),
            finished: raw["completed"].as_bool().unwrap_or(false),
            updated_at: 0,
            last_episode_id: last.map(|episode| episode.id.clone()),
            last_read_at: 0,
            extra: mark_fixture(raw.clone())?,
        };
        details.push(ComicDetail {
            comic: comic.clone(),
            extra: json!({
                "demo_only": true,
                "episodes_provided": episodes_provided,
                "known_current_only": !episodes_provided && !episodes.is_empty(),
                "entitlement_simulated": episodes.iter().any(|episode| episode.extra["entitlement_simulated"] == true),
                "source_sha256": fixture["source_sha256"],
            }),
            episodes,
        });
        comics.push(comic);
    }
    ensure!(episodes_by_comic.keys().all(|id| comic_ids.contains(id)),
        "A design episode refers to an unknown comic");

    let mut indices = Vec::new();
    let mut pages = BTreeMap::new();
    let mut panels = BTreeMap::new();
    let resume_episode = identity(&fixture["continue_reading"], "episode_id")?;
    for detail in &details {
        for episode in &detail.episodes {
            // Demo assets do not grant access to locked fixture episodes.
            if !episode.readable_at(0) { continue; }
            let known_count = episode.extra["page_count"].as_u64();
            let count = known_count.unwrap_or(1);
            ensure!((1..=10_000).contains(&count), "Invalid design page count");
            let mut images = Vec::with_capacity(count as usize);
            let mut local_paths = Vec::with_capacity(count as usize);
            let mut paths = Vec::with_capacity(count as usize);
            let mut page_assets = Vec::with_capacity(count as usize);
            for position in 0..count {
                let number = position + 1;
                let template = if episode.id == resume_episode
                    || episode.extra["known_current_only"] == true {
                    templates.iter().find(|template| template.number == number)
                        .unwrap_or(&templates[0])
                } else { &templates[0] };
                let mut identity = Sha256::new();
                identity.update(b"bilicomics-design-page-v1\0");
                identity.update(episode.id.as_bytes());
                identity.update([0]);
                identity.update(position.to_le_bytes());
                identity.update(template.digest.as_bytes());
                let page_id = format!("{:x}", identity.finalize());
                images.push(PageDescriptor { id: page_id.clone(), index: position as usize,
                    width: template.width, height: template.height });
                local_paths.push(template.path.clone());
                // This is a relative local fixture path, never a source/CDN URL.
                paths.push(format!("fixtures/{}", template.filename));
                panels.insert(page_id.clone(), template.panels.clone());
                page_assets.push(json!({
                    "page_id": page_id,
                    "sha256": template.digest,
                    "source_file": template.filename,
                    "source_page_number": template.number,
                    "fixture_substitution": episode.id != resume_episode || number != template.number,
                    "panels": template.panels,
                }));
            }
            let mut revision = Sha256::new();
            revision.update(b"bilicomics-design-index-v1\0");
            revision.update(serde_json::to_vec(&images)?);
            let index = ImageIndex {
                episode_id: episode.id.clone(),
                revision: format!("demo-{:x}", revision.finalize()),
                images,
                paths,
                extra: json!({
                    "demo_only": true,
                    "remote_transport": false,
                    "entitlement_simulated": episode.extra["entitlement_simulated"].as_bool().unwrap_or(false),
                    "page_count_known": known_count.is_some(),
                    "design_page_count": known_count,
                    "fixture_page_count": count,
                    "page_assets": page_assets,
                }),
            };
            pages.insert(episode.id.clone(), local_paths);
            indices.push(index);
        }
    }
    Ok(DemoData { comics, details, indices, pages, covers,
        account_name: text(&fixture["account"], "name"), fixture, panels, strip, strip_panels })
}

fn known_current_episode(raw: &Value, comic_id: &str) -> Result<Option<Episode>> {
    let ordinal = raw["read_episode"].as_u64().unwrap_or(0);
    let page_count = raw["read_pages"].as_u64().unwrap_or(0);
    if ordinal == 0 || page_count == 0 { return Ok(None); }
    ensure!(page_count <= 10_000, "Invalid known design page count");
    let comic_number: u64 = comic_id.parse().context("A design comic needs a numeric identity")?;
    let episode_number = comic_number.checked_mul(100).and_then(|base| base.checked_add(ordinal))
        .context("Known design episode identity overflow")?;
    let title = ["read_episode_title", "episode_title"].into_iter()
        .filter_map(|key| raw[key].as_str()).find(|title| !title.is_empty())
        .map(str::to_owned).unwrap_or_else(|| crate::i18n::format("common_chapter", &[("episode", ordinal.to_string())]));
    Ok(Some(Episode {
        id: episode_number.to_string(),
        comic_id: comic_id.to_owned(),
        title,
        short_title: ordinal.to_string(),
        order: ordinal as f64,
        // Readability is simulated only for the isolated design fixture.
        access: EpisodeAccess::Owned,
        expires_at: None,
        pay_gold: None,
        read: raw["read_page"].as_u64().is_some_and(|page| page >= page_count),
        extra: json!({
            "demo_only": true,
            "entitlement_simulated": true,
            "known_current_only": true,
            "source": "design_catalog_current_progress",
            "ordinal": ordinal,
            "page_count": page_count,
            "read_page": raw["read_page"],
            "current": true,
        }),
    }))
}

fn progress(raw: &Value, chapter_count: u64) -> u32 {
    if raw["completed"].as_bool().unwrap_or(false) { return 100; }
    let ordinal = raw["read_episode"].as_u64().unwrap_or(0);
    if ordinal == 0 || chapter_count == 0 { return 0; }
    let within = match (raw["read_page"].as_u64(), raw["read_pages"].as_u64()) {
        (Some(page), Some(count)) if count > 0 => (page as f64 / count as f64).clamp(0.0, 1.0),
        _ => 0.0,
    };
    (((ordinal - 1) as f64 + within) / chapter_count as f64 * 100.0).clamp(0.0, 100.0) as u32
}

fn load_templates(manifest: &Value, assets: &BTreeMap<String, PathBuf>) -> Result<TemplateAssets> {
    let mut pages = Vec::new();
    let mut strip = None;
    let mut strip_panels = Vec::new();
    for raw in array(manifest, "pages")? {
        let filename = text(raw, "filename");
        let path = assets.get(&filename).context("A design page image was not bundled")?.clone();
        let (width, height) = image::image_dimensions(&path).context("Read a materialized design page")?;
        ensure!(width > 0 && height > 0, "A design page has empty geometry");
        let panels: Vec<DemoPanel> = serde_json::from_value(raw["panels"].clone())
            .context("Read the explicit design panel regions")?;
        ensure!(panels.iter().all(|panel| {
            [panel.x, panel.y, panel.width, panel.height].into_iter().all(f64::is_finite)
                && panel.x >= 0.0 && panel.y >= 0.0 && panel.width > 0.0 && panel.height > 0.0
                && panel.x + panel.width <= 1.000_001 && panel.y + panel.height <= 1.000_001
        }), "Invalid normalized design panel geometry");
        if raw["kind"].as_str() == Some("strip") {
            ensure!(strip.is_none(), "The demo can contain only one independent strip image");
            strip = Some(path);
            strip_panels = panels;
            continue;
        }
        pages.push(TemplatePage {
            number: raw["number"].as_u64().context("A design page needs its source number")?,
            digest: format!("{:x}", Sha256::digest(embedded(&filename)?)),
            filename, path, width, height, panels,
        });
    }
    pages.sort_by_key(|page| page.number);
    Ok(TemplateAssets { pages, strip, strip_panels })
}

fn array<'a>(value: &'a Value, key: &str) -> Result<&'a [Value]> {
    value[key].as_array().map(Vec::as_slice).with_context(|| format!("Missing design array: {key}"))
}

fn text(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap_or_default().to_owned()
}

fn identity(value: &Value, key: &str) -> Result<String> {
    if let Some(number) = value[key].as_u64() { return Ok(number.to_string()); }
    let id = value[key].as_str().context("Missing design identity")?;
    ensure!(!id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit()), "Invalid design identity");
    Ok(id.to_owned())
}

fn mark_fixture(mut value: Value) -> Result<Value> {
    value.as_object_mut().context("Design metadata must be an object")?
        .insert("demo_only".into(), true.into());
    Ok(value)
}

fn embedded(name: &str) -> Result<&'static [u8]> {
    DEMO_ASSETS.iter().find_map(|(key, bytes)| (*key == name).then_some(*bytes))
        .with_context(|| format!("Missing embedded demo asset: {name}"))
}

fn safe_relative(name: &str) -> Result<PathBuf> {
    ensure!(!name.is_empty() && name.len() <= 512 && !name.contains('\\'), "Invalid demo asset path");
    let path = Path::new(name);
    ensure!(path.components().all(|part| matches!(part, Component::Normal(_)))
        && name.split('/').all(|part| !part.is_empty() && part != "." && part != ".."
            && part.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))),
        "Demo asset paths must be relative names without traversal");
    Ok(path.to_owned())
}

fn reject_link(metadata: &Metadata) -> Result<()> {
    ensure!(!metadata.file_type().is_symlink(), "Demo fixture links are not allowed");
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        ensure!(metadata.file_attributes() & 0x400 == 0, "Demo fixture reparse points are not allowed");
    }
    Ok(())
}

fn prepare_directory(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => { reject_link(&metadata)?; ensure!(metadata.is_dir(), "A demo fixture directory is occupied"); }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(path).context("Create a demo fixture directory")?;
            let metadata = fs::symlink_metadata(path)?;
            reject_link(&metadata)?;
            ensure!(metadata.is_dir(), "The demo fixture directory was replaced");
        }
        Err(error) => return Err(error).context("Inspect a demo fixture directory"),
    }
    Ok(())
}

fn materialize(data_dir: &Path) -> Result<BTreeMap<String, PathBuf>> {
    match fs::symlink_metadata(data_dir) {
        Ok(metadata) => reject_link(&metadata)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("Inspect the isolated demo data directory"),
    }
    fs::create_dir_all(data_dir).context("Create the isolated demo data directory")?;
    let root = data_dir.canonicalize().context("Resolve the isolated demo data directory")?;
    let fixtures = root.join("fixtures");
    prepare_directory(&fixtures)?;
    let mut assets = BTreeMap::new();
    let mut total_bytes = 0_usize;
    for (name, bytes) in DEMO_ASSETS {
        ensure!(bytes.len() <= MAX_ASSET_BYTES, "A bundled demo asset exceeds its size limit");
        total_bytes = total_bytes.checked_add(bytes.len()).context("Demo asset byte count overflow")?;
        ensure!(total_bytes <= MAX_TOTAL_BYTES, "Bundled demo assets exceed the total limit");
        let relative = safe_relative(name)?;
        let mut parent = fixtures.clone();
        for part in relative.parent().into_iter().flat_map(Path::components) {
            parent.push(part.as_os_str());
            prepare_directory(&parent)?;
        }
        ensure!(parent.canonicalize()?.starts_with(&fixtures), "A demo asset escaped its fixture directory");
        let destination = fixtures.join(relative);
        if !matches_asset(&destination, bytes)? {
            publish(&destination, bytes)?;
        }
        ensure!(assets.insert((*name).to_owned(), destination).is_none(), "Duplicate embedded demo asset path");
    }
    Ok(assets)
}

fn matches_asset(path: &Path, bytes: &[u8]) -> Result<bool> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error).context("Inspect an existing demo asset"),
    };
    reject_link(&metadata)?;
    ensure!(metadata.is_file(), "A demo fixture file is occupied");
    if metadata.len() != bytes.len() as u64 { return Ok(false); }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(path).context("Open an existing demo asset")?;
    let mut existing = Vec::with_capacity(bytes.len());
    file.take(bytes.len() as u64 + 1).read_to_end(&mut existing)
        .context("Read an existing demo asset within its byte limit")?;
    Ok(existing == bytes)
}

fn publish(destination: &Path, bytes: &[u8]) -> Result<()> {
    let parent = destination.parent().context("A demo asset needs a parent directory")?;
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |time| time.as_nanos());
    let temporary = parent.join(format!(".demo-{}-{stamp}-{}.pending", std::process::id(),
        NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed)));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary).context("Create a private demo asset temporary file")?;
    let result = (|| {
        file.write_all(bytes).context("Write a bundled demo asset")?;
        file.sync_all().context("Sync a bundled demo asset")?;
        drop(file);
        atomic_replace(&temporary, destination)
    })();
    if result.is_err() { let _ = fs::remove_file(&temporary); }
    result
}

#[cfg(not(windows))]
fn atomic_replace(source: &Path, destination: &Path) -> Result<()> {
    fs::rename(source, destination).context("Publish a bundled demo asset")
}

#[cfg(windows)]
fn atomic_replace(source: &Path, destination: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(existing: *const u16, replacement: *const u16, flags: u32) -> i32;
    }
    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = destination.as_os_str().encode_wide().chain(Some(0)).collect();
    // Both NUL-terminated paths remain alive for this synchronous replacement.
    if unsafe { MoveFileExW(source.as_ptr(), destination.as_ptr(), 0x1 | 0x8) } == 0 {
        return Err(std::io::Error::last_os_error()).context("Publish a bundled demo asset");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_design_data_keeps_episode_identity_geometry_and_isolation() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let demo = load(directory.path())?;
        let detail = demo.details.iter().find(|detail| detail.comic.id == "28601").unwrap();
        assert_eq!(detail.episodes.len(), 32);
        let index = demo.indices.iter().find(|index| index.episode_id == "2860112").unwrap();
        assert_eq!(index.images.len(), 45);
        assert_eq!(index.images[17].index, 17);
        assert_eq!((index.images[17].width, index.images[17].height), (1700, 2360));
        assert_ne!(index.images[0].id, index.images[1].id);
        assert_eq!(demo.panels[&index.images[17].id].len(), 5);
        assert_eq!(image::image_dimensions(demo.strip.as_ref().unwrap())?, (618, 7690));
        assert_eq!(demo.strip_panels.len(), 14);
        assert!(index.paths.iter().all(|path| path.starts_with("fixtures/") && !path.contains("://")));
        assert!(!demo.indices.iter().any(|index| index.episode_id == "2860113"));
        assert_eq!(demo.account_name, demo.fixture["account"]["name"].as_str().unwrap());
        let fixture_root = directory.path().canonicalize()?.join("fixtures");
        assert!(demo.covers.values().all(|path| path.starts_with(&fixture_root)));
        assert!(!directory.path().join("private").exists());
        assert!(!directory.path().join("session.json").exists());
        Ok(())
    }

    #[test]
    fn materialized_corruption_is_replaced_without_changing_page_identity() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let first = load(directory.path())?;
        let path = &first.pages["2860112"][17];
        fs::write(path, b"damaged fixture")?;
        let second = load(directory.path())?;
        assert_eq!(first.indices, second.indices);
        assert_eq!(image::image_dimensions(path)?, (1700, 2360));
        Ok(())
    }

    #[test]
    fn another_known_current_episode_is_readable_without_inventing_unknown_catalogs() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let demo = load(directory.path())?;
        let detail = demo.details.iter().find(|detail| detail.comic.id == "28602").unwrap();
        assert_eq!(detail.episodes.len(), 1);
        let episode = &detail.episodes[0];
        assert_eq!(episode.id, "2860203");
        assert_eq!(episode.title, crate::i18n::format("common_chapter", &[("episode", "3".into())]));
        assert_eq!(detail.comic.last_episode_id.as_deref(), Some("2860203"));
        assert!(detail.comic.latest_episode_id.is_none());
        assert_eq!(detail.comic.extra["read_page"], 7);
        assert!(episode.readable_at(0));
        assert_eq!(episode.extra["entitlement_simulated"], true);
        assert_eq!(detail.extra["episodes_provided"], false);
        let index = demo.indices.iter().find(|index| index.episode_id == episode.id).unwrap();
        assert_eq!(index.images.len(), 30);
        assert_eq!(demo.pages[&episode.id].len(), 30);
        assert_eq!(index.extra["entitlement_simulated"], true);
        assert!(index.extra["page_assets"].as_array().unwrap().iter()
            .all(|page| page["fixture_substitution"] == true));
        let unknown = demo.details.iter().find(|detail| detail.comic.id == "28606").unwrap();
        assert!(unknown.episodes.is_empty());
        assert!(unknown.comic.last_episode_id.is_none());
        assert!(!demo.indices.iter().any(|index| index.episode_id.starts_with("28606")));
        assert!(demo.details.iter().filter(|detail| detail.comic.id != "28601")
            .all(|detail| detail.episodes.len() <= 1));
        Ok(())
    }

    #[test]
    fn fixture_paths_reject_absolute_names_and_traversal() {
        for name in ["", "/outside.png", "../outside.png", "pages/../outside.png",
            "C:/outside.png", "pages\\outside.png", "pages//outside.png"] {
            assert!(safe_relative(name).is_err());
        }
        assert!(safe_relative("pages/page-18.png").is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn a_fixture_directory_link_cannot_write_into_another_directory() -> Result<()> {
        use std::os::unix::fs::symlink;
        let directory = tempfile::tempdir()?;
        let outside = tempfile::tempdir()?;
        symlink(outside.path(), directory.path().join("fixtures"))?;
        assert!(load(directory.path()).is_err());
        assert_eq!(fs::read_dir(outside.path())?.count(), 0);
        Ok(())
    }
}
