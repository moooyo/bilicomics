use super::*;

fn offline() -> Connectivity {
    Connectivity { online: false, offline: true, demo: false }
}

fn online() -> Connectivity {
    Connectivity { online: true, offline: false, demo: false }
}

fn observe(state: &mut CoverState, id: &str, url: &str, connectivity: Connectivity, now: Instant) -> Observation {
    state.observe("fixture-account", id, url, connectivity, now).expect("Bounded fixture metadata")
}

#[test]
fn offline_missing_cover_suppresses_repeated_frames_and_allows_a_later_local_probe() {
    let mut state = CoverState::default();
    let now = Instant::now();
    let first = observe(&mut state, "1", "https://fixture.invalid/cover.png", offline(), now);
    assert!(state.begin(&first.token));
    state.local_finished(&first.token);
    state.failed(&first.token, NegativeReason::Offline, now);
    for frame in 1..1_000 {
        let next = observe(&mut state, "1", "https://fixture.invalid/cover.png", offline(), now + Duration::from_millis(frame));
        assert!(next.blocked);
        assert!(!state.begin(&next.token));
    }
    assert_eq!(state.entries.len(), 1);
    assert_eq!(state.entries["1"].negative.as_ref().unwrap().reason, NegativeReason::Offline);
    let retry = observe(&mut state, "1", "https://fixture.invalid/cover.png", offline(), now + LOCAL_RECHECK);
    assert!(!retry.blocked);
    assert!(state.begin(&retry.token));
}

#[test]
fn empty_url_negative_state_is_invalidated_by_a_full_url_change() {
    let mut state = CoverState::default();
    let now = Instant::now();
    let missing = observe(&mut state, "1", "", online(), now);
    state.failed(&missing.token, NegativeReason::EmptyUrl, now);
    assert!(observe(&mut state, "1", "", online(), now).blocked);
    assert!(observe(&mut state, "1", "", online(), now + Duration::from_secs(86_400)).blocked);
    assert!(!state.take_due_retries(now + Duration::from_secs(86_400)));
    let available = observe(&mut state, "1", "https://fixture.invalid/cover.png?token=new", online(), now);
    assert!(available.url_changed);
    assert!(!available.blocked);
    assert!(!state.current(&missing.token));
    assert!(state.current(&available.token));
}

#[test]
fn query_parameters_remain_part_of_the_cover_identity() {
    let mut state = CoverState::default();
    let now = Instant::now();
    let first = observe(&mut state, "1", "https://fixture.invalid/cover.png?token=first&size=480", online(), now);
    assert!(state.begin(&first.token));
    let replacement = observe(&mut state, "1", "https://fixture.invalid/cover.png?token=second&size=480", online(), now);
    assert_ne!(first.token.url_hash, replacement.token.url_hash);
    assert!(replacement.url_changed);
    assert!(!replacement.blocked);
    assert!(!state.current(&first.token));
    state.failed(&first.token, NegativeReason::Download, now);
    assert!(state.entries["1"].negative.is_none());
}

#[test]
fn relative_and_published_absolute_paths_preserve_the_same_managed_source_identity() {
    let url = "https://fixture.invalid/cover.png?token=first";
    let relative = cached_cover_path(Path::new("relative-fixture"), url);
    let absolute = cached_cover_path(&std::env::temp_dir().join("absolute-fixture"), url);
    assert!(same_cover_source(&absolute, &relative, false));
    assert!(!same_cover_source(&absolute, &relative, true));
    let changed = cached_cover_path(Path::new("relative-fixture"), "https://fixture.invalid/cover.png?token=second");
    assert!(!same_cover_source(&absolute, &changed, false));
}

#[test]
fn reconnect_and_account_change_release_negative_state_without_reusing_old_tokens() {
    let mut state = CoverState::default();
    let now = Instant::now();
    let first = observe(&mut state, "1", "https://fixture.invalid/cover.png", offline(), now);
    state.failed(&first.token, NegativeReason::Offline, now);
    let reconnected = observe(&mut state, "1", "https://fixture.invalid/cover.png", online(), now);
    assert!(!reconnected.blocked);
    assert!(state.begin(&reconnected.token));
    let other = state.observe("other-fixture-account", "1", "https://fixture.invalid/cover.png", online(), now).unwrap();
    assert!(other.account_changed);
    assert!(!other.blocked);
    assert!(!state.current(&reconnected.token));
    state.clear();
    let signed_in_again = state.observe("other-fixture-account", "1", "https://fixture.invalid/cover.png", online(), now).unwrap();
    assert_ne!(signed_in_again.token, other.token);
    assert!(!state.current(&other.token));
}

