use super::*;
use crate::{
    domain::EpisodeAccess,
    source::{crypto::NativeCrypto, transport::{HttpRequest, HttpResponse, ResponseFuture, Transport}},
};
use image::ImageEncoder;
use std::{fs, sync::atomic::AtomicUsize};

#[path = "resource_tests.rs"]
mod resource_tests;

const COVER_URL: &str = "https://i0.hdslb.com/synthetic-sleep-cover.png";

fn png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>> {
    let mut encoded = Vec::new();
    image::codecs::png::PngEncoder::new(&mut encoded)
        .write_image(rgba, width, height, image::ExtendedColorType::Rgba8)?;
    Ok(encoded)
}

fn dimensions(image: &slint::Image) -> (u32, u32) {
    let size = image.size();
    (size.width, size.height)
}

fn prepared_cover(width: u32, height: u32, gray: u8) -> PreparedCover {
    let mut pixels = slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(width, height);
    pixels.make_mut_slice().fill(slint::Rgb8Pixel { r: gray, g: gray, b: gray });
    PreparedCover { luma: vec![gray; (width * height) as usize], pixels, width, height }
}

fn comic() -> Comic {
    Comic { id: "sleep-comic".into(), title: "Synthetic Comic".into(),
        author: "Synthetic Author".into(), cover_url: COVER_URL.into(), ..Default::default() }
}

fn runtime() -> Result<tokio::runtime::Runtime> {
    Ok(tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_time().build()?)
}

fn reader_app(directory: &Path, handle: tokio::runtime::Handle) -> Result<App> {
    let mut app = super::super::test_support::app(directory, DeviceProfile::Paperwhite6,
        BiliClient::offline(directory)?, handle)?;
    app.options.offline = true;
    app.preferences.sleep_cover = true;
    app.preferences.reader_guidance_seen = true;
    app.demo = Some(Arc::new(DemoData { comics: Vec::new(), details: Vec::new(), indices: Vec::new(),
        pages: BTreeMap::new(), covers: BTreeMap::new(), account_name: "Synthetic account".into(),
        fixture: serde_json::json!({}), panels: BTreeMap::new(), strip: None, strip_panels: Vec::new() }));
    app.catalog.detail = Some(ComicDetail { comic: comic(), episodes: vec![Episode {
        id: "sleep-episode".into(), comic_id: comic().id, access: EpisodeAccess::Free,
        ..Default::default() }], ..Default::default() });
    // Missing fixture pages retain an actual Reader route and its metadata,
    // without starting page acquisition or constructing a synthetic ReadingState.
    app.open_episode("sleep-comic", "sleep-episode")?;
    anyhow::ensure!(app.current_sleep_comic().is_some(), "The fixture must open the Reader");
    Ok(app)
}

struct ImageTransport {
    body: Vec<u8>,
    requests: AtomicUsize,
}

impl Transport for ImageTransport {
    fn send(&self, request: HttpRequest) -> ResponseFuture<'_> {
        Box::pin(async move {
            assert_eq!(request.method, "GET");
            assert_eq!(request.url, COVER_URL);
            assert!(request.headers.iter().all(|(name, _)| !name.eq_ignore_ascii_case("cookie")));
            self.requests.fetch_add(1, Ordering::SeqCst);
            Ok(HttpResponse { status: 200, headers: Vec::new(), body: self.body.clone() })
        })
    }
}

fn connect_fixture(app: &mut App) -> Result<Arc<ImageTransport>> {
    let transport = Arc::new(ImageTransport { body: png(16, 24, &[68, 68, 68, 255].repeat(16 * 24))?,
        requests: AtomicUsize::new(0) });
    app.client = BiliClient::with_adapters(None, &app.data_dir, transport.clone(),
        Arc::new(NativeCrypto::new(&app.data_dir)));
    app.demo = None;
    // Keep the synthetic desktop DeviceControl while exercising Kindle-only
    // preparation admission; this never installs or opens a native backend.
    Arc::get_mut(&mut app.platform).expect("The fixture owns its platform").is_kindle = true;
    app.options.offline = false;
    Ok(transport)
}

