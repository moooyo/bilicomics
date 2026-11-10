//! Deterministic source-proof and pixel equivalence tests for one prepared frame.

use super::*;
use crate::{
    downloads::{AcquiredPage, AcquisitionFuture, AcquisitionSource, DownloadManager},
    image_integrity::identity_computation_count,
    reader::ReaderPreparationProfile,
    store::{PageReceipt, Store},
};
use std::{fs, io::Cursor};

const SOURCE_WIDTH: u32 = 19;
const SOURCE_HEIGHT: u32 = 31;
const ACCOUNT: &str = "prepared-source-fixture";
const EPOCH: u64 = 7;
const GENERATION: u64 = 41;
const ORIGIN_VERSION: u64 = 11;

fn viewport() -> Viewport { Viewport::new(37, 53) }

fn index() -> ImageIndex {
    ImageIndex { episode_id: "prepared-source-episode".into(), revision: "prepared-source-revision".into(),
        images: (0..2).map(|position| PageDescriptor { id: format!("prepared-source-page-{position}"),
            index: position, width: SOURCE_WIDTH, height: SOURCE_HEIGHT }).collect(), ..Default::default() }
}

fn settings(rotation: u16, zoom_ratio: f64, dither: bool) -> ReaderSettings {
    ReaderSettings { rotation, zoom_ratio, dither, landscape_spread: false, ..Default::default() }
}

fn encoded_image(tone: u8) -> Result<Vec<u8>> {
    let image = image::GrayImage::from_fn(SOURCE_WIDTH, SOURCE_HEIGHT, |x, y| {
        image::Luma([tone.wrapping_add((x * 7 + y * 13 + x * y) as u8)])
    });
    let mut encoded = Cursor::new(Vec::new());
    image::DynamicImage::ImageLuma8(image).write_to(&mut encoded, image::ImageFormat::Png)?;
    Ok(encoded.into_inner())
}

struct DemoFixture {
    files: ReaderFiles,
    paths: Vec<PathBuf>,
    index: ImageIndex,
    _directory: tempfile::TempDir,
}

impl DemoFixture {
    fn new() -> Result<Self> {
        let directory = tempfile::tempdir()?;
        let paths = (0..2).map(|position| {
            let path = directory.path().join(format!("prepared-source-{position}.png"));
            fs::write(&path, encoded_image(17 + position as u8 * 61)?)?;
            Ok(path)
        }).collect::<Result<Vec<_>>>()?;
        let files = ReaderFiles::Demo { paths: paths.clone(), strip: None, strip_active: false };
        Ok(Self { files, paths, index: index(), _directory: directory })
    }
}

struct Inputs {
    origin: ReaderCore,
    target: ReaderCore,
    key: NextFrameKey,
}

fn inputs(index: &ImageIndex, files: &ReaderFiles, settings: ReaderSettings) -> Result<Inputs> {
    let mut origin = ReaderCore::new(index.clone(), settings)?;
    origin.set_landscape(false);
    ready(prepare_reader_frame(&mut origin, files, viewport(), (7, 9), &AtomicBool::new(false))?)?;
    let mut target = origin.clone();
    assert_eq!(target.step(true), ReaderMove::Moved);
    assert_ne!(target.page_index(), origin.page_index());
    let key = NextFrameKey { epoch: EPOCH, generation: GENERATION, origin_version: ORIGIN_VERSION,
        origin: FrameInput::new(&origin), target: FrameInput::new(&target), viewport: viewport(), landscape: false };
    Ok(Inputs { origin, target, key })
}

fn candidate(inputs: &Inputs, files: &ReaderFiles) -> Result<NextFrame> {
    prepare_next_frame(inputs.target.clone(), inputs.key.clone(), files, &AtomicBool::new(false), None)?
        .context("The complete local source must produce one prepared frame")
}

fn ready(outcome: PreparedOutcome) -> Result<PreparedFrame> {
    match outcome {
        PreparedOutcome::Ready(frame) => Ok(frame),
        _ => anyhow::bail!("The complete fixture source must render"),
    }
}

fn assert_raster(actual: &Raster, expected: &Raster) {
    assert_eq!((actual.width, actual.height), (expected.width, expected.height));
    assert_eq!(actual.rgba, expected.rgba);
}

fn assert_optional_raster(actual: &Option<Raster>, expected: &Option<Raster>) {
    match (actual, expected) {
        (Some(actual), Some(expected)) => assert_raster(actual, expected),
        (None, None) => {},
        _ => panic!("Prepared and ordinary frames disagree about a raster slot"),
    }
}

