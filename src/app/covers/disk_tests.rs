use super::*;
use anyhow::Result;
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

fn name(index: u32) -> String { format!("{index:064x}.png") }

fn stamp(seconds: u64) -> SystemTime { UNIX_EPOCH + Duration::from_secs(seconds) }

fn blob(data: &Path, object: &str, bytes: usize, modified: SystemTime) -> Result<PathBuf> {
    let covers = data.join("covers");
    fs::create_dir_all(&covers)?;
    let path = covers.join(object);
    fs::write(&path, vec![b'c'; bytes])?;
    fs::OpenOptions::new().write(true).open(&path)?.set_modified(modified)?;
    Ok(path)
}

fn managed_files(data: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(data.join("covers"))? {
        let entry = entry?;
        let file_name = entry.file_name();
        let Some(file_name) = file_name.to_str() else { continue; };
        if entry.file_type()?.is_file() && file_name.len() == 68 && file_name.ends_with(".png")
            && file_name.as_bytes()[..64].iter().all(|byte| matches!(*byte, b'0'..=b'9' | b'a'..=b'f')) {
            files.push(entry.path());
        }
    }
    files.sort();
    Ok(files)
}

fn alive(_: u32) -> ProcessLiveness { ProcessLiveness::Alive }
fn dead(_: u32) -> ProcessLiveness { ProcessLiveness::Dead }
fn unknown(_: u32) -> ProcessLiveness { ProcessLiveness::Unknown }

#[test]
fn publishes_and_decodes_a_real_png_without_exposing_temporary_objects() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let object = name(1);
    let pixel = [17_u8, 83, 149, 255];
    let published = publish_cover(directory.path(), &object, &pixel.repeat(4), 2, 2)?;
    assert_eq!(published, directory.path().join("covers").join(&object));
    let (rgba, width, height) = read_cover(directory.path(), &object)?.expect("The published cover must decode");
    assert!(width > 0 && height > 0);
    assert_eq!(rgba.len(), width as usize * height as usize * 4);
    assert!(rgba.chunks_exact(4).all(|value| value == pixel.as_slice()));
    assert_eq!(fs::read_dir(directory.path().join("covers"))?.count(), 1);
    let result = maintain(directory.path(), MaintenanceRequest::default())?;
    assert!(result.evicted.is_empty());
    assert!(published.is_file());
    Ok(())
}

#[test]
fn missing_managed_cover_is_a_cache_miss() -> Result<()> {
    let directory = tempfile::tempdir()?;
    fs::create_dir(directory.path().join("covers"))?;
    assert!(read_cover(directory.path(), &name(1))?.is_none());
    Ok(())
}

#[test]
fn rejects_traversal_and_noncanonical_names_before_read_or_publish() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let invalid = [
        "cover.png".to_owned(),
        format!("../{}", name(1)),
        format!("{}.png", "A".repeat(64)),
        format!("{}.png", "a".repeat(63)),
        format!("{}.PNG", "a".repeat(64)),
    ];
    for object in invalid {
        assert!(read_cover(directory.path(), &object).is_err(), "Invalid read name was accepted: {object}");
        assert!(publish_cover(directory.path(), &object, &[0; 16], 2, 2).is_err(),
            "Invalid publish name was accepted: {object}");
    }
    assert!(!directory.path().join(name(1)).exists());
    Ok(())
}

#[test]
fn batch_touch_updates_disk_time_and_changes_the_lru_victim() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let first = blob(directory.path(), &name(1), 16, stamp(10))?;
    let second = blob(directory.path(), &name(2), 16, stamp(20))?;
    let touched = stamp(40);
    let request = MaintenanceRequest {
        touches: BTreeMap::from([(name(1), touched)]),
        protected: BTreeSet::new(),
    };
    let result = maintain_with_limits(directory.path(), request, 16, 10, unknown)?;
    assert!(first.is_file(), "The recently used cover must remain on disk");
    assert!(!second.exists(), "The previously newer but now least recently used cover must be evicted");
    assert_eq!(result.evicted, vec![name(2)]);
    assert!(fs::metadata(&first)?.modified()? >= touched, "A touch must update persistent disk recency");
    assert!(!result.needs_more);
    Ok(())
}

