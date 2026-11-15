use super::*;

#[derive(Default)]
struct RecordingRefreshSink(Mutex<Vec<RefreshPlan>>);

impl RefreshSink for RecordingRefreshSink {
    fn submit(&self, plan: RefreshPlan) -> Result<(), PlatformError> {
        self.0.lock().unwrap().push(plan);
        Ok(())
    }
}

fn platform_and_sink() -> (Platform, Arc<RecordingRefreshSink>) {
    let platform = Platform::new(DeviceProfile::Scribe, false).unwrap();
    let sink = Arc::new(RecordingRefreshSink::default());
    platform.set_refresh_sink(sink.clone()).unwrap();
    (platform, sink)
}

#[test]
fn the_visible_scene_is_one_independent_atomic_choice() {
    let (platform, sink) = platform_and_sink();
    assert_eq!(platform.reader_refresh_scene(), ReaderRefreshScene::Normal);
    for scene in [ReaderRefreshScene::FastContent, ReaderRefreshScene::FastOverlay, ReaderRefreshScene::Normal] {
        platform.set_reader_refresh_scene(scene);
        assert_eq!(platform.reader_refresh_scene(), scene);
        assert!(!platform.reader_hardware_y4_requested());
    }
    assert!(sink.0.lock().unwrap().is_empty());
}

#[test]
fn continuous_fast_content_never_uses_or_resets_periodic_refresh_budgets() {
    let mut scheduler = RefreshScheduler::default();
    for _ in 0..6 { scheduler.plan(RefreshReason::Content, Duration::ZERO); }
    for _ in 0..5 { scheduler.plan(RefreshReason::ReaderTurn, Duration::ZERO); }
    let image = RefreshRect { x: 8, y: 12, width: 120, height: 80 };
    for iteration in 0..100 {
        for reason in [RefreshReason::Content, RefreshReason::ReaderTurn, RefreshReason::ReaderGeometry,
                       RefreshReason::ImageLoaded(image), RefreshReason::FeedbackReleased, RefreshReason::EnterScreen] {
            let plan = scheduler.plan_with_scene(reason, Duration::from_secs(iteration),
                                                 ReaderRefreshScene::FastContent).unwrap();
            assert_eq!(plan.kind, if matches!(reason, RefreshReason::ReaderGeometry | RefreshReason::EnterScreen) {
                RefreshKind::FastFull
            } else { RefreshKind::Fast });
        }
    }
    assert_eq!(scheduler.plan(RefreshReason::Content, Duration::ZERO).unwrap().kind, RefreshKind::Full);
    assert_eq!(scheduler.plan(RefreshReason::ReaderTurn, Duration::ZERO).unwrap().kind, RefreshKind::Full);
}

#[test]
fn fast_content_ignores_every_periodic_page_interval_and_keeps_progress_throttling() {
    for interval in [0, 1, 6, 12] {
        let mut scheduler = RefreshScheduler::default();
        scheduler.set_reader_interval(interval).unwrap();
        for _ in 0..32 {
            assert_eq!(scheduler.plan_with_scene(RefreshReason::ReaderTurn, Duration::ZERO,
                       ReaderRefreshScene::FastContent).unwrap().kind, RefreshKind::Fast);
        }
        assert!(scheduler.plan_with_scene(RefreshReason::Progress, Duration::ZERO,
                                         ReaderRefreshScene::FastContent).is_some());
        assert!(scheduler.plan_with_scene(RefreshReason::Progress, Duration::from_millis(999),
                                         ReaderRefreshScene::FastContent).is_none());
        assert!(scheduler.plan_with_scene(RefreshReason::FeedbackReleased, Duration::from_millis(999),
                                         ReaderRefreshScene::FastContent).is_some());
    }
}

