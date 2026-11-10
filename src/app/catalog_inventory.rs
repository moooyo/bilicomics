//! Incremental local inventory work; never runs on the application thread.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use super::{LocalEpisode, inspect_local_page};
use crate::domain::ImageIndex;
use crate::store::{PageReceipt, Store};

const PAGES_PER_BATCH: usize = 4;
const METADATA_STEPS_PER_BATCH: usize = 16;
const BATCH_TIME_BUDGET: Duration = Duration::from_millis(25);

pub(super) struct InventoryScan {
    comics: VecDeque<String>,
    pending: VecDeque<(String, Option<String>)>,
    discovered: BTreeSet<String>,
    skip: BTreeSet<String>,
    priority_episode: Option<String>,
    downloads_pending: bool,
    current: Option<EpisodeScan>,
    store: Option<Store>,
}

struct EpisodeScan {
    comic_id: Option<String>,
    index: ImageIndex,
    receipts: BTreeMap<String, PageReceipt>,
    next_page: usize,
    local: LocalEpisode,
}

pub(super) struct CompletedEpisode {
    pub(super) comic_id: Option<String>,
    pub(super) episode_id: String,
    pub(super) local: LocalEpisode,
}

pub(super) struct InventoryBatch {
    pub(super) completed: Vec<CompletedEpisode>,
    pub(super) continuation: Option<InventoryScan>,
    pub(super) complete_targets: Option<BTreeSet<String>>,
    #[cfg(test)]
    pub(super) inspected_pages: usize,
}

impl InventoryScan {
    pub(super) fn new(mut comics: Vec<String>, skip: BTreeSet<String>, priority: Option<(String, String)>) -> Self {
        if let Some((comic_id, _)) = &priority {
            comics.retain(|id| id != comic_id);
            comics.insert(0, comic_id.clone());
        }
        Self { comics: comics.into(), pending: VecDeque::new(), discovered: BTreeSet::new(), skip,
            priority_episode: priority.map(|(_, episode)| episode), downloads_pending: true, current: None, store: None }
    }

    pub(super) fn run(mut self, database: PathBuf, account: &str) -> Result<InventoryBatch> {
        // The continuation moves one exclusively owned connection between
        // workers. No statement or concurrent borrow crosses the boundary.
        let store = match self.store.take() { Some(store) => store, None => Store::open(database)? };
        let mut batch = self.advance(&store, account)?;
        if let Some(continuation) = &mut batch.continuation { continuation.store = Some(store); }
        Ok(batch)
    }

    fn enqueue(&mut self, episode_id: String, comic_id: Option<String>) {
        if !self.discovered.insert(episode_id.clone()) || self.skip.contains(&episode_id) { return; }
        if self.priority_episode.as_ref() == Some(&episode_id) {
            self.pending.push_front((episode_id, comic_id));
        } else {
            self.pending.push_back((episode_id, comic_id));
        }
    }

    /// A chapter is published only after all of its descriptors were checked.
    /// The budget bounds the number of large file reads, not an individual
    /// kernel read's duration. Exact-content validation remains authoritative.
    pub(super) fn advance(mut self, store: &Store, account: &str) -> Result<InventoryBatch> {
        let pages = store.path().parent().context("Resolve the local inventory directory")?
            .join("accounts").join(super::hex_digest(account.as_bytes())).join("pages");
        let started = Instant::now();
        let mut completed = Vec::new();
        let mut inspected_pages = 0;
        let mut metadata_steps = 0;
        loop {
            if inspected_pages >= PAGES_PER_BATCH || metadata_steps >= METADATA_STEPS_PER_BATCH
                || inspected_pages + metadata_steps > 0 && started.elapsed() >= BATCH_TIME_BUDGET
            { break; }
            if let Some(current) = &mut self.current {
                if let Some(page) = current.index.images.get(current.next_page) {
                    let valid = inspect_local_page(store, account, &pages, page, &current.receipts)?;
                    if let Some(receipt) = valid {
                        current.local.present += 1;
                        current.local.bytes = current.local.bytes.saturating_add(receipt.bytes);
                        current.local.retained &= receipt.retained;
                    } else { current.local.retained = false; }
                    current.next_page += 1;
                    inspected_pages += 1;
                }
                if current.next_page == current.index.images.len() {
                    let current = self.current.take().expect("The current inventory chapter exists");
                    completed.push(CompletedEpisode { comic_id: current.comic_id,
                        episode_id: current.index.episode_id, local: current.local });
                }
                continue;
            }
            if let Some((episode_id, comic_id)) = self.pending.pop_front() {
                metadata_steps += 1;
                let Some(index) = store.load_index(account, &episode_id, None)? else {
                    completed.push(CompletedEpisode { comic_id, episode_id,
                        local: LocalEpisode { known: true, ..Default::default() } });
                    continue;
                };
                let receipts = store.page_receipts(account, &episode_id, &index.revision)?
                    .into_iter().map(|receipt| (receipt.page_id.clone(), receipt)).collect();
                let local = LocalEpisode { known: true, revision: index.revision.clone(), total: index.images.len(),
                    retained: !index.images.is_empty(), ..Default::default() };
                self.current = Some(EpisodeScan { comic_id, index, receipts, next_page: 0, local });
                continue;
            }
            if let Some(comic_id) = self.comics.pop_front() {
                metadata_steps += 1;
                if let Some(detail) = store.load_detail(account, &comic_id)? {
                    for episode in detail.episodes { self.enqueue(episode.id, Some(comic_id.clone())); }
                }
                continue;
            }
            if self.downloads_pending {
                self.downloads_pending = false;
                metadata_steps += 1;
                for download in store.downloads(account)? {
                    self.enqueue(download.episode_id, Some(download.comic_id));
                }
                continue;
            }
            break;
        }
        let unfinished = self.current.is_some() || !self.pending.is_empty()
            || !self.comics.is_empty() || self.downloads_pending;
        let complete_targets = if unfinished { None } else { Some(std::mem::take(&mut self.discovered)) };
        Ok(InventoryBatch { completed, continuation: unfinished.then_some(self), complete_targets,
            #[cfg(test)] inspected_pages })
    }
}