fn assert_frame(actual: &PreparedFrame, expected: &PreparedFrame) {
    assert_raster(&actual.page, &expected.page);
    assert_optional_raster(&actual.second, &expected.second);
    assert_optional_raster(&actual.overview, &expected.overview);
    let actual_metadata = actual.metadata;
    let expected_metadata = expected.metadata;
    assert_eq!((actual_metadata.overlap, actual_metadata.left_page, actual_metadata.right_page),
        (expected_metadata.overlap, expected_metadata.left_page, expected_metadata.right_page));
    assert_eq!((actual_metadata.ratio.to_bits(), actual_metadata.focus_x.to_bits(),
        actual_metadata.focus_y.to_bits(), actual_metadata.focus_width.to_bits(), actual_metadata.focus_height.to_bits()),
        (expected_metadata.ratio.to_bits(), expected_metadata.focus_x.to_bits(),
        expected_metadata.focus_y.to_bits(), expected_metadata.focus_width.to_bits(), expected_metadata.focus_height.to_bits()));
    assert_eq!(actual.dark_fraction.to_bits(), expected.dark_fraction.to_bits());
}

fn assert_core(actual: &ReaderCore, expected: &ReaderCore) {
    assert_eq!(actual.anchor(23), expected.anchor(23));
    assert_eq!(actual.settings(), expected.settings());
    assert_eq!((actual.crop_index(), actual.crop_count(), actual.is_spread(), actual.is_transition()),
        (expected.crop_index(), expected.crop_count(), expected.is_spread(), expected.is_transition()));
}

fn assert_single_stage(profile: &ReaderPreparationProfile, stage: &str) {
    let totals = &profile.stages[stage];
    assert_eq!((totals.attempts, totals.completed, totals.partial), (1, 1, 0), "Unexpected {stage} accounting");
}

#[test]
fn a_prepared_frame_hit_hashes_current_bytes_without_resizing_or_copying_its_raster() -> Result<()> {
    let fixture = DemoFixture::new()?;
    let inputs = inputs(&fixture.index, &fixture.files, settings(0, 1.0, true))?;
    let next = candidate(&inputs, &fixture.files)?;
    let pixels = next.frame.page.rgba.as_ptr();
    let origin = inputs.origin.anchor(0);
    let mut ordinary = inputs.target.clone();
    let expected = ready(prepare_reader_frame(&mut ordinary, &fixture.files, viewport(), (7, 9),
        &AtomicBool::new(false))?)?;
    let mut actual = inputs.target.clone();
    let hashes = identity_computation_count();
    let mut profile = ReaderPreparationProfile::default();
    let frame = ready(reuse_next_frame(&mut actual, next, &fixture.files, viewport(), (7, 9),
        &AtomicBool::new(false), Some(&mut profile), None)?)?;

    assert_eq!(identity_computation_count(), hashes + 1);
    assert_eq!((profile.unchanged_sources, profile.rendered_frames, profile.resized_pixels), (1, 0, 0));
    assert_eq!((profile.decoded_sources, profile.reused_sources, profile.output_bytes), (0, 0, 0));
    assert_single_stage(&profile, "source_read");
    assert_single_stage(&profile, "source_hash");
    assert!(!profile.stages.contains_key("resize"));
    assert_eq!(frame.page.rgba.as_ptr(), pixels);
    assert_frame(&frame, &expected);
    assert_core(&actual, &ordinary);
    assert_eq!(inputs.origin.anchor(0), origin);
    Ok(())
}

#[test]
fn changed_local_pixels_fall_back_once_and_match_the_complete_ordinary_result() -> Result<()> {
    let fixture = DemoFixture::new()?;
    let inputs = inputs(&fixture.index, &fixture.files, settings(180, 1.5, true))?;
    let next = candidate(&inputs, &fixture.files)?;
    let previous = next.frame.page.rgba.clone();
    fs::write(&fixture.paths[1], encoded_image(201)?)?;
    let mut actual = inputs.target.clone();
    let mut profile = ReaderPreparationProfile::default();
    let hashes = identity_computation_count();
    let frame = ready(reuse_next_frame(&mut actual, next, &fixture.files, viewport(), (7, 9),
        &AtomicBool::new(false), Some(&mut profile), None)?)?;

    assert_eq!(identity_computation_count(), hashes + 1);
    assert_eq!((profile.unchanged_sources, profile.rendered_frames), (0, 1));
    assert!(profile.resized_pixels > 0);
    assert_single_stage(&profile, "source_read");
    assert_single_stage(&profile, "source_hash");
    assert_single_stage(&profile, "resize");
    assert_ne!(frame.page.rgba, previous);
    let mut ordinary = inputs.target.clone();
    let expected = ready(prepare_reader_frame(&mut ordinary, &fixture.files, viewport(), (7, 9),
        &AtomicBool::new(false))?)?;
    assert_frame(&frame, &expected);
    assert_core(&actual, &ordinary);
    Ok(())
}