#[test]
fn connectivity_update_does_not_discard_an_inflight_success_token() {
    let mut state = CoverState::default();
    let now = Instant::now();
    let disconnected = Connectivity { online: false, offline: false, demo: false };
    let first = observe(&mut state, "1", "https://fixture.invalid/cover.png", disconnected, now);
    assert!(state.begin(&first.token));
    let updated = observe(&mut state, "1", "https://fixture.invalid/cover.png", online(), now);
    assert!(updated.blocked);
    assert_eq!(updated.token, first.token);
    assert!(state.current(&first.token));
}

#[test]
fn a_published_object_releases_negative_entries_sharing_its_full_url() {
    let mut state = CoverState::default();
    let now = Instant::now();
    let url = "https://fixture.invalid/shared-cover.png?signed=fixture";
    let first = observe(&mut state, "1", url, offline(), now);
    let second = observe(&mut state, "2", url, offline(), now);
    state.failed(&first.token, NegativeReason::Offline, now);
    state.failed(&second.token, NegativeReason::Offline, now);
    state.published("1", Some(&format!("{}.png", first.token.url_hash)), now);
    assert!(!observe(&mut state, "1", url, offline(), now).blocked);
    assert!(!observe(&mut state, "2", url, offline(), now).blocked);
}

#[test]
fn transient_failures_have_a_bounded_backoff_and_can_recover() {
    let mut state = CoverState::default();
    let mut now = Instant::now();
    for _ in 0..20 {
        let current = observe(&mut state, "1", "https://fixture.invalid/cover.png", online(), now);
        assert!(!current.blocked);
        assert!(state.begin(&current.token));
        state.failed(&current.token, NegativeReason::Download, now);
        let negative = state.entries["1"].negative.as_ref().unwrap();
        let wait = negative.retry_at.duration_since(now);
        assert!(wait >= TRANSIENT_RETRY && wait <= MAX_TRANSIENT_RETRY);
        now = negative.retry_at;
    }
    let current = observe(&mut state, "1", "https://fixture.invalid/cover.png", online(), now);
    state.published("1", Some(&format!("{}.png", current.token.url_hash)), now);
    assert_eq!(state.entries["1"].failures, 0);
    assert!(state.entries["1"].negative.is_none());
}

#[test]
fn scheduler_capacity_deferral_is_short_and_does_not_inflate_failure_backoff() {
    let mut state = CoverState::default();
    let mut now = Instant::now();
    for _ in 0..20 {
        let current = observe(&mut state, "1", "https://fixture.invalid/cover.png", online(), now);
        assert!(state.begin(&current.token));
        state.failed(&current.token, NegativeReason::Deferred, now);
        assert_eq!(state.entries["1"].failures, 0);
        let retry_at = state.entries["1"].negative.as_ref().unwrap().retry_at;
        assert_eq!(retry_at.duration_since(now), MAINTENANCE_NEXT_BATCH);
        now = retry_at;
    }
}

#[test]
fn metadata_and_touch_records_are_bounded_and_keep_recently_used_entries() {
    let mut state = CoverState::default();
    let now = Instant::now();
    for index in 0..COVER_METADATA_LIMIT {
        observe(&mut state, &index.to_string(), &format!("https://fixture.invalid/{index}.png"), online(), now);
    }
    observe(&mut state, "0", "https://fixture.invalid/0.png", online(), now);
    let next = observe(&mut state, "overflow", "https://fixture.invalid/overflow.png", online(), now);
    assert_eq!(next.evicted, ["1"]);
    assert_eq!(state.entries.len(), COVER_METADATA_LIMIT);
    assert!(state.entries.contains_key("0"));
    for index in 0..COVER_METADATA_LIMIT * 3 {
        let current = observe(&mut state, &format!("next-{index}"), &format!("https://fixture.invalid/next-{index}.png"), online(), now);
        state.touch(&current.token.id, &format!("{}.png", current.token.url_hash), now);
        assert!(state.entries.len() <= COVER_METADATA_LIMIT);
        assert!(state.touches.len() <= COVER_METADATA_LIMIT);
    }
}