fn seed_thumbnail(app: &mut App) {
    let comic = app.current_sleep_comic().expect("The fixture Reader has metadata");
    app.remember_cover(comic.id.clone(), covers::cached_cover_path(&app.data_dir, &comic.cover_url),
        [102, 102, 102, 255].repeat(8 * 12), 8, 12);
}

fn apply_job(app: &mut App, key: &str) -> Result<()> {
    let event = super::super::test_support::next_job(app, Duration::from_secs(10))?;
    anyhow::ensure!(event.key == key && event.epoch == app.epoch
        && app.pending.get(key) == Some(&event.sequence), "The expected current sleep-cover job must complete");
    app.pending.remove(key);
    (event.apply)(app);
    app.reap_sleep_cover_preparation();
    Ok(())
}

fn drive_local_phase_after_idle(app: &mut App) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        anyhow::ensure!(Instant::now() < deadline, "The local cover phase never obtained idle admission");
        app.sleep_screen.idle_not_before = Some(Instant::now());
        app.sleep_screen.retry_at = Some(Instant::now());
        app.retry_next_frame_after_delivery();
        app.prepare_sleep_screen();
        if !app.pending.contains_key(LOCAL_JOB) { std::thread::yield_now(); continue; }
        let token = app.sleep_screen.preparation.as_ref().unwrap().token.clone();
        apply_job(app, LOCAL_JOB)?;
        if !token.is_cancelled() { return Ok(()); }
        // Concurrent foreground fixtures may cancel before a worker begins.
        // Advance only this fixture's synthetic idle/backoff clocks and retry.
    }
}

fn drive_source_refill_after_idle(app: &mut App) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !app.sleep_screen.full_resolution {
        anyhow::ensure!(Instant::now() < deadline, "The current cover could not refill after Reader idle");
        app.sleep_screen.idle_not_before = Some(Instant::now());
        app.sleep_screen.retry_at = Some(Instant::now());
        app.retry_next_frame_after_delivery();
        app.prepare_sleep_screen();
        let key = [SOURCE_JOB, PREPARE_JOB].into_iter().find(|key| app.pending.contains_key(*key));
        if let Some(key) = key { apply_job(app, key)?; }
        else { std::thread::yield_now(); }
    }
    Ok(())
}

struct HeadlessPlatform { window: Rc<slint::platform::software_renderer::MinimalSoftwareWindow> }

impl slint::platform::Platform for HeadlessPlatform {
    fn create_window_adapter(&self) -> std::result::Result<Rc<dyn slint::platform::WindowAdapter>, slint::PlatformError> {
        Ok(self.window.clone())
    }
}

fn with_ui(work: impl FnOnce(&AppWindow) -> Result<()> + Send + 'static) -> Result<()> {
    std::thread::spawn(move || {
        let window = slint::platform::software_renderer::MinimalSoftwareWindow::new(
            slint::platform::software_renderer::RepaintBufferType::ReusedBuffer);
        slint::platform::set_platform(Box::new(HeadlessPlatform { window }))?;
        let ui = AppWindow::new()?;
        work(&ui)
    }).join().map_err(|_| anyhow::anyhow!("The isolated sleep-screen UI check panicked"))?
}

#[test]
fn complete_covers_keep_their_aspect_ratio_in_both_orientations() -> Result<()> {
    for (width, height, expected) in [(2560, 1280, (1536, 768)), (1280, 2560, (1024, 2048))] {
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                let gray = match (x < width / 2, y < height / 2) {
                    (true, true) => 0, (false, true) => 85,
                    (true, false) => 170, (false, false) => 255,
                };
                pixels.extend_from_slice(&[gray, gray, gray, 255]);
            }
        }
        let prepared = decode_cover(png(width, height, &pixels)?)?;
        assert_eq!((prepared.width, prepared.height), expected);
        assert_eq!(u64::from(prepared.width) * u64::from(height), u64::from(prepared.height) * u64::from(width));
        let last_row = ((prepared.height - 1) * prepared.width) as usize;
        assert_eq!([prepared.luma[0], prepared.luma[(prepared.width - 1) as usize],
            prepared.luma[last_row], *prepared.luma.last().unwrap()], [0, 85, 170, 255]);
    }
    let small = decode_cover(png(16, 24, &[68, 68, 68, 255].repeat(16 * 24))?)?;
    assert_eq!((small.width, small.height), (16, 24), "Small source images must not be needlessly enlarged");
    Ok(())
}