#[test]
fn a_missing_prepared_source_never_returns_the_old_pixels() -> Result<()> {
    let fixture = DemoFixture::new()?;
    let inputs = inputs(&fixture.index, &fixture.files, settings(90, 1.0, false))?;
    let next = candidate(&inputs, &fixture.files)?;
    fs::remove_file(&fixture.paths[1])?;
    let mut actual = inputs.target.clone();
    let mut profile = ReaderPreparationProfile::default();
    let hashes = identity_computation_count();
    let outcome = reuse_next_frame(&mut actual, next, &fixture.files, viewport(), (7, 9),
        &AtomicBool::new(false), Some(&mut profile), None)?;

    assert!(matches!(outcome, PreparedOutcome::Missing(ref pages) if pages == &[1]));
    assert_eq!(identity_computation_count(), hashes);
    assert_eq!((profile.unchanged_sources, profile.rendered_frames, profile.resized_pixels), (0, 0, 0));
    assert_core(&actual, &inputs.target);
    Ok(())
}

#[test]
fn same_length_corruption_cannot_reuse_a_prepared_demo_frame() -> Result<()> {
    let fixture = DemoFixture::new()?;
    let inputs = inputs(&fixture.index, &fixture.files, settings(270, 1.0, true))?;
    let next = candidate(&inputs, &fixture.files)?;
    let mut corrupt = fs::read(&fixture.paths[1])?;
    corrupt[0] ^= 0x80;
    fs::write(&fixture.paths[1], corrupt)?;
    let mut actual = inputs.target.clone();
    let mut profile = ReaderPreparationProfile::default();
    let hashes = identity_computation_count();
    let result = reuse_next_frame(&mut actual, next, &fixture.files, viewport(), (7, 9),
        &AtomicBool::new(false), Some(&mut profile), None);

    assert!(result.is_err());
    assert_eq!(identity_computation_count(), hashes + 1);
    assert_eq!((profile.unchanged_sources, profile.rendered_frames, profile.resized_pixels), (0, 0, 0));
    assert_core(&actual, &inputs.target);
    assert!(prepare_reader_frame(&mut inputs.target.clone(), &fixture.files, viewport(), (7, 9),
        &AtomicBool::new(false)).is_err());
    Ok(())
}

#[test]
fn absent_or_corrupt_local_sources_do_not_create_a_candidate() -> Result<()> {
    for corrupt in [false, true] {
        let fixture = DemoFixture::new()?;
        let inputs = inputs(&fixture.index, &fixture.files, settings(0, 1.0, false))?;
        if corrupt { fs::write(&fixture.paths[1], b"Incomplete fixture image")?; }
        else { fs::remove_file(&fixture.paths[1])?; }
        let mut profile = ReaderPreparationProfile::default();
        let result = prepare_next_frame(inputs.target, inputs.key, &fixture.files,
            &AtomicBool::new(false), Some(&mut profile));
        if corrupt { assert!(result.is_err()); }
        else { assert!(matches!(result, Ok(None))); }
        assert_eq!((profile.rendered_frames, profile.resized_pixels), (0, 0));
    }
    Ok(())
}

#[test]
fn pre_cancelled_preparation_and_consumption_do_no_source_work() -> Result<()> {
    let fixture = DemoFixture::new()?;
    let inputs = inputs(&fixture.index, &fixture.files, settings(0, 1.0, true))?;
    let next = candidate(&inputs, &fixture.files)?;
    let hashes = identity_computation_count();
    let cancelled = AtomicBool::new(true);
    let mut preparation = ReaderPreparationProfile::default();
    assert!(prepare_next_frame(inputs.target.clone(), inputs.key, &fixture.files,
        &cancelled, Some(&mut preparation))?.is_none());
    let mut actual = inputs.target.clone();
    let mut consumption = ReaderPreparationProfile::default();
    assert!(matches!(reuse_next_frame(&mut actual, next, &fixture.files, viewport(), (7, 9),
        &cancelled, Some(&mut consumption), None)?, PreparedOutcome::Cancelled));
    assert_eq!(identity_computation_count(), hashes);
    assert_eq!(preparation, ReaderPreparationProfile::default());
    assert_eq!(consumption, ReaderPreparationProfile::default());
    assert_core(&actual, &inputs.target);
    Ok(())
}