#[test]
fn byte_quota_is_independent_of_the_object_count_limit() -> Result<()> {
    let directory = tempfile::tempdir()?;
    blob(directory.path(), &name(1), 10, stamp(10))?;
    blob(directory.path(), &name(2), 11, stamp(20))?;
    blob(directory.path(), &name(3), 12, stamp(30))?;
    let result = maintain_with_limits(directory.path(), MaintenanceRequest::default(), 23, 100, unknown)?;
    let remaining = managed_files(directory.path())?;
    let remaining_bytes = remaining.iter().map(|path| fs::metadata(path).map(|metadata| metadata.len()))
        .collect::<std::io::Result<Vec<_>>>()?.into_iter().sum::<u64>();
    assert_eq!(result.evicted, vec![name(1)]);
    assert_eq!(remaining_bytes, 23);
    assert_eq!(remaining.len(), 2);
    assert!(!result.needs_more);
    Ok(())
}

#[test]
fn protected_cover_is_not_an_eviction_candidate() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let protected = blob(directory.path(), &name(1), 16, stamp(10))?;
    let candidate = blob(directory.path(), &name(2), 16, stamp(20))?;
    let request = MaintenanceRequest {
        touches: BTreeMap::new(),
        protected: BTreeSet::from([name(1)]),
    };
    let result = maintain_with_limits(directory.path(), request, 16, 1, unknown)?;
    assert!(protected.is_file());
    assert!(!candidate.exists());
    assert_eq!(result.evicted, vec![name(2)]);
    Ok(())
}

#[test]
fn count_pressure_converges_through_bounded_eviction_batches() -> Result<()> {
    let directory = tempfile::tempdir()?;
    for index in 0..300 { blob(directory.path(), &name(index), 1, stamp(10 + index as u64))?; }
    let mut removed = 0;
    let mut finished = false;
    for batch in 0..16 {
        let result = maintain_with_limits(directory.path(), MaintenanceRequest::default(), u64::MAX, 5, unknown)?;
        assert!(result.evicted.len() <= 128, "One maintenance batch must remain bounded");
        removed += result.evicted.len();
        if batch == 0 { assert!(result.needs_more, "The first bounded batch cannot remove all excess objects"); }
        if !result.needs_more { finished = true; break; }
    }
    assert!(finished, "Repeated bounded maintenance must converge");
    assert_eq!(managed_files(directory.path())?.len(), 5);
    assert_eq!(removed, 295);
    Ok(())
}

#[test]
fn maintenance_does_not_visit_demo_pages_retained_pins_or_unknown_objects() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let managed = blob(directory.path(), &name(1), 8, stamp(10))?;
    let sentinels = [
        directory.path().join("demo/fixtures/covers").join(name(2)),
        directory.path().join("accounts/account/pages").join(name(3)),
        directory.path().join("retained").join(name(4)),
        directory.path().join("pins").join(name(5)),
        directory.path().join("covers/demo").join(name(6)),
        directory.path().join("covers/unknown.png"),
        directory.path().join("covers/notes.pending"),
    ];
    for path in &sentinels {
        fs::create_dir_all(path.parent().expect("Every sentinel has a parent"))?;
        fs::write(path, b"preserved sentinel")?;
    }
    let named_directory = directory.path().join("covers").join(name(7));
    fs::create_dir(&named_directory)?;
    fs::write(named_directory.join("sentinel"), b"preserved directory")?;
    let result = maintain_with_limits(directory.path(), MaintenanceRequest::default(), 0, 0, dead)?;
    assert_eq!(result.evicted, vec![name(1)]);
    assert!(!managed.exists());
    for path in &sentinels { assert_eq!(fs::read(path)?, b"preserved sentinel"); }
    assert_eq!(fs::read(named_directory.join("sentinel"))?, b"preserved directory");
    Ok(())
}

#[cfg(unix)]
#[test]
fn cover_directory_symlink_is_rejected_without_changing_its_target() -> Result<()> {
    use std::os::unix::fs::symlink;
    let directory = tempfile::tempdir()?;
    let outside = tempfile::tempdir()?;
    let target = outside.path().join(name(1));
    image::save_buffer(&target, &[0; 16], 2, 2, image::ColorType::Rgba8)?;
    let original = fs::read(&target)?;
    symlink(outside.path(), directory.path().join("covers"))?;
    assert!(read_cover(directory.path(), &name(1)).is_err());
    assert!(publish_cover(directory.path(), &name(1), &[255; 16], 2, 2).is_err());
    assert!(maintain_with_limits(directory.path(), MaintenanceRequest::default(), 0, 0, dead).is_err());
    assert_eq!(fs::read(&target)?, original);
    Ok(())
}