#[test]
fn transparency_uses_white_paper_and_prepared_pixels_are_four_bit_grayscale() -> Result<()> {
    let rgba = [
        0, 0, 0, 0, 0, 0, 0, 255, 255, 255, 255, 255, 0, 0, 0, 128,
        255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255,
    ];
    let prepared = decode_cover(png(7, 1, &rgba)?)?;
    assert_eq!(prepared.luma, [255, 0, 255, 119, 51, 187, 17]);
    assert!(prepared.luma.iter().all(|gray| gray % 17 == 0));
    assert_eq!(prepared.pixels.as_bytes(), prepared.luma.iter()
        .flat_map(|gray| [*gray; 3]).collect::<Vec<_>>());
    Ok(())
}

#[test]
fn worker_rgb_preparation_observes_cancellation_at_bounded_pixel_chunks() -> Result<()> {
    let checks = std::cell::Cell::new(0_u32);
    let encoded = png(1536, 16, &[68, 68, 68, 255].repeat(1536 * 16))?;
    let result = decode_cover_cancellable(encoded, &|| {
        checks.set(checks.get() + 1);
        anyhow::ensure!(checks.get() < 5, "Controlled sleep-cover cancellation");
        Ok(())
    }, None);
    assert!(result.is_err());
    assert_eq!(checks.get(), 5, "Conversion must check between bounded chunks before preparing the whole RGB payload");
    Ok(())
}

#[test]
fn reader_activity_keeps_ready_memory_and_backoff_while_extending_idle() -> Result<()> {
    let runtime = runtime()?;
    let directory = tempfile::tempdir()?;
    let mut app = reader_app(directory.path(), runtime.handle().clone())?;
    app.sleep_screen.identity = Some(CoverIdentity::new(&app.account, app.epoch, &comic()).unwrap());
    app.install_sleep_cover(prepared_cover(16, 24, 68));
    let retry_at = Instant::now() + Duration::from_secs(60);
    app.sleep_screen.retry_at = Some(retry_at);
    app.sleep_screen.failures = 3;
    let generation = app.sleep_screen.generation;
    let before = Instant::now();
    app.defer_sleep_cover_for_reader_activity();
    let first_idle = app.sleep_screen.idle_not_before.unwrap();
    assert!(first_idle >= before + READER_IDLE_DELAY);
    app.defer_sleep_cover_for_reader_activity();
    assert!(app.sleep_screen.idle_not_before.unwrap() >= first_idle);
    assert_eq!(app.sleep_screen.generation, generation + 2);
    assert_eq!(app.sleep_screen.retry_at, Some(retry_at));
    assert_eq!(app.sleep_screen.failures, 3);
    assert!(app.sleep_screen.full_resolution);
    assert_eq!(dimensions(app.sleep_screen.cover.as_ref().unwrap()), (16, 24));
    let snapshot = app.sleep_cover_resource_snapshot();
    assert_eq!(snapshot["ready"], true);
    assert_eq!(snapshot["full"], true);
    assert_eq!((snapshot["width"].as_u64(), snapshot["height"].as_u64()), (Some(16), Some(24)));
    assert!(snapshot["idle_remaining_ms"].as_u64().unwrap() > 0);
    Ok(())
}

#[test]
fn idle_defers_cover_misses_and_survives_identity_reset_even_with_sleep_cover_disabled() -> Result<()> {
    let runtime = runtime()?;
    let directory = tempfile::tempdir()?;
    let mut app = reader_app(directory.path(), runtime.handle().clone())?;
    let transport = connect_fixture(&mut app)?;
    let sequence = app.sequence;
    for _ in 0..4 { app.prepare_sleep_screen(); }
    assert_eq!(app.sequence, sequence, "A fallback thumbnail miss must not spawn an ordinary cover job");
    assert!(app.pending.is_empty());
    assert_eq!(transport.requests.load(Ordering::SeqCst), 0);
    let deadline = app.sleep_screen.idle_not_before.unwrap();
    app.preferences.sleep_cover = false;
    app.prepare_sleep_screen();
    assert!(app.sleep_screen.idle_not_before.unwrap() >= deadline);
    assert!(!app.optional_cover_work_allowed(), "Maintenance must honor Reader idle independently of the sleep preference");
    app.schedule_cover_maintenance(true);
    assert_eq!(app.sequence, sequence);
    assert!(!app.pending.contains_key("cover-maintenance"));
    Ok(())
}