#[test]
fn all_rotations_match_input_keys_separately_from_rendered_anchor_and_full_pixels() -> Result<()> {
    let fixture = DemoFixture::new()?;
    for rotation in [0, 90, 180, 270] {
        for zoom in [1.0, 1.5] {
            for dither in [false, true] {
                let inputs = inputs(&fixture.index, &fixture.files, settings(rotation, zoom, dither))?;
                let next = candidate(&inputs, &fixture.files)?;
                assert!(next.key.target.matches(&inputs.target));
                assert!(next.key.origin.matches(&inputs.origin));
                if rotation != 0 {
                    assert_ne!(next.core.anchor(0), next.key.target.anchor);
                    assert!(!next.key.target.matches(&next.core));
                }
                let mut state = ReadingState::pending("prepared-source-comic".into(), fixture.index.episode_id.clone(),
                    GENERATION, SavedReaderPreferences::default(), false, true);
                state.version = ORIGIN_VERSION.wrapping_add(1);
                state.presented_core = Some(inputs.origin.clone());
                state.core = Some(inputs.target.clone());
                assert!(next.key.target_is_current(&state, EPOCH, viewport(), false));
                let mut ordinary = inputs.target.clone();
                let expected = ready(prepare_reader_frame(&mut ordinary, &fixture.files, viewport(), (7, 9),
                    &AtomicBool::new(false))?)?;
                let mut actual = inputs.target;
                let frame = ready(reuse_next_frame(&mut actual, next, &fixture.files, viewport(), (7, 9),
                    &AtomicBool::new(false), None, None)?)?;
                assert_frame(&frame, &expected);
                assert_core(&actual, &ordinary);
                assert_eq!(state.presented_core.as_ref().unwrap().anchor(0), inputs.origin.anchor(0));
            }
        }
    }
    Ok(())
}

struct NoAcquisition;

impl AcquisitionSource for NoAcquisition {
    fn image_index<'a>(&'a self, _episode_id: &'a str) -> AcquisitionFuture<'a, ImageIndex> {
        panic!("Prepared frame work must not acquire an image index")
    }

    fn download_page<'a>(&'a self, _index: &'a ImageIndex, _page_index: usize) -> AcquisitionFuture<'a, AcquiredPage> {
        panic!("Prepared frame work must not acquire remote pixels")
    }
}

struct AccountFixture {
    files: ReaderFiles,
    store: Store,
    index: ImageIndex,
    paths: Vec<PathBuf>,
    _manager: DownloadManager,
    _runtime: tokio::runtime::Runtime,
    _directory: tempfile::TempDir,
}

impl AccountFixture {
    fn new() -> Result<Self> {
        let directory = tempfile::tempdir()?;
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
        let manager = DownloadManager::new(directory.path(), ACCOUNT.into(), EPOCH,
            Arc::new(NoAcquisition), runtime.handle().clone())?;
        let mut store = Store::open(manager.database_path())?;
        let index = index();
        let task = DownloadRecord { id: "prepared-source-task".into(), comic_id: "prepared-source-comic".into(),
            episode_id: index.episode_id.clone(), revision: index.revision.clone(), generation: 1,
            status: DownloadStatus::Running, total_pages: 2, created_at: 1, updated_at: 1, ..Default::default() };
        store.save_index(ACCOUNT, &index)?;
        store.save_download(ACCOUNT, &task)?;
        let mut paths = Vec::new();
        for page in &index.images {
            let encoded = encoded_image(29 + page.index as u8 * 71)?;
            let digest = hex_digest(&encoded);
            let receipt = PageReceipt { episode_id: index.episode_id.clone(), revision: index.revision.clone(),
                page_id: page.id.clone(), page_index: page.index, file_name: format!("{digest}.png"), sha256: digest,
                bytes: encoded.len() as u64, width: SOURCE_WIDTH, height: SOURCE_HEIGHT,
                verified_at: 1, ..Default::default() };
            store.begin_page_commit_generation(ACCOUNT, &task.id, task.generation, &receipt)?;
            let path = manager.pages_root().join(&receipt.file_name);
            fs::write(&path, encoded)?;
            store.commit_page_generation(ACCOUNT, &task.id, task.generation, &receipt)?;
            paths.push(path);
        }
        let files = ReaderFiles::Account(manager.page_resolver());
        Ok(Self { files, store, index, paths, _manager: manager, _runtime: runtime, _directory: directory })
    }

