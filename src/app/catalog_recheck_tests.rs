//! Offline entry and periodic integrity checks through real controller paths.

use super::*;
use crate::{
    domain::{ImageIndex, ReadingAnchor},
    downloads::DownloadManager,
    platform::DeviceProfile,
    source::BiliClient,
    store::{DownloadRecord, DownloadStatus},
};
use std::{path::PathBuf, sync::Arc};

const COMICS: [&str; 2] = ["7001", "7002"];
const EPISODES: [&str; 2] = ["7101", "7102"];

fn fixture(handle: tokio::runtime::Handle, pages_per_episode: usize) -> Result<(tempfile::TempDir, App, Vec<Vec<PathBuf>>)> {
    let directory = tempfile::tempdir()?;
    let client = BiliClient::offline(directory.path())?;
    let mut app = super::super::test_support::app(directory.path(), DeviceProfile::Paperwhite5, client, handle)?;
    app.options.offline = true;
    app.online = false;
    app.route = "shelf".into();
    let pages = directory.path().join("accounts").join(hex_digest(app.account.as_bytes())).join("pages");
    std::fs::create_dir_all(&pages)?;
    let mut files = Vec::new();
    for (book, (comic_id, episode_id)) in COMICS.into_iter().zip(EPISODES).enumerate() {
        app.store.save_detail(&app.account, &ComicDetail {
            comic: Comic { id: comic_id.into(), title: format!("Fictional local comic {book}"),
                favorite: true, chapter_count: 1, ..Default::default() },
            episodes: vec![Episode { id: episode_id.into(), comic_id: comic_id.into(), order: 1.0,
                access: EpisodeAccess::Free, ..Default::default() }], ..Default::default()
        })?;
        let index = ImageIndex { episode_id: episode_id.into(), revision: "revision".into(),
            images: (0..pages_per_episode).map(|page| PageDescriptor { id: format!("page-{page}"),
                index: page, width: 2, height: 3 }).collect(), ..Default::default() };
        app.store.save_index(&app.account, &index)?;
        let task = format!("retained-{comic_id}");
        app.store.save_download(&app.account, &DownloadRecord { id: task.clone(), comic_id: comic_id.into(),
            episode_id: episode_id.into(), revision: "revision".into(), total_pages: pages_per_episode as u32,
            generation: 1, retained: true, status: DownloadStatus::Running, ..Default::default() })?;
        let mut book_files = Vec::new();
        for page in 0..pages_per_episode {
            let mut encoded = std::io::Cursor::new(Vec::new());
            image::DynamicImage::ImageLuma8(image::GrayImage::from_pixel(2, 3,
                image::Luma([(book * 32 + page) as u8]))).write_to(&mut encoded, image::ImageFormat::Png)?;
            let bytes = encoded.into_inner();
            let digest = hex_digest(&bytes);
            let file_name = format!("{digest}.png");
            let path = pages.join(&file_name);
            std::fs::write(&path, &bytes)?;
            let receipt = PageReceipt { episode_id: episode_id.into(), revision: "revision".into(),
                page_id: format!("page-{page}"), page_index: page, file_name, sha256: digest,
                bytes: bytes.len() as u64, width: 2, height: 3, retained: true, verified_at: 20,
                ..Default::default() };
            app.store.begin_page_commit(&app.account, &task, &receipt)?;
            app.store.commit_page(&app.account, &task, &receipt)?;
            let proof = crate::image_integrity::validate_image(&bytes, None)?;
            app.store.promote_page_validation(&app.account, &receipt, &proof, 20)?
                .context("Bind the fictional complete page to its receipt")?;
            book_files.push(path);
        }
        files.push(book_files);
    }
    app.catalog = CatalogState::prepare(&app.store, &app.account, None)?;
    Ok((directory, app, files))
}

fn apply_next(app: &mut App) -> Result<()> {
    let event = super::super::test_support::next_job(app, Duration::from_secs(10))?;
    if event.epoch == app.epoch && app.pending.get(&event.key) == Some(&event.sequence) {
        app.pending.remove(&event.key);
        (event.apply)(app);
    }
    Ok(())
}