#[test]
fn resident_preview_is_source_bound_and_reader_cover_misses_create_no_regular_jobs() -> Result<()> {
    let runtime = runtime()?;
    let directory = tempfile::tempdir()?;
    let mut app = reader_app(directory.path(), runtime.handle().clone())?;
    connect_fixture(&mut app)?;
    seed_thumbnail(&mut app);
    assert_eq!(dimensions(&app.cached_cover_preview(&comic()).unwrap()), (8, 12));
    let mut replacement = comic();
    replacement.cover_url.push_str("?source=replaced");
    assert!(app.cached_cover_preview(&replacement).is_none());
    let sequence = app.sequence;
    let preview = app.cover_image(&comic());
    assert_eq!(dimensions(&preview), (8, 12));
    let missing = Comic { id: "missing-cover".into(), cover_url: COVER_URL.into(), ..Default::default() };
    assert_eq!(dimensions(&app.cover_image(&missing)), (0, 0));
    app.request_cover(missing.clone(), covers::cached_cover_path(&app.data_dir, &missing.cover_url));
    assert_eq!(app.sequence, sequence);
    assert!(app.pending.is_empty());
    assert!(app.cover_pending.is_empty());
    assert_eq!(dimensions(&app.cached_cover_preview(&comic()).unwrap()), (8, 12));
    Ok(())
}

#[test]
fn encoded_and_decoded_source_budgets_are_enforced() -> Result<()> {
    assert!(decode_cover(vec![0; MAX_ENCODED_BYTES + 1]).is_err());
    assert!(decode_cover(png(4097, 1, &[0, 0, 0, 255].repeat(4097))?).is_err());
    assert!(decode_cover(png(1, 4097, &[0, 0, 0, 255].repeat(4097))?).is_err());
    assert!(decode_cover(b"invalid encoded image".to_vec()).is_err());
    Ok(())
}

#[test]
fn identities_separate_accounts_comics_sources_and_live_epochs() {
    let comic = comic();
    let first = CoverIdentity::new("account-one", 1, &comic).unwrap();
    let epoch = CoverIdentity::new("account-one", 2, &comic).unwrap();
    assert_ne!(first, epoch);
    assert_eq!(first.file_name, epoch.file_name, "A restart may reuse the same account-scoped derivative");
    assert_ne!(first.file_name, CoverIdentity::new("account-two", 1, &comic).unwrap().file_name);
    let mut changed = comic.clone();
    changed.id.push_str("-other");
    assert_ne!(first.file_name, CoverIdentity::new("account-one", 1, &changed).unwrap().file_name);
    changed = comic.clone();
    changed.cover_url.push_str("?source=replacement");
    assert_ne!(first.file_name, CoverIdentity::new("account-one", 1, &changed).unwrap().file_name);
    assert_eq!(first.file_name.len(), 68);
    assert!(first.file_name.ends_with(".png"));
    assert!(first.file_name[..64].bytes().all(|byte| byte.is_ascii_hexdigit()));
    let mut ambiguous = comic.clone();
    ambiguous.id = "bc".into();
    let left = CoverIdentity::new("a", 1, &ambiguous).unwrap();
    ambiguous.id = "c".into();
    assert_ne!(left.file_name, CoverIdentity::new("ab", 1, &ambiguous).unwrap().file_name);
    assert!(CoverIdentity::new(&"a".repeat(4097), 1, &comic).is_none());
    changed.cover_url = "u".repeat(8193);
    assert!(CoverIdentity::new("account-one", 1, &changed).is_none());
}