    fn receipt(&self) -> Result<Option<PageReceipt>> {
        self.store.page_receipt(ACCOUNT, &self.index.episode_id, &self.index.revision, &self.index.images[1].id)
    }

    fn resolver(&self) -> &LocalPageResolver {
        let ReaderFiles::Account(resolver) = &self.files else { unreachable!("The fixture owns account files") };
        resolver
    }
}

#[test]
fn account_hits_recheck_current_receipts_and_objects_without_writing_or_resizing() -> Result<()> {
    let fixture = AccountFixture::new()?;
    let inputs = inputs(&fixture.index, &fixture.files, settings(180, 1.0, true))?;
    let next = candidate(&inputs, &fixture.files)?;
    let receipt = fixture.receipt()?.context("Preparation must retain a current receipt")?;
    assert_eq!(receipt.decode_validation_version, crate::image_integrity::VALIDATION_VERSION);
    let connection = rusqlite::Connection::open(fixture.store.path())?;
    connection.execute_batch(
        "CREATE TRIGGER reject_prepared_receipt_rewrite BEFORE UPDATE ON page_receipts
         BEGIN SELECT RAISE(FAIL, 'A prepared hit must not rewrite a current receipt'); END;"
    )?;
    let mut ordinary = inputs.target.clone();
    let expected = ready(prepare_reader_frame(&mut ordinary, &fixture.files, viewport(), (7, 9),
        &AtomicBool::new(false))?)?;
    let mut actual = inputs.target;
    let mut profile = ReaderPreparationProfile::default();
    let hashes = identity_computation_count();
    let frame = ready(reuse_next_frame(&mut actual, next, &fixture.files, viewport(), (7, 9),
        &AtomicBool::new(false), Some(&mut profile), None)?)?;

    assert_eq!(identity_computation_count(), hashes + 1);
    assert_eq!((profile.unchanged_sources, profile.rendered_frames, profile.resized_pixels), (1, 0, 0));
    for stage in ["receipt_lookup", "source_read", "source_hash", "aggregate_receipt_prepare", "receipt_recheck"] {
        assert_single_stage(&profile, stage);
    }
    assert_eq!(fixture.receipt()?, Some(receipt));
    assert_frame(&frame, &expected);
    assert_core(&actual, &ordinary);
    Ok(())
}

#[test]
fn removed_account_receipts_do_not_gain_authority_from_prepared_pixels() -> Result<()> {
    let fixture = AccountFixture::new()?;
    let inputs = inputs(&fixture.index, &fixture.files, settings(0, 1.0, false))?;
    let next = candidate(&inputs, &fixture.files)?;
    fixture.store.invalidate_page(ACCOUNT, &fixture.index.episode_id, &fixture.index.revision,
        &fixture.index.images[1].id)?;
    assert!(fixture.paths[1].is_file());
    let mut actual = inputs.target.clone();
    let mut profile = ReaderPreparationProfile::default();
    let hashes = identity_computation_count();
    let outcome = reuse_next_frame(&mut actual, next, &fixture.files, viewport(), (7, 9),
        &AtomicBool::new(false), Some(&mut profile), None)?;

    assert!(matches!(outcome, PreparedOutcome::Missing(ref pages) if pages == &[1]));
    assert_eq!(identity_computation_count(), hashes);
    assert_eq!((profile.unchanged_sources, profile.rendered_frames, profile.resized_pixels), (0, 0, 0));
    assert!(fixture.receipt()?.is_none());
    assert_core(&actual, &inputs.target);
    Ok(())
}

#[test]
fn same_length_account_corruption_invalidates_the_receipt_instead_of_showing_cached_pixels() -> Result<()> {
    let fixture = AccountFixture::new()?;
    let inputs = inputs(&fixture.index, &fixture.files, settings(270, 1.0, true))?;
    let next = candidate(&inputs, &fixture.files)?;
    let mut corrupt = fs::read(&fixture.paths[1])?;
    corrupt[0] ^= 0x80;
    fs::write(&fixture.paths[1], corrupt)?;
    let mut actual = inputs.target.clone();
    let mut profile = ReaderPreparationProfile::default();
    let hashes = identity_computation_count();
    let outcome = reuse_next_frame(&mut actual, next, &fixture.files, viewport(), (7, 9),
        &AtomicBool::new(false), Some(&mut profile), None)?;

    assert!(matches!(outcome, PreparedOutcome::Missing(ref pages) if pages == &[1]));
    assert_eq!(identity_computation_count(), hashes + 1);
    assert_eq!((profile.unchanged_sources, profile.rendered_frames, profile.resized_pixels), (0, 0, 0));
    assert_single_stage(&profile, "source_hash");
    assert_single_stage(&profile, "receipt_recheck");
    assert!(fixture.receipt()?.is_none());
    assert_core(&actual, &inputs.target);
    Ok(())
}