#[test]
fn fast_content_maps_automatic_and_direct_ui_updates_but_preserves_manual_full() {
    let (platform, sink) = platform_and_sink();
    platform.set_reader_refresh_interval(1).unwrap();
    platform.set_reader_refresh_scene(ReaderRefreshScene::FastContent);
    for reason in [RefreshReason::Content, RefreshReason::ReaderTurn, RefreshReason::ReaderGeometry,
                   RefreshReason::FeedbackReleased] {
        assert_eq!(platform.request_refresh(reason).unwrap().unwrap().kind,
            if matches!(reason, RefreshReason::ReaderGeometry) { RefreshKind::FastFull } else { RefreshKind::Fast });
    }
    let rect = Some(RefreshRect { x: 20, y: 40, width: 100, height: 80 });
    for kind in [RefreshKind::Ui, RefreshKind::Partial, RefreshKind::FlashUi, RefreshKind::FlashPartial,
                 RefreshKind::A2, RefreshKind::Fast, RefreshKind::FastFull] {
        platform.submit_refresh(RefreshPlan { kind, rect }).unwrap();
    }
    let manual = RefreshPlan { kind: RefreshKind::Full, rect: None };
    platform.submit_refresh(manual).unwrap();
    let recorded = sink.0.lock().unwrap();
    assert!(recorded[..recorded.len() - 1].iter().all(|plan| matches!(plan.kind,
        RefreshKind::Fast | RefreshKind::FastFull)));
    assert_eq!(*recorded.last().unwrap(), manual);
    assert!(recorded[4..recorded.len() - 1].iter().all(|plan| plan.rect.is_none()));
}

#[test]
fn gray_overlay_upgrades_late_fast_feedback_and_normal_scene_keeps_its_policy() {
    let (platform, sink) = platform_and_sink();
    platform.set_reader_refresh_scene(ReaderRefreshScene::FastContent);
    let late = platform.plan_refresh(RefreshReason::FeedbackReleased).unwrap();
    platform.set_reader_refresh_scene(ReaderRefreshScene::FastOverlay);
    platform.submit_refresh(late).unwrap();
    platform.request_refresh(RefreshReason::Press).unwrap();
    platform.request_refresh(RefreshReason::Keyboard).unwrap();
    platform.request_refresh(RefreshReason::FeedbackReleased).unwrap();
    let rect = RefreshRect { x: 10, y: 20, width: 300, height: 400 };
    platform.request_refresh(RefreshReason::OverlayOpened(rect)).unwrap();
    platform.request_refresh(RefreshReason::OverlayClosed(rect)).unwrap();
    platform.set_reader_refresh_scene(ReaderRefreshScene::Normal);
    platform.request_refresh(RefreshReason::FeedbackReleased).unwrap();
    let recorded = sink.0.lock().unwrap();
    assert!(recorded[..5].iter().all(|plan| plan.kind == RefreshKind::Ui));
    assert_eq!(recorded[5], RefreshPlan { kind: RefreshKind::FlashPartial, rect: Some(rect) });
    assert_eq!(recorded[6].kind, RefreshKind::Ui);
}

#[test]
fn a_late_complete_binary_replacement_cannot_force_du_over_gray_controls() {
    let (platform, sink) = platform_and_sink();
    platform.set_reader_refresh_scene(ReaderRefreshScene::FastContent);
    let geometry = platform.plan_refresh(RefreshReason::ReaderGeometry).unwrap();
    assert_eq!(geometry.kind, RefreshKind::FastFull);
    for scene in [ReaderRefreshScene::FastOverlay, ReaderRefreshScene::Normal] {
        platform.set_reader_refresh_scene(scene);
        platform.submit_refresh(geometry).unwrap();
    }
    assert!(sink.0.lock().unwrap().iter().all(|plan| plan.kind == RefreshKind::Full));
}

#[cfg(all(feature = "kindle", target_os = "linux"))]
#[test]
fn binary_reader_plans_map_to_du_while_overlay_and_manual_clear_keep_gray_waveforms() {
    use slint_backend_kindle::{RefreshArea, RefreshWaveform};
    let (platform, _) = platform_and_sink();
    platform.set_reader_refresh_scene(ReaderRefreshScene::FastContent);
    for reason in [RefreshReason::Content, RefreshReason::ReaderTurn, RefreshReason::ReaderGeometry,
                   RefreshReason::FeedbackReleased] {
        let request = kindle_refresh_request(platform.plan_refresh(reason).unwrap());
        assert_eq!(request.waveform, RefreshWaveform::Du);
        assert!(!request.flashing);
        assert_eq!(request.area, if matches!(reason, RefreshReason::ReaderGeometry) {
            RefreshArea::Full
        } else { RefreshArea::Dirty });
    }
    platform.set_reader_refresh_scene(ReaderRefreshScene::FastOverlay);
    let overlay = kindle_refresh_request(platform.plan_refresh(RefreshReason::Press).unwrap());
    assert_eq!(overlay.waveform, RefreshWaveform::Auto);
    assert!(!overlay.flashing);
    let manual = kindle_refresh_request(RefreshPlan { kind: RefreshKind::Full, rect: None });
    assert_eq!(manual.waveform, RefreshWaveform::Gc16);
    assert!(manual.flashing);
    assert_eq!(manual.area, RefreshArea::Full);
}