#[cfg(unix)]
#[test]
fn managed_leaf_symlink_is_not_read_replaced_or_followed_by_gc() -> Result<()> {
    use std::os::unix::fs::symlink;
    let directory = tempfile::tempdir()?;
    let outside = tempfile::tempdir()?;
    let target = outside.path().join("external.png");
    image::save_buffer(&target, &[0; 16], 2, 2, image::ColorType::Rgba8)?;
    let original = fs::read(&target)?;
    let removable = blob(directory.path(), &name(1), 8, stamp(10))?;
    let link = directory.path().join("covers").join(name(2));
    symlink(&target, &link)?;
    assert!(read_cover(directory.path(), &name(2)).is_err());
    assert!(publish_cover(directory.path(), &name(2), &[255; 16], 2, 2).is_err());
    let result = maintain_with_limits(directory.path(), MaintenanceRequest::default(), 0, 0, dead)?;
    assert_eq!(result.evicted, vec![name(1)]);
    assert!(!removable.exists());
    assert!(fs::symlink_metadata(&link)?.file_type().is_symlink());
    assert_eq!(fs::read(&target)?, original);
    Ok(())
}

#[test]
fn temporary_cleanup_requires_a_definitively_dead_creator() -> Result<()> {
    for (liveness, should_remove) in [(alive as fn(u32) -> ProcessLiveness, false), (unknown, false), (dead, true)] {
        let directory = tempfile::tempdir()?;
        let covers = directory.path().join("covers");
        fs::create_dir(&covers)?;
        let temporary = covers.join(format!(".cover-42-{}.pending", "a".repeat(32)));
        fs::write(&temporary, b"unfinished owned cover")?;
        maintain_with_limits(directory.path(), MaintenanceRequest::default(), 0, 0, liveness)?;
        assert_eq!(!temporary.exists(), should_remove);
    }
    Ok(())
}

#[test]
fn malformed_temporary_names_are_preserved_even_if_a_process_is_dead() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let covers = directory.path().join("covers");
    fs::create_dir(&covers)?;
    let names = [
        format!(".cover-0-{}.pending", "a".repeat(32)),
        format!(".cover-00042-{}.pending", "a".repeat(32)),
        format!(".cover--42-{}.pending", "a".repeat(32)),
        format!(".cover-42-{}.pending", "A".repeat(32)),
        format!(".cover-42-{}.pending", "a".repeat(31)),
        format!(".cover-42949672960-{}.pending", "a".repeat(32)),
        "notes.pending".to_owned(),
    ];
    for object in &names { fs::write(covers.join(object), b"unknown ownership")?; }
    maintain_with_limits(directory.path(), MaintenanceRequest::default(), 0, 0, dead)?;
    for object in names { assert_eq!(fs::read(covers.join(object))?, b"unknown ownership"); }
    Ok(())
}

fn owned_temporary_name(nonce: char) -> String {
    format!(".cover-{}-{}.pending", std::process::id(), nonce.to_string().repeat(32))
}

#[test]
fn temporary_creation_collision_preserves_the_existing_file() -> Result<()> {
    let data = tempfile::tempdir()?;
    let directory = CacheDirectory::open(data.path(), true)?.expect("The cover directory must be created");
    let temporary_name = owned_temporary_name('1');
    let path = data.path().join("covers").join(&temporary_name);
    fs::write(&path, b"preexisting temporary sentinel")?;
    assert!(OwnedTemporary::create_named(&directory, temporary_name).is_err());
    assert_eq!(fs::read(path)?, b"preexisting temporary sentinel");
    Ok(())
}

#[test]
fn dropping_one_temporary_guard_does_not_remove_another_guard() -> Result<()> {
    let data = tempfile::tempdir()?;
    let directory = CacheDirectory::open(data.path(), true)?.expect("The cover directory must be created");
    let first_name = owned_temporary_name('2');
    let second_name = owned_temporary_name('3');
    let first_path = data.path().join("covers").join(&first_name);
    let second_path = data.path().join("covers").join(&second_name);
    let mut first = OwnedTemporary::create_named(&directory, first_name)?;
    let mut second = OwnedTemporary::create_named(&directory, second_name)?;
    first.file_mut().write_all(b"first owned temporary")?;
    second.file_mut().write_all(b"second owned temporary")?;
    drop(first);
    assert!(!first_path.exists());
    assert_eq!(fs::read(&second_path)?, b"second owned temporary");
    drop(second);
    assert!(!second_path.exists());
    Ok(())
}

#[cfg(unix)]
#[test]
fn temporary_guard_does_not_delete_a_replacement_with_the_same_name() -> Result<()> {
    let data = tempfile::tempdir()?;
    let directory = CacheDirectory::open(data.path(), true)?.expect("The cover directory must be created");
    let temporary_name = owned_temporary_name('4');
    let path = data.path().join("covers").join(&temporary_name);
    let mut temporary = OwnedTemporary::create_named(&directory, temporary_name)?;
    temporary.file_mut().write_all(b"original owned temporary")?;
    fs::remove_file(&path)?;
    fs::write(&path, b"replacement sentinel")?;
    // The guard keeps its original inode open until identity-checked cleanup completes.
    drop(temporary);
    assert_eq!(fs::read(path)?, b"replacement sentinel");
    Ok(())
}

