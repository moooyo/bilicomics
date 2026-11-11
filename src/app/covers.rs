//! Bounded cover caching, retry suppression, and background-only disk maintenance.

mod disk;

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime},
};

use super::{App, CachedCover, COVER_IMAGE_BUDGET, hex_digest, startup};
use crate::domain::Comic;

pub(super) const COVER_METADATA_LIMIT: usize = 512;
const LOCAL_RECHECK: Duration = Duration::from_secs(60);
const TRANSIENT_RETRY: Duration = Duration::from_secs(30);
const MAX_TRANSIENT_RETRY: Duration = Duration::from_secs(5 * 60);
const TOUCH_INTERVAL: Duration = Duration::from_secs(60);
const MAINTENANCE_INTERVAL: Duration = Duration::from_secs(5 * 60);
const MAINTENANCE_NEXT_BATCH: Duration = Duration::from_secs(1);

pub(super) fn cached_cover_path(data_dir: &Path, url: &str) -> PathBuf {
    data_dir.join("covers").join(format!("{}.png", hex_digest(url.as_bytes())))
}

pub(super) fn read_cached_cover(data_dir: &Path, url: &str) -> anyhow::Result<Option<(Vec<u8>, u32, u32)>> {
    disk::read_cover(data_dir, &format!("{}.png", hex_digest(url.as_bytes())))
}

pub(super) fn read_sleep_cover(data_dir: &Path, name: &str) -> anyhow::Result<Option<Vec<u8>>> {
    disk::read_cover_bytes(data_dir, name)
}

pub(super) fn publish_sleep_cover(data_dir: &Path, name: &str, luma: &[u8], width: u32, height: u32) -> anyhow::Result<PathBuf> {
    disk::publish_sleep_cover(data_dir, name, luma, width, height)
}