fn complete_inventory(app: &mut App) -> Result<()> {
    app.refresh_catalog_local_snapshot();
    for _ in 0..100 {
        if !app.catalog.local_loading { break; }
        apply_next(app)?;
    }
    anyhow::ensure!(!app.catalog.local_loading && app.catalog.local_scan.is_none(),
        "The bounded inventory did not finish its fixture");
    anyhow::ensure!(app.catalog.local_snapshot.checked_all, "The final inventory is not known");
    Ok(())
}

fn corrupt_same_length(path: &Path) -> Result<()> {
    let mut bytes = std::fs::read(path)?;
    bytes[8] ^= 0x5a;
    std::fs::write(path, bytes)?;
    Ok(())
}

#[test]
fn unknown_offline_inventory_allows_real_detail_and_cover_entries_to_read_local_pages() -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build()?;
    for action in ["comic.open", "comic.cover"] {
        let (directory, mut app, _files) = fixture(runtime.handle().clone(), 1)?;
        app.downloads = Some(DownloadManager::new(directory.path(), app.account.clone(), app.epoch,
            Arc::new(app.download_source()?), runtime.handle().clone())?);
        if action == "comic.cover" {
            app.store.save_anchor(&app.account, COMICS[0], &ReadingAnchor {
                episode_id: EPISODES[0].into(), revision: "revision".into(), page_id: "page-0".into(),
                zoom_ratio: 1.0, updated_at: 20, ..Default::default()
            })?;
        }
        assert!(!app.catalog.local_snapshot.checked_all);
        assert!(app.catalog.local_snapshot.downloaded_comics.is_empty());
        let comic = app.store.comic(&app.account, COMICS[0])?.context("The local shelf comic exists")?;
        assert_eq!(app.shelf_comic_view(&comic).meta.as_str(), i18n::text("cat_local_download_checking"));
        app.dispatch("catalog", action, COMICS[0])?;
        assert!(app.pending_network.is_none(), "Unknown inventory blocked the actual {action} entry");
        if action == "comic.open" {
            assert_eq!(app.route, "detail");
            assert_eq!(app.catalog.detail.as_ref().map(|detail| detail.comic.id.as_str()), Some(COMICS[0]));
            app.dispatch("catalog", "episode.open", EPISODES[0])?;
        }
        assert_eq!(app.route, "reader");
        for _ in 0..30 {
            if app.reader_lifecycle_snapshot().is_some_and(|state|
                state["frame_width"].as_u64().is_some_and(|width| width > 0) && state["rendering"] == false)
            { break; }
            apply_next(&mut app)?;
        }
        let ready = app.reader_lifecycle_snapshot().context("The real local reader became ready")?;
        assert!(ready["frame_width"].as_u64().is_some_and(|width| width > 0));
        assert_eq!(ready["rendering"], false);
        assert!(app.pending_network.is_none());
        app.close_reader()?;
    }
    Ok(())
}

#[test]
fn repeated_incremental_refreshes_do_not_postpone_overdue_full_content_validation() -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build()?;
    let (_directory, mut app, files) = fixture(runtime.handle().clone(), 1)?;
    complete_inventory(&mut app)?;
    let full_checked = app.catalog.local_full_checked.context("A completed full validation has a clock")?;
    corrupt_same_length(&files[1][0])?;
    for _ in 0..3 {
        app.request_catalog_local_refresh();
        complete_inventory(&mut app)?;
        assert_eq!(app.catalog.local_full_checked, Some(full_checked),
            "Metadata-only refresh postponed the next complete-content recheck");
    }
    let overdue = Instant::now() - LOCAL_SCAN_INTERVAL - Duration::from_secs(1);
    app.catalog.local_full_checked = Some(overdue);
    app.catalog.local_checked = Instant::now();
    app.request_catalog_local_refresh();
    complete_inventory(&mut app)?;
    assert_eq!(app.catalog.local_snapshot.episodes[EPISODES[1]].present, 0);
    assert!(!app.catalog.local_snapshot.downloaded_comics.contains(COMICS[1]));
    assert!(app.catalog.local_snapshot.downloaded_comics.contains(COMICS[0]));
    assert!(app.catalog.local_full_checked.is_some_and(|checked| checked > overdue));
    Ok(())
}