#[test]
fn prepared_metadata_is_bounded_by_unicode_scalars() {
    let title = "\u{00e9}".repeat(300);
    let author = "\u{1f4d6}".repeat(200);
    assert_eq!(bounded_text(&title, 256).chars().count(), 256);
    assert_eq!(bounded_text(&author, 128).chars().count(), 128);
}

#[test]
fn leaving_reader_does_not_select_residual_reading_metadata() -> Result<()> {
    let runtime = runtime()?;
    let directory = tempfile::tempdir()?;
    let mut app = reader_app(directory.path(), runtime.handle().clone())?;
    let identity = CoverIdentity::new(&app.account, app.epoch, &comic()).unwrap();
    app.sleep_screen.identity = Some(identity.clone());
    app.install_sleep_cover(prepared_cover(2, 2, 68));
    app.route = "account".into();
    assert!(app.reading.is_some(), "Retain residual Reader state to exercise route selection");
    assert!(app.current_sleep_comic().is_none());
    assert!(!app.sleep_cover_is_current(&identity));
    app.prepare_sleep_screen();
    assert!(app.sleep_screen.identity.is_none());
    assert!(app.sleep_screen.cover.is_none());
    assert!(app.pending.is_empty());
    Ok(())
}

#[test]
fn a_cold_offline_reader_uses_its_disk_thumbnail_then_refills_the_full_cover_online() -> Result<()> {
    let runtime = runtime()?;
    let _serial = runtime.block_on(source::work::process_test_lock().lock());
    let directory = tempfile::tempdir()?;
    let mut app = reader_app(directory.path(), runtime.handle().clone())?;
    let transport = connect_fixture(&mut app)?;
    app.options.offline = true;
    assert!(app.cover_images.is_empty() && app.cover_files.is_empty());
    let identity = CoverIdentity::new(&app.account, app.epoch, &comic()).unwrap();
    let thumbnail_path = covers::cached_cover_path(directory.path(), COVER_URL);
    let thumbnail = png(8, 12, &[102, 102, 102, 255].repeat(8 * 12))?;
    fs::create_dir(directory.path().join("covers"))?;
    fs::write(&thumbnail_path, &thumbnail)?;
    assert!(!directory.path().join("covers").join(&identity.file_name).exists());
    let sequence = app.sequence;
    app.prepare_sleep_screen();
    assert_eq!(app.sequence, sequence, "A disk thumbnail still waits for idle optional admission");
    drive_local_phase_after_idle(&mut app)?;
    assert!(app.sleep_screen.local_checked);
    assert!(!app.sleep_screen.full_resolution && !app.sleep_screen.pending);
    assert_eq!(dimensions(app.sleep_screen.cover.as_ref().unwrap()), (8, 12));
    assert_eq!(app.sleep_cover_resource_snapshot()["full"], false);
    assert!(app.cover_images.is_empty() && app.cover_files.is_empty(),
        "Offline fallback must not restart the ordinary cover pipeline");
    assert_eq!(transport.requests.load(Ordering::SeqCst), 0);
    assert!(!directory.path().join("covers").join(&identity.file_name).exists());
    let sequence = app.sequence;
    for _ in 0..3 {
        app.retry_next_frame_after_delivery();
        app.prepare_sleep_screen();
    }
    assert_eq!(app.sequence, sequence, "A ready offline thumbnail must not be redecoded every tick");
    assert_eq!(transport.requests.load(Ordering::SeqCst), 0);
    app.options.offline = false;
    drive_source_refill_after_idle(&mut app)?;
    assert!(app.sleep_screen.full_resolution);
    assert_eq!(dimensions(app.sleep_screen.cover.as_ref().unwrap()), (16, 24));
    assert!(transport.requests.load(Ordering::SeqCst) >= 1);
    assert!(covers::read_sleep_cover(directory.path(), &identity.file_name)?.is_some());
    assert_eq!(fs::read(thumbnail_path)?, thumbnail);
    Ok(())
}