#[cfg(unix)]
#[test]
fn failed_temporary_publication_preserves_a_symlink_and_cleans_only_its_source() -> Result<()> {
    use std::os::unix::fs::symlink;
    let data = tempfile::tempdir()?;
    let outside = tempfile::tempdir()?;
    let target = outside.path().join("outside-sentinel.png");
    fs::write(&target, b"external sentinel")?;
    let directory = CacheDirectory::open(data.path(), true)?.expect("The cover directory must be created");
    let link = data.path().join("covers").join(name(2));
    symlink(&target, &link)?;
    let temporary_name = owned_temporary_name('5');
    let temporary_path = data.path().join("covers").join(&temporary_name);
    let mut temporary = OwnedTemporary::create_named(&directory, temporary_name)?;
    temporary.file_mut().write_all(b"candidate prepared image")?;
    assert!(temporary.publish(&name(2)).is_err());
    assert!(!temporary_path.exists());
    assert!(fs::symlink_metadata(link)?.file_type().is_symlink());
    assert_eq!(fs::read(target)?, b"external sentinel");
    Ok(())
}

#[test]
fn managed_hardlink_cannot_read_publish_touch_or_evict_an_external_page() -> Result<()> {
    let data = tempfile::tempdir()?;
    let page = data.path().join("accounts/account/pages/original.png");
    fs::create_dir_all(page.parent().expect("The external page has a parent"))?;
    image::save_buffer(&page, &[0; 16], 2, 2, image::ColorType::Rgba8)?;
    fs::OpenOptions::new().write(true).open(&page)?.set_modified(stamp(10))?;
    let original_bytes = fs::read(&page)?;
    let original_modified = fs::metadata(&page)?.modified()?;
    let covers = data.path().join("covers");
    fs::create_dir(&covers)?;
    let link = covers.join(name(1));
    fs::hard_link(&page, &link)?;

    assert!(read_cover(data.path(), &name(1)).is_err());
    assert!(publish_cover(data.path(), &name(1), &[255; 16], 2, 2).is_err());
    let request = MaintenanceRequest {
        touches: BTreeMap::from([(name(1), stamp(40))]),
        protected: BTreeSet::new(),
    };
    let result = maintain_with_limits(data.path(), request, 0, 0, dead)?;

    assert!(result.evicted.is_empty());
    assert!(link.is_file(), "The rejected hardlink must remain untouched");
    assert_eq!(fs::read(&link)?, original_bytes);
    assert_eq!(fs::read(&page)?, original_bytes);
    assert_eq!(fs::metadata(&page)?.modified()?, original_modified);
    Ok(())
}

#[cfg(unix)]
#[test]
fn pinned_directory_operations_ignore_a_replaced_cover_path() -> Result<()> {
    use std::os::unix::fs::symlink;
    let data = tempfile::tempdir()?;
    let outside = tempfile::tempdir()?;
    let old_name = name(1);
    let retained_name = name(2);
    let outside_only_name = name(3);
    blob(data.path(), &old_name, 8, stamp(10))?;
    blob(data.path(), &retained_name, 9, stamp(20))?;
    let outside_same_name = outside.path().join(&old_name);
    let outside_only = outside.path().join(&outside_only_name);
    fs::write(&outside_same_name, b"external same-name sentinel")?;
    fs::write(&outside_only, b"external-only sentinel")?;
    let directory = CacheDirectory::open(data.path(), true)?.expect("The original cover directory must be pinned");
    let moved = data.path().join("original-covers");
    fs::rename(data.path().join("covers"), &moved)?;
    symlink(outside.path(), data.path().join("covers"))?;

    let mut names = directory.entries()?.collect::<Result<Vec<_>>>()?;
    names.sort();
    assert_eq!(names, vec![old_name.clone(), retained_name.clone()]);
    let mut original = directory.open_regular(&old_name, false)?.expect("The pinned original object must remain readable");
    let identity = file_identity(&original)?;
    let mut bytes = Vec::new();
    original.read_to_end(&mut bytes)?;
    assert_eq!(bytes, vec![b'c'; 8]);
    assert!(directory.remove_if_same(&old_name, identity)?);

    assert!(!moved.join(&old_name).exists());
    assert!(moved.join(&retained_name).is_file());
    assert_eq!(fs::read(outside_same_name)?, b"external same-name sentinel");
    assert_eq!(fs::read(outside_only)?, b"external-only sentinel");
    Ok(())
}