#[test]
fn scoped_invalidation_during_a_full_pass_preserves_the_need_to_recheck_known_chapters() -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build()?;
    let (_directory, mut app, files) = fixture(runtime.handle().clone(), 6)?;
    complete_inventory(&mut app)?;
    corrupt_same_length(&files[1][0])?;
    let overdue = Instant::now() - LOCAL_SCAN_INTERVAL - Duration::from_secs(1);
    app.catalog.local_full_checked = Some(overdue);
    app.refresh_catalog_local_snapshot();
    assert!(app.catalog.local_scan_full);
    let stale = super::super::test_support::next_job(&app, Duration::from_secs(10))?;
    app.invalidate_catalog_local_episode(COMICS[0], EPISODES[0]);
    app.pending.remove(&stale.key);
    (stale.apply)(&mut app);
    assert_eq!(app.catalog.local_full_checked, Some(overdue));
    assert!(app.catalog.local_scan_full, "An interrupted full pass degraded into skipping known chapters");
    complete_inventory(&mut app)?;
    assert!(!app.catalog.local_snapshot.downloaded_comics.contains(COMICS[1]));
    assert_eq!(app.catalog.local_snapshot.episodes[EPISODES[1]].present, 5);
    assert!(app.catalog.local_snapshot.downloaded_comics.contains(COMICS[0]));
    assert!(app.catalog.local_full_checked.is_some_and(|checked| checked > overdue));
    Ok(())
}

#[test]
fn deferred_cache_maintenance_retries_from_tick_with_the_latest_active_detail_pin() -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build()?;
    let (_directory, mut app, _files) = fixture(runtime.handle().clone(), 1)?;
    for comic_id in ["8801", "8802"] {
        app.store.save_detail(&app.account, &ComicDetail {
            comic: Comic { id: comic_id.into(), title: "Fictional transient detail".into(), ..Default::default() },
            episodes: vec![Episode { id: format!("episode-{comic_id}"), comic_id: comic_id.into(),
                access: EpisodeAccess::Free, ..Default::default() }], ..Default::default()
        })?;
    }
    let newer: Vec<_> = (0..512).map(|index| Comic { id: format!("recent-{index:04}"),
        title: "Fictional cached result".into(), ..Default::default() }).collect();
    app.store.save_catalog(&app.account, "search:recent", &newer)?;
    for index in 0..33 {
        app.store.save_detail(&app.account, &ComicDetail {
            comic: Comic { id: format!("detail-{index:04}"), title: "Fictional recent detail".into(),
                ..Default::default() }, ..Default::default()
        })?;
    }
    let writer = rusqlite::Connection::open(app.store.path())?;
    writer.execute_batch("BEGIN IMMEDIATE;")?;
    let started = Instant::now();
    app.trim_catalog_cache();
    assert!(started.elapsed() < Duration::from_secs(1), "UI cache maintenance waited for a writer");
    assert!(app.catalog.cache_maintenance_pending);
    let attempts = app.catalog.cache_retry_attempts;
    app.maintain_catalog_cache();
    assert_eq!(app.catalog.cache_retry_attempts, attempts, "Maintenance did not honor its backoff");

    // The active detail changes after the failed attempt. A later successful
    // attempt must collect new pins rather than reuse a stale background list.
    app.dispatch("catalog", "comic.open", "8801")?;
    assert_eq!(app.catalog.detail.as_ref().map(|detail| detail.comic.id.as_str()), Some("8801"));
    assert!(app.pending_network.is_none());
    writer.execute_batch("ROLLBACK;")?;
    app.catalog.cache_retry_at = None;
    app.tick()?;
    assert!(!app.catalog.cache_maintenance_pending);
    assert_eq!(app.catalog.cache_retry_attempts, 0);
    assert!(app.store.load_detail(&app.account, "8801")?.is_some());
    assert!(app.store.comic(&app.account, "8802")?.is_none());
    Ok(())
}