#[test]
fn corrupt_local_derivative_can_refill_from_the_current_source_while_awake() -> Result<()> {
    let runtime = runtime()?;
    let _serial = runtime.block_on(source::work::process_test_lock().lock());
    let directory = tempfile::tempdir()?;
    let mut app = reader_app(directory.path(), runtime.handle().clone())?;
    let transport = connect_fixture(&mut app)?;
    seed_thumbnail(&mut app);
    let identity = CoverIdentity::new(&app.account, app.epoch, &comic()).unwrap();
    fs::create_dir(directory.path().join("covers"))?;
    fs::write(directory.path().join("covers").join(&identity.file_name), b"corrupt derivative")?;
    app.prepare_sleep_screen();
    assert!(app.pending.is_empty(), "A new candidate must wait for Reader idle");
    drive_local_phase_after_idle(&mut app)?;
    assert!(app.sleep_screen.local_checked);
    assert!(app.sleep_screen.retry_at.is_some());
    assert_eq!(transport.requests.load(Ordering::SeqCst), 0);
    assert_eq!(dimensions(app.sleep_screen.cover.as_ref().unwrap()), (8, 12));
    drive_source_refill_after_idle(&mut app)?;
    assert!(transport.requests.load(Ordering::SeqCst) >= 1);
    assert!(app.sleep_screen.full_resolution);
    assert_eq!(dimensions(app.sleep_screen.cover.as_ref().unwrap()), (16, 24));
    let cached = covers::read_sleep_cover(directory.path(), &identity.file_name)?.unwrap();
    let decoded = decode_cover(cached)?;
    assert_eq!((decoded.width, decoded.height), (16, 24));
    assert!(decoded.luma.iter().all(|gray| *gray == 68));
    Ok(())
}

#[test]
fn a_delayed_source_result_cannot_replace_the_new_account_epoch_candidate() -> Result<()> {
    let runtime = runtime()?;
    let directory = tempfile::tempdir()?;
    let mut app = reader_app(directory.path(), runtime.handle().clone())?;
    let transport = connect_fixture(&mut app)?;
    let identity = CoverIdentity::new(&app.account, app.epoch, &comic()).unwrap();
    app.sleep_screen.identity = Some(identity.clone());
    app.sleep_screen.can_fetch = true;
    // Start after the local probe and Reader idle admission. The delayed
    // result below is a held envelope, not a bypass of foreground priority.
    app.sleep_screen.local_checked = true;
    app.sleep_screen.elapse_reader_idle_for_test();
    app.retry_next_frame_after_delivery();
    assert!(app.sleep_cover_can_start(), "The source fixture must first pass idle admission");
    app.request_sleep_cover(identity.clone(), COVER_URL.into());
    assert!(app.pending.contains_key(SOURCE_JOB), "The admitted source fixture must reserve its job before waiting");
    let event = super::super::test_support::next_job(&app, Duration::from_secs(10))?;
    assert_eq!(event.key, SOURCE_JOB);
    app.pending.remove(SOURCE_JOB);
    app.epoch += 1;
    let replacement = CoverIdentity::new(&app.account, app.epoch, &comic()).unwrap();
    app.sleep_screen = SleepScreenState { identity: Some(replacement.clone()), title: "Current candidate".into(),
        ..Default::default() };
    // Exercise the callback's own identity fence even after its consumer
    // already took the old envelope before an account transition.
    (event.apply)(&mut app);
    assert_eq!(transport.requests.load(Ordering::SeqCst), 1);
    assert_eq!(app.sleep_screen.identity, Some(replacement));
    assert_eq!(app.sleep_screen.title.as_str(), "Current candidate");
    assert!(!app.sleep_screen.pending);
    assert!(!app.pending.contains_key(PREPARE_JOB));
    assert!(!directory.path().join("covers").join(identity.file_name).exists());
    Ok(())
}