fn same_cover_source(known: &Path, expected: &Path, demo: bool) -> bool {
    if demo { return known == expected; }
    // Managed filenames contain the full URL hash; relative and absolute roots are equivalent.
    known.file_name().zip(expected.file_name()).is_some_and(|(known, expected)| known == expected)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Connectivity {
    online: bool,
    offline: bool,
    demo: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NegativeReason {
    Offline,
    EmptyUrl,
    DemoMissing,
    Deferred,
    Download,
    Storage,
}

#[derive(Clone, Debug)]
struct NegativeCover {
    reason: NegativeReason,
    retry_at: Instant,
}

impl NegativeCover {
    fn deadline(&self) -> Option<Instant> {
        // A missing URL or fixture mapping needs an identity/context change, not repeated I/O.
        (!matches!(self.reason, NegativeReason::EmptyUrl | NegativeReason::DemoMissing)).then_some(self.retry_at)
    }

    fn due(&self, now: Instant) -> bool { self.deadline().is_some_and(|deadline| now >= deadline) }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CoverToken {
    account: String,
    id: String,
    url_hash: String,
    generation: u64,
}

#[derive(Debug)]
struct CoverEntry {
    token: CoverToken,
    negative: Option<NegativeCover>,
    pending: bool,
    failures: u8,
    used: u64,
    last_touch: Option<Instant>,
}

struct Observation {
    token: CoverToken,
    account_changed: bool,
    url_changed: bool,
    evicted: Vec<String>,
    blocked: bool,
}

#[derive(Default)]
pub(super) struct CoverState {
    account: Option<String>,
    connectivity: Option<Connectivity>,
    entries: BTreeMap<String, CoverEntry>,
    touches: BTreeMap<String, SystemTime>,
    generation: u64,
    used: u64,
    next_retry: Option<Instant>,
    maintenance_pending: bool,
    maintenance_due: Option<Instant>,
}

impl CoverState {
    pub(super) fn clear(&mut self) {
        // Do not reuse tokens when an account is cleared and immediately signs in again.
        let generation = self.generation.wrapping_add(1);
        *self = Self { generation, ..Self::default() };
    }

    fn update_connectivity(&mut self, connectivity: Connectivity) -> bool {
        if self.connectivity == Some(connectivity) { return false; }
        let mut released = false;
        for entry in self.entries.values_mut() {
            released |= entry.negative.take().is_some();
            entry.failures = 0;
        }
        self.connectivity = Some(connectivity);
        self.next_retry = None;
        released
    }

    fn observe(&mut self, account: &str, id: &str, url: &str, connectivity: Connectivity,
        now: Instant) -> Option<Observation>
    {
        let account_changed = self.account.as_deref().is_some_and(|previous| previous != account);
        if account_changed {
            self.entries.clear();
            self.touches.clear();
            self.next_retry = None;
        }
        self.account = Some(account.to_owned());
        self.update_connectivity(connectivity);
        self.used = self.used.wrapping_add(1);
        let url_hash = hex_digest(url.as_bytes());
        let url_changed = self.entries.get(id).is_some_and(|entry| entry.token.url_hash != url_hash);
        let mut evicted = Vec::new();
        if !self.entries.contains_key(id) {
            while self.entries.len() >= COVER_METADATA_LIMIT {
                let oldest = self.entries.iter().filter(|(_, entry)| !entry.pending)
                    .min_by_key(|(_, entry)| entry.used).map(|(id, _)| id.clone());
                let Some(oldest) = oldest else { return None; };
                self.entries.remove(&oldest);
                evicted.push(oldest);
            }
        }
        if !self.entries.contains_key(id) || url_changed {
            self.generation = self.generation.wrapping_add(1);
            self.entries.insert(id.to_owned(), CoverEntry {
                token: CoverToken { account: account.to_owned(), id: id.to_owned(), url_hash,
                    generation: self.generation },
                negative: None, pending: false, failures: 0, used: self.used, last_touch: None,
            });
        }
        let entry = self.entries.get_mut(id)?;
        entry.used = self.used;
        if entry.negative.as_ref().is_some_and(|negative| negative.due(now)) {
            entry.negative = None;
        }
        Some(Observation { token: entry.token.clone(), account_changed, url_changed, evicted,
            blocked: entry.pending || entry.negative.is_some() })
    }

    fn current(&self, token: &CoverToken) -> bool {
        self.account.as_deref() == Some(token.account.as_str())
            && self.entries.get(&token.id).is_some_and(|entry| &entry.token == token)
    }

    fn begin(&mut self, token: &CoverToken) -> bool {
        let Some(entry) = self.entries.get_mut(&token.id).filter(|entry| &entry.token == token) else { return false; };
        if entry.pending || entry.negative.is_some() { return false; }
        entry.pending = true;
        true
    }

    fn local_finished(&mut self, token: &CoverToken) {
        if let Some(entry) = self.entries.get_mut(&token.id).filter(|entry| &entry.token == token) {
            entry.pending = false;
        }
    }

    fn failed(&mut self, token: &CoverToken, reason: NegativeReason, now: Instant) {
        let Some(entry) = self.entries.get_mut(&token.id).filter(|entry| &entry.token == token) else { return; };
        entry.pending = false;
        if reason != NegativeReason::Deferred { entry.failures = entry.failures.saturating_add(1); }
        let delay = match reason {
            NegativeReason::Offline | NegativeReason::EmptyUrl | NegativeReason::DemoMissing => LOCAL_RECHECK,
            NegativeReason::Deferred => MAINTENANCE_NEXT_BATCH,
            NegativeReason::Download | NegativeReason::Storage => {
                TRANSIENT_RETRY.saturating_mul(1_u32 << entry.failures.saturating_sub(1).min(4))
                    .min(MAX_TRANSIENT_RETRY)
            }
        };
        let retry_at = now + delay;
        entry.negative = Some(NegativeCover { reason, retry_at });
        if entry.negative.as_ref().is_some_and(|negative| negative.deadline().is_some()) {
            self.next_retry = Some(self.next_retry.map_or(retry_at, |earliest| earliest.min(retry_at)));
        }
    }

    fn published(&mut self, id: &str, file_name: Option<&str>, now: Instant) {
        let published_hash = file_name.and_then(|name| name.strip_suffix(".png"));
        for (entry_id, entry) in &mut self.entries {
            if entry_id == id || published_hash == Some(entry.token.url_hash.as_str()) {
                entry.negative = None;
                entry.failures = 0;
                if entry_id == id { entry.pending = false; }
            }
        }
        self.next_retry = self.entries.values().filter_map(|entry| entry.negative.as_ref().and_then(NegativeCover::deadline)).min();
        if let Some(name) = file_name { self.touch(id, name, now); }
    }

    fn take_due_retries(&mut self, now: Instant) -> bool {
        if self.next_retry.is_none_or(|deadline| now < deadline) { return false; }
        let mut due = false;
        for entry in self.entries.values_mut() {
            if entry.negative.as_ref().is_some_and(|negative| negative.due(now)) {
                entry.negative = None;
                due = true;
            }
        }
        self.next_retry = self.entries.values().filter_map(|entry| entry.negative.as_ref().and_then(NegativeCover::deadline)).min();
        due
    }

    fn touch(&mut self, id: &str, file_name: &str, now: Instant) {
        let Some(entry) = self.entries.get_mut(id) else { return; };
        if entry.last_touch.is_some_and(|last| now.saturating_duration_since(last) < TOUCH_INTERVAL) { return; }
        entry.last_touch = Some(now);
        self.queue_touch(file_name, SystemTime::now());
    }

    fn queue_touch(&mut self, file_name: &str, touched: SystemTime) {
        if let Some(previous) = self.touches.get_mut(file_name) {
            *previous = (*previous).max(touched);
            return;
        }
        if !self.touches.contains_key(file_name) && self.touches.len() >= COVER_METADATA_LIMIT {
            let oldest = self.touches.iter().min_by_key(|(_, touched)| **touched).map(|(name, _)| name.clone());
            if let Some(oldest) = oldest { self.touches.remove(&oldest); }
        }
        self.touches.insert(file_name.to_owned(), touched);
    }

    fn maintenance_request(&mut self, force: bool, now: Instant) -> Option<disk::MaintenanceRequest> {
        if self.maintenance_pending || !force && self.maintenance_due.is_some_and(|due| now < due) { return None; }
        self.maintenance_pending = true;
        let protected = self.entries.values().filter(|entry| entry.pending)
            .map(|entry| format!("{}.png", entry.token.url_hash)).collect();
        Some(disk::MaintenanceRequest { touches: std::mem::take(&mut self.touches), protected })
    }

    fn maintenance_finished(&mut self, needs_more: bool, now: Instant) {
        self.maintenance_pending = false;
        self.maintenance_due = Some(now + if needs_more { MAINTENANCE_NEXT_BATCH } else { MAINTENANCE_INTERVAL });
    }
}

impl App {
    /// The caller marks cover-bearing views dirty after a retry deadline or connectivity change.
    pub(super) fn cover_retry_due(&mut self) -> bool {
        if self.is_suspended() { return false; }
        let connectivity = Connectivity { online: self.online, offline: self.options.offline, demo: self.demo.is_some() };
        self.cover_state.update_connectivity(connectivity)
            || self.cover_state.take_due_retries(Instant::now())
    }

    fn observe_cover(&mut self, comic: &Comic) -> Option<Observation> {
        let connectivity = Connectivity { online: self.online, offline: self.options.offline, demo: self.demo.is_some() };
        let observation = self.cover_state.observe(&self.account, &comic.id, &comic.cover_url, connectivity, Instant::now())?;
        if observation.account_changed {
            self.cover_files.clear();
            self.cover_images.clear();
            self.cover_pending.clear();
            self.cover_failed.clear();
        }
        if observation.url_changed {
            self.cover_files.remove(&comic.id);
            self.cover_images.remove(&(self.account.clone(), comic.id.clone()));
            self.cover_pending.remove(&comic.id);
        }
        for id in &observation.evicted {
            self.cover_files.remove(id);
            self.cover_images.remove(&(self.account.clone(), id.clone()));
            self.cover_pending.remove(id);
            self.cover_failed.remove(id);
        }
        // Older startup bookkeeping must never become a permanent failure gate.
        self.cover_failed.retain(|id| self.cover_state.entries.contains_key(id));
        self.cover_failed.remove(&comic.id);
        Some(observation)
    }

    pub(super) fn cover_image(&mut self, comic: &Comic) -> slint::Image {
        let Some(observation) = self.observe_cover(comic) else { return slint::Image::default(); };
        let token = observation.token;
        let file_name = format!("{}.png", token.url_hash);
        let demo_path = self.demo.as_ref().and_then(|demo| demo.covers.get(&comic.id)).cloned();
        if self.demo.is_some() && demo_path.is_none() {
            if !observation.blocked { self.cover_state.failed(&token, NegativeReason::DemoMissing, Instant::now()); }
            return slint::Image::default();
        }
        let path = demo_path.unwrap_or_else(|| cached_cover_path(&self.data_dir, &comic.cover_url));
        let key = (self.account.clone(), comic.id.clone());
        if self.cover_files.get(&comic.id).is_some_and(|known| !same_cover_source(known, &path, self.demo.is_some())) {
            self.cover_files.remove(&comic.id);
            self.cover_images.remove(&key);
        }
        self.cover_sequence = self.cover_sequence.wrapping_add(1);
        if let Some(cached) = self.cover_images.get_mut(&key) {
            cached.used = self.cover_sequence;
            let image = cached.image.clone();
            if self.demo.is_none() { self.cover_state.touch(&comic.id, &file_name, Instant::now()); }
            return image;
        }
        if self.is_suspended() || observation.blocked || !self.cover_state.begin(&token) {
            return slint::Image::default();
        }
        self.cover_pending.insert(comic.id.clone());
        let candidate = path.clone();
        let data_dir = self.data_dir.clone();
        let is_demo = self.demo.is_some();
        let comic = comic.clone();
        self.cover_background(&format!("cover-local:{}", comic.id), move || {
            if is_demo { startup::decode_cover(&candidate) } else { disk::read_cover(&data_dir, &file_name) }
        }, move |app, result| {
            if !app.cover_state.current(&token) { return; }
            app.cover_state.local_finished(&token);
            app.cover_pending.remove(&comic.id);
            match result {
                Ok(Some((rgba, width, height))) => app.remember_cover(comic.id, path, rgba, width, height),
                Err(error) if error.downcast_ref::<super::jobs::CapacityError>().is_some() => {
                    app.cover_state.failed(&token, NegativeReason::Deferred, Instant::now());
                }
                Ok(None) | Err(_) => app.request_cover(comic, path),
            }
        });
        slint::Image::default()
    }

    pub(super) fn remember_cover(&mut self, id: String, path: PathBuf, rgba: Vec<u8>, width: u32, height: u32) {
        let expected = width.checked_mul(height).and_then(|pixels| pixels.checked_mul(4));
        if width == 0 || height == 0 || expected != u32::try_from(rgba.len()).ok() || rgba.len() > COVER_IMAGE_BUDGET {
            self.cover_pending.remove(&id);
            if let Some(token) = self.cover_state.entries.get(&id).map(|entry| entry.token.clone()) {
                self.cover_state.failed(&token, NegativeReason::Storage, Instant::now());
            }
            return;
        }
        self.cover_sequence = self.cover_sequence.wrapping_add(1);
        let bytes = rgba.len();
        let pixels = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&rgba, width, height);
        let key = (self.account.clone(), id.clone());
        self.cover_state.published(&id, if self.demo.is_none() { path.file_name().and_then(|name| name.to_str()) } else { None }, Instant::now());
        self.cover_failed.remove(&id);
        self.cover_pending.remove(&id);
        self.cover_files.insert(id, path);
        self.cover_images.insert(key, CachedCover {
            image: slint::Image::from_rgba8(pixels), bytes, used: self.cover_sequence,
        });
        while self.cover_images.len() > COVER_METADATA_LIMIT
            || self.cover_images.values().map(|cover| cover.bytes).sum::<usize>() > COVER_IMAGE_BUDGET
        {
            let oldest = self.cover_images.iter().min_by_key(|(_, cover)| cover.used).map(|(key, _)| key.clone());
            let Some(oldest) = oldest else { break; };
            self.cover_images.remove(&oldest);
        }
        while self.cover_files.len() > COVER_METADATA_LIMIT {
            let oldest = self.cover_files.keys().filter(|id| !self.cover_pending.contains(*id))
                .filter(|id| !self.cover_images.contains_key(&(self.account.clone(), (*id).clone())))
                .min_by_key(|id| self.cover_state.entries.get(*id).map_or(0, |entry| entry.used)).cloned();
            let Some(oldest) = oldest else { break; };
            self.cover_files.remove(&oldest);
        }
    }

    pub(super) fn request_cover(&mut self, comic: Comic, _path: PathBuf) {
        if self.is_suspended() { return; }
        let Some(observation) = self.observe_cover(&comic) else { return; };
        let token = observation.token;
        if observation.blocked { return; }
        let unavailable = if self.demo.is_some() { Some(NegativeReason::DemoMissing) }
            else if self.options.offline { Some(NegativeReason::Offline) }
            else if comic.cover_url.trim().is_empty() { Some(NegativeReason::EmptyUrl) }
            else { None };
        if let Some(reason) = unavailable {
            self.cover_pending.remove(&comic.id);
            self.cover_state.failed(&token, reason, Instant::now());
            return;
        }
        if !self.cover_state.begin(&token) { return; }
        self.cover_pending.insert(comic.id.clone());
        let client = self.client.clone();
        let id = comic.id;
        let url = comic.cover_url;
        self.source_job(&format!("cover:{id}"), async move { client.download_cover(&url).await }, move |app, result| {
            if !app.cover_state.current(&token) { return; }
            match result {
                Ok(image) => {
                    let data_dir = app.data_dir.clone();
                    let file_name = format!("{}.png", token.url_hash);
                    let id_for_work = id.clone();
                    app.cover_background(&format!("cover-prepare:{id}"), move || {
                        let (rgba, width, height) = startup::decode_cover_bytes(image.bytes)?;
                        let path = disk::publish_cover(&data_dir, &file_name, &rgba, width, height)?;
                        Ok((id_for_work, path, rgba, width, height))
                    }, move |app, result| {
                        if !app.cover_state.current(&token) { return; }
                        app.cover_pending.remove(&id);
                        match result {
                            Ok((id, path, rgba, width, height)) => {
                                app.remember_cover(id, path, rgba, width, height);
                                app.schedule_cover_maintenance(false);
                            }
                            Err(error) => {
                                if error.downcast_ref::<super::jobs::CapacityError>().is_some() {
                                    app.cover_state.failed(&token, NegativeReason::Deferred, Instant::now());
                                } else {
                                    app.cover_state.failed(&token, NegativeReason::Storage, Instant::now());
                                    app.toast("common_storage_error");
                                }
                            }
                        }
                    });
                }
                Err(error) => {
                    app.cover_pending.remove(&id);
                    let reason = if error.code == "application_job_capacity_reached" {
                        NegativeReason::Deferred
                    } else { NegativeReason::Download };
                    app.cover_state.failed(&token, reason, Instant::now());
                }
            }
        });
    }

    /// Called after startup and from the normal tick; all filesystem work stays in a worker.
    pub(super) fn schedule_cover_maintenance(&mut self, force: bool) {
        if self.is_suspended() || self.demo.is_some() { return; }
        let Some(request) = self.cover_state.maintenance_request(force, Instant::now()) else { return; };
        let data_dir = self.data_dir.clone();
        let retry_touches = request.touches.clone();
        self.background("cover-maintenance", move || disk::maintain(&data_dir, request), move |app, result| {
            let deferred = result.as_ref().err().is_some_and(|error| error.downcast_ref::<super::jobs::CapacityError>().is_some());
            let needs_more = deferred || result.as_ref().is_ok_and(|result| result.needs_more);
            if result.is_err() {
                for (name, touched) in retry_touches { app.cover_state.queue_touch(&name, touched); }
            }
            app.cover_state.maintenance_finished(needs_more, Instant::now());
            if let Ok(result) = result {
                let removed: BTreeSet<_> = result.evicted.into_iter().collect();
                let resident: BTreeSet<_> = app.cover_images.keys().map(|(_, id)| id.clone()).collect();
                // Resident images retain their source identity even after their disk object is evicted.
                app.cover_files.retain(|id, path| resident.contains(id) || !path.file_name()
                    .and_then(|name| name.to_str()).is_some_and(|name| removed.contains(name)));
            }
        });
    }
}

#[cfg(test)]
mod tests;