#[test]
fn bounded_metadata_never_evicts_an_inflight_entry_to_admit_more_work() {
    let mut state = CoverState::default();
    let now = Instant::now();
    let mut first = None;
    for index in 0..COVER_METADATA_LIMIT {
        let current = observe(&mut state, &index.to_string(), &format!("https://fixture.invalid/{index}.png"), online(), now);
        if first.is_none() { first = Some(current.token.clone()); }
        assert!(state.begin(&current.token));
    }
    assert!(state.observe("fixture-account", "overflow", "https://fixture.invalid/overflow.png", online(), now).is_none());
    let first = first.unwrap();
    assert!(state.current(&first));
    state.local_finished(&first);
    let admitted = observe(&mut state, "overflow", "https://fixture.invalid/overflow.png", online(), now);
    assert_eq!(admitted.evicted, ["0"]);
    assert_eq!(state.entries.len(), COVER_METADATA_LIMIT);
}

#[test]
fn maintenance_is_gated_and_flushes_bounded_touches_in_background_batches() {
    let mut state = CoverState::default();
    let now = Instant::now();
    let current = observe(&mut state, "1", "https://fixture.invalid/cover.png", online(), now);
    assert!(state.begin(&current.token));
    let name = format!("{}.png", current.token.url_hash);
    state.touch("1", &name, now);
    let request = state.maintenance_request(false, now).unwrap();
    assert!(request.protected.contains(&name));
    assert!(request.touches.contains_key(&name));
    assert!(state.maintenance_request(true, now).is_none());
    state.maintenance_finished(true, now);
    assert!(state.maintenance_request(false, now).is_none());
    assert!(state.maintenance_request(false, now + MAINTENANCE_NEXT_BATCH).is_some());
    state.maintenance_finished(false, now);
    assert!(state.maintenance_request(false, now + MAINTENANCE_INTERVAL - Duration::from_millis(1)).is_none());
    assert!(state.maintenance_request(false, now + MAINTENANCE_INTERVAL).is_some());
}

#[test]
fn retry_deadline_wakes_a_stationary_view_without_waiting_for_maintenance() {
    let mut state = CoverState::default();
    let now = Instant::now();
    let current = observe(&mut state, "1", "https://fixture.invalid/cover.png", offline(), now);
    state.failed(&current.token, NegativeReason::Offline, now);
    assert!(!state.take_due_retries(now + LOCAL_RECHECK - Duration::from_millis(1)));
    assert!(state.take_due_retries(now + LOCAL_RECHECK));
    assert!(!state.take_due_retries(now + LOCAL_RECHECK));
    assert!(state.entries["1"].negative.is_none());
    assert!(state.next_retry.is_none());
    assert!(state.maintenance_due.is_none());
    assert!(state.begin(&current.token));
}

#[test]
fn a_connection_event_releases_a_stationary_views_negative_cache_immediately() {
    let mut state = CoverState::default();
    let now = Instant::now();
    let current = observe(&mut state, "1", "https://fixture.invalid/cover.png", offline(), now);
    state.failed(&current.token, NegativeReason::Offline, now);
    assert!(state.update_connectivity(online()));
    assert!(state.entries["1"].negative.is_none());
    assert!(state.next_retry.is_none());
    assert!(!state.update_connectivity(online()));
    assert!(state.begin(&current.token));
}

#[test]
fn successful_local_publication_recovers_a_missing_offline_cover_immediately() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let mut state = CoverState::default();
    let now = Instant::now();
    let current = observe(&mut state, "1", "https://fixture.invalid/cover.png?token=exact", offline(), now);
    let name = format!("{}.png", current.token.url_hash);
    assert!(state.begin(&current.token));
    assert!(disk::read_cover(directory.path(), &name)?.is_none());
    state.local_finished(&current.token);
    state.failed(&current.token, NegativeReason::Offline, now);
    assert!(observe(&mut state, "1", "https://fixture.invalid/cover.png?token=exact", offline(), now).blocked);
    let rgba = [20, 30, 40, 255].repeat(4);
    disk::publish_cover(directory.path(), &name, &rgba, 2, 2)?;
    state.published("1", Some(&name), now);
    let recovered = observe(&mut state, "1", "https://fixture.invalid/cover.png?token=exact", offline(), now);
    assert!(!recovered.blocked);
    let (decoded, width, height) = disk::read_cover(directory.path(), &name)?.expect("Published cover");
    assert_eq!((width, height), (2, 2));
    assert_eq!(decoded, rgba);
    Ok(())
}