#[test]
fn prepared_memory_is_shown_after_the_sleep_gate_without_storage_or_new_jobs() -> Result<()> {
    with_ui(|ui| {
        let runtime = runtime()?;
        let directory = tempfile::tempdir()?;
        let mut app = reader_app(directory.path(), runtime.handle().clone())?;
        let transport = connect_fixture(&mut app)?;
        app.ui = ui.as_weak();
        app.sleep_screen.identity = Some(CoverIdentity::new(&app.account, app.epoch, &comic()).unwrap());
        app.install_sleep_cover(prepared_cover(16, 24, 68));
        app.prepare_sleep_screen();
        ui.global::<ReaderModel>().set_page(cover_image(prepared_cover(2, 2, 136)));
        covers::publish_sleep_cover(directory.path(), &app.sleep_screen.identity.as_ref().unwrap().file_name,
            &[68; 4], 2, 2)?;
        fs::remove_dir_all(directory.path().join("covers"))?;
        let sequence = app.sequence;
        app.begin_suspend_intent_gate();
        assert!(app.is_suspended());
        assert!(app.sleep_cover_enabled());
        app.prepare_sleep_screen();
        app.show_sleep_screen()?;
        let model = ui.global::<SleepScreenModel>();
        assert!(model.get_active() && model.get_has_cover());
        assert_eq!(dimensions(&model.get_cover()), (16, 24));
        assert_eq!(model.get_title().as_str(), "Synthetic Comic");
        assert_eq!(model.get_author().as_str(), "Synthetic Author");
        assert_eq!(app.route, "reader");
        assert_eq!(app.sequence, sequence);
        assert!(app.pending.is_empty());
        assert!(app.jobs.take().is_none());
        assert_eq!(transport.requests.load(Ordering::SeqCst), 0);
        app.hide_sleep_screen();
        app.hide_sleep_screen();
        assert!(!model.get_active() && !model.get_has_cover());
        assert_eq!(dimensions(&model.get_cover()), (0, 0));
        assert!(model.get_title().is_empty() && model.get_author().is_empty());
        assert_eq!(dimensions(&ui.global::<ReaderModel>().get_page()), (2, 2));
        assert_eq!(app.current_sleep_comic().unwrap().id, comic().id);
        Ok(())
    })
}

#[test]
fn stale_cover_uses_current_text_and_non_reader_routes_use_neutral_presentation() -> Result<()> {
    with_ui(|ui| {
        let runtime = runtime()?;
        let directory = tempfile::tempdir()?;
        let mut app = reader_app(directory.path(), runtime.handle().clone())?;
        app.ui = ui.as_weak();
        app.sleep_screen.identity = Some(CoverIdentity::new("another-account", app.epoch, &comic()).unwrap());
        app.sleep_screen.title = "Stale title".into();
        app.install_sleep_cover(prepared_cover(2, 2, 68));
        app.show_sleep_screen()?;
        let model = ui.global::<SleepScreenModel>();
        assert!(!model.get_has_cover());
        assert_eq!(model.get_title().as_str(), comic().title);
        assert_eq!(model.get_author().as_str(), comic().author);
        app.route = "shelf".into();
        app.show_sleep_screen()?;
        assert!(!model.get_has_cover());
        assert_eq!(model.get_title(), tr("app_name"));
        assert_eq!(model.get_author(), tr("sleep_neutral"));
        assert!(app.pending.is_empty());
        Ok(())
    })
}

#[test]
fn system_preference_and_logout_clear_the_prepared_candidate_and_its_jobs() -> Result<()> {
    let runtime = runtime()?;
    let directory = tempfile::tempdir()?;
    let mut app = reader_app(directory.path(), runtime.handle().clone())?;
    app.sleep_screen.identity = Some(CoverIdentity::new(&app.account, app.epoch, &comic()).unwrap());
    app.install_sleep_cover(prepared_cover(2, 2, 68));
    let mut tickets = Vec::new();
    for key in [LOCAL_JOB, SOURCE_JOB, PREPARE_JOB] {
        let (_, _, ticket) = app.begin_job(key, false)?;
        tickets.push(ticket);
    }
    app.preferences.sleep_cover = false;
    assert!(!app.sleep_cover_enabled());
    app.prepare_sleep_screen();
    assert!(app.sleep_screen.identity.is_none() && app.sleep_screen.cover.is_none());
    assert!(app.pending.is_empty());
    app.preferences.sleep_cover = true;
    app.sleep_screen.identity = Some(CoverIdentity::new(&app.account, app.epoch, &comic()).unwrap());
    app.install_sleep_cover(prepared_cover(2, 2, 68));
    let epoch = app.epoch;
    app.demo = None;
    app.logout()?;
    assert!(app.epoch > epoch);
    assert!(app.sleep_screen.identity.is_none() && app.sleep_screen.cover.is_none());
    assert!(!app.sleep_screen.full_resolution && !app.sleep_screen.pending);
    assert!(app.current_sleep_comic().is_none());
    assert!([LOCAL_JOB, SOURCE_JOB, PREPARE_JOB].iter().all(|key| !app.pending.contains_key(*key)));
    drop(tickets);
    Ok(())
}