#[cfg(unix)]
#[test]
fn a_prepared_proof_cannot_outlive_receipt_removal_inside_the_recheck_boundary() -> Result<()> {
    let fixture = AccountFixture::new()?;
    let inputs = inputs(&fixture.index, &fixture.files, settings(0, 1.0, true))?;
    let next = candidate(&inputs, &fixture.files)?;
    let cancelled = AtomicBool::new(false);
    let mut profile = ReaderPreparationProfile::default();
    let result = fixture.resolver().with_verified_page_profiled(&fixture.index.episode_id,
        &fixture.index.revision, &fixture.index.images[1].id, &cancelled,
        &mut profile, |profile| &mut profile.stages, |input, profile| {
            let prepared = crate::reader::load_hashed_source_profiled(input, Some(&next.proof), &cancelled, profile)?;
            assert!(matches!(&prepared, Some((_, None))));
            fixture.store.invalidate_page(ACCOUNT, &fixture.index.episode_id, &fixture.index.revision,
                &fixture.index.images[1].id).expect("Remove the fixture receipt before its final recheck");
            Ok(prepared)
        })?;

    assert!(result.is_none());
    assert_eq!((profile.unchanged_sources, profile.rendered_frames), (1, 0));
    assert_single_stage(&profile, "receipt_recheck");
    assert!(fixture.receipt()?.is_none());
    let mut actual = inputs.target.clone();
    assert!(matches!(reuse_next_frame(&mut actual, next, &fixture.files, viewport(), (7, 9),
        &cancelled, None, None)?, PreparedOutcome::Missing(ref pages) if pages == &[1]));
    assert_core(&actual, &inputs.target);
    Ok(())
}

#[cfg(unix)]
#[test]
fn an_atomic_object_replacement_retries_a_prepared_proof_before_rejecting_changed_bytes() -> Result<()> {
    let fixture = AccountFixture::new()?;
    let inputs = inputs(&fixture.index, &fixture.files, settings(90, 1.0, false))?;
    let next = candidate(&inputs, &fixture.files)?;
    let mut corrupt = fs::read(&fixture.paths[1])?;
    corrupt[0] ^= 0x80;
    let replacement = fixture.paths[1].with_extension("replacement");
    fs::write(&replacement, corrupt)?;
    let cancelled = AtomicBool::new(false);
    let mut profile = ReaderPreparationProfile::default();
    let hashes = identity_computation_count();
    let mut callbacks = 0;
    let result = fixture.resolver().with_verified_page_profiled(&fixture.index.episode_id,
        &fixture.index.revision, &fixture.index.images[1].id, &cancelled,
        &mut profile, |profile| &mut profile.stages, |input, profile| {
            callbacks += 1;
            let prepared = crate::reader::load_hashed_source_profiled(input, Some(&next.proof), &cancelled, profile)?;
            assert!(matches!(&prepared, Some((_, None))));
            fs::rename(&replacement, &fixture.paths[1]).expect("Replace the opened fixture object before its final recheck");
            Ok(prepared)
        })?;

    assert!(result.is_none());
    assert_eq!(callbacks, 1);
    assert_eq!(identity_computation_count(), hashes + 2);
    assert_eq!((profile.unchanged_sources, profile.rendered_frames, profile.resized_pixels), (1, 0, 0));
    assert_eq!(profile.stages["receipt_lookup"].attempts, 2);
    assert_eq!(profile.stages["source_read"].attempts, 2);
    assert_eq!(profile.stages["receipt_recheck"].attempts, 2);
    assert!(fixture.receipt()?.is_none());
    let mut actual = inputs.target.clone();
    assert!(matches!(reuse_next_frame(&mut actual, next, &fixture.files, viewport(), (7, 9),
        &cancelled, None, None)?, PreparedOutcome::Missing(ref pages) if pages == &[1]));
    assert_core(&actual, &inputs.target);
    Ok(())
}