#[test]
fn transient_retry_is_bounded_and_capacity_deferral_does_not_inflate_it() -> Result<()> {
    let runtime = runtime()?;
    let directory = tempfile::tempdir()?;
    let mut app = reader_app(directory.path(), runtime.handle().clone())?;
    for _ in 0..20 {
        let before = Instant::now();
        app.defer_sleep_cover(false);
        let after = Instant::now();
        let deadline = app.sleep_screen.retry_at.unwrap();
        assert!(deadline >= before + RETRY_DELAY && deadline <= after + Duration::from_secs(300));
    }
    let failures = app.sleep_screen.failures;
    let before = Instant::now();
    app.defer_sleep_cover(true);
    let after = Instant::now();
    assert_eq!(app.sleep_screen.failures, failures);
    let deadline = app.sleep_screen.retry_at.unwrap();
    assert!(deadline >= before + CAPACITY_RETRY && deadline <= after + CAPACITY_RETRY);
    Ok(())
}

#[test]
fn kindle_sleep_cover_actions_persist_explicit_choices_across_account_initialization() -> Result<()> {
    let runtime = runtime()?;
    let directory = tempfile::tempdir()?;
    let client = BiliClient::offline(directory.path())?;
    let session = AuthSession::from_cookie_text(
        "SESSDATA=synthetic-sleep-session; bili_jct=synthetic-sleep-csrf; DedeUserID=11"
    )?.with_identity(crate::source::AccountIdentity { id: "11".into(),
        name: "Synthetic Kindle Account".into(), avatar_url: String::new() }, 1)?;
    client.replace_session(Some(session))?;
    let mut app = super::super::test_support::app(directory.path(), DeviceProfile::Paperwhite6,
        client, runtime.handle().clone())?;
    app.options.offline = true;
    // Keep the synthetic desktop DeviceControl and set only admission metadata;
    // no native backend or device operation is involved in this preference check.
    Arc::get_mut(&mut app.platform).expect("The fixture owns its platform").is_kindle = true;
    assert!(app.demo.is_none());
    assert_eq!(app.client.session().and_then(|session| session.account_key().map(str::to_owned)),
        Some(app.account.clone()));
    assert!(Preferences::default().sleep_cover);
    app.initialize_account()?;
    assert!(app.sleep_cover_enabled(), "A real account must retain the default cover preference");

    app.account_action("sleep-cover", "system")?;
    assert!(!app.sleep_cover_enabled());
    let saved: Preferences = app.store.preference(&app.account, "preferences")?
        .context("The system-screen preference must be persisted")?;
    assert!(!saved.sleep_cover);

    app.preferences = Preferences::default();
    app.preferences = app.store.preference(&app.account, "preferences")?
        .context("The explicit system-screen choice must reload")?;
    app.initialize_account()?;
    assert!(!app.sleep_cover_enabled(), "An explicit false must survive account initialization");

    app.account_action("sleep-cover", "cover")?;
    assert!(app.sleep_cover_enabled());
    let saved: Preferences = app.store.preference(&app.account, "preferences")?
        .context("The cover-screen preference must be persisted")?;
    assert!(saved.sleep_cover);
    app.preferences.sleep_cover = false;
    app.preferences = app.store.preference(&app.account, "preferences")?
        .context("The explicit cover-screen choice must reload")?;
    let prepared = super::super::account::PreparedAccount::load(&app.store, &app.account, None)?;
    app.initialize_account_prepared(prepared)?;
    assert!(app.sleep_cover_enabled(), "Prepared real-account attachment must retain the cover preference");
    Ok(())
}
