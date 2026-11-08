//! Transient, account-bound Bilibili page grants for the download scheduler.

use std::{collections::HashSet, sync::Arc, time::{Duration, Instant}};

use tokio::sync::Mutex;

use super::{AcquiredPage, AcquisitionFailure, AcquisitionFuture, AcquisitionSource, PreparedPageBatch};
use crate::{
    domain::{EpisodeAccess, ImageIndex},
    source::{AcquisitionBatch, BiliClient, SourceError, VerifiedImage},
};

const IMAGE_BATCH_SIZE: usize = 20;
// This local queue-age budget is not a claim about the service's token lifetime.
const IMAGE_BATCH_FRESHNESS_BUDGET: Duration = Duration::from_secs(60);

struct BatchPage {
    page_index: usize,
    width: u32,
    height: u32,
    path: String,
}

struct BatchState {
    batch: AcquisitionBatch,
    token_indices: Vec<Option<usize>>,
    completed: Vec<bool>,
    created_at: Instant,
    refreshes: usize,
    completed_at_refresh: usize,
    refresh_failure: Option<AcquisitionFailure>,
}

/// No acquisition material is serialized or included in a Debug implementation.
struct BiliPageBatch {
    client: BiliClient,
    pages: Vec<BatchPage>,
    state: Mutex<BatchState>,
}

impl BiliPageBatch {
    async fn prepare(client: &BiliClient, index: &ImageIndex, page_indices: &[usize])
        -> std::result::Result<Self, AcquisitionFailure>
    {
        if page_indices.is_empty() || page_indices.len() > IMAGE_BATCH_SIZE {
            return Err(AcquisitionFailure::new("invalid_image_path_batch", false));
        }
        let mut seen = HashSet::with_capacity(page_indices.len());
        let mut pages = Vec::with_capacity(page_indices.len());
        for &page_index in page_indices {
            if !seen.insert(page_index) {
                return Err(AcquisitionFailure::new("duplicate_page_index", false));
            }
            let descriptor = index.images.get(page_index)
                .ok_or_else(|| AcquisitionFailure::new("page_index_invalid", false))?;
            let path = index.paths.get(page_index)
                .ok_or_else(|| AcquisitionFailure::new("fresh_image_index_required", false))?;
            pages.push(BatchPage {
                page_index, width: descriptor.width, height: descriptor.height, path: path.clone(),
            });
        }
        let paths = pages.iter().map(|page| page.path.clone()).collect::<Vec<_>>();
        let batch = client.prepare_image_batch(&paths).await.map_err(source_failure)?;
        let state = BatchState {
            batch, token_indices: (0..pages.len()).map(Some).collect(),
            completed: vec![false; pages.len()], created_at: Instant::now(), refreshes: 0,
            completed_at_refresh: 0, refresh_failure: None,
        };
        // Cloning preserves the application's expected-account restriction and
        // shared account epoch without retaining the complete chapter index.
        Ok(Self { client: client.clone(), pages, state: Mutex::new(state) })
    }

    async fn grant(&self, page_position: usize)
        -> std::result::Result<(AcquisitionBatch, usize), AcquisitionFailure>
    {
        let mut state = self.state.lock().await;
        self.client.require_current_image_batch(&state.batch).map_err(source_failure)?;
        if state.completed[page_position] {
            return Err(AcquisitionFailure::new("page_batch_page_completed", false));
        }
        if state.created_at.elapsed() >= IMAGE_BATCH_FRESHNESS_BUDGET {
            if let Some(error) = &state.refresh_failure { return Err(error.clone()); }
            let completed_count = state.completed.iter().filter(|completed| **completed).count();
            if state.refreshes >= self.pages.len()
                || state.refreshes > 0 && completed_count <= state.completed_at_refresh {
                return Err(AcquisitionFailure::new("image_batch_expired", true));
            }
            let pending = state.completed.iter().enumerate()
                .filter_map(|(position, completed)| (!*completed).then_some(position)).collect::<Vec<_>>();
            let paths = pending.iter().map(|&position| self.pages[position].path.clone()).collect::<Vec<_>>();
            // Count before awaiting so cancellation cannot permit unbounded
            // refreshes. Further refreshes require a completed page since this
            // attempt, and their total cannot exceed this batch's page count.
            state.refreshes += 1;
            state.completed_at_refresh = completed_count;
            let batch = match self.client.prepare_image_batch(&paths).await.map_err(source_failure) {
                Ok(batch) => batch,
                Err(error) => { state.refresh_failure = Some(error.clone()); return Err(error); }
            };
            // A new grant must never revive the original batch after an
            // explicit session replacement, even for the same account.
            if let Err(error) = self.client.require_current_image_batch(&state.batch).map_err(source_failure) {
                state.refresh_failure = Some(error.clone());
                return Err(error);
            }
            let mut token_indices = vec![None; self.pages.len()];
            for (token_index, &position) in pending.iter().enumerate() {
                token_indices[position] = Some(token_index);
            }
            state.batch = batch;
            state.token_indices = token_indices;
            state.created_at = Instant::now();
        }
        let token_index = state.token_indices[page_position]
            .ok_or_else(|| AcquisitionFailure::new("page_index_invalid", false))?;
        // Every in-flight GET owns its grant and P-256 key. Refreshing state
        // never replaces a key that an earlier image conversion still needs.
        Ok((state.batch.clone(), token_index))
    }
}

impl PreparedPageBatch for BiliPageBatch {
    fn download_page(&self, page_index: usize) -> AcquisitionFuture<'_, AcquiredPage> {
        Box::pin(async move {
            let position = self.pages.iter().position(|page| page.page_index == page_index)
                .ok_or_else(|| AcquisitionFailure::new("page_index_invalid", false))?;
            let (batch, token_index) = self.grant(position).await?;
            let image = self.client.download_image(&batch, token_index).await.map_err(source_failure)?;
            self.client.require_current_image_batch(&batch).map_err(source_failure)?;
            let page = &self.pages[position];
            if image.width != page.width || image.height != page.height {
                return Err(AcquisitionFailure::new("image_geometry_changed", false));
            }
            self.state.lock().await.completed[position] = true;
            Ok(acquired_page(image))
        })
    }
}

impl AcquisitionSource for BiliClient {
    fn image_index<'a>(&'a self, episode_id: &'a str) -> AcquisitionFuture<'a, ImageIndex> {
        Box::pin(async move { BiliClient::image_index(self, episode_id).await.map_err(source_failure) })
    }

    fn image_index_for<'a>(&'a self, comic_id: &'a str, episode_id: &'a str) -> AcquisitionFuture<'a, ImageIndex> {
        Box::pin(async move {
            let (account, _) = self.credential_context()
                .ok_or_else(|| AcquisitionFailure::new("account_identity_unconfirmed", false))?;
            let client = self.bound_to_account(&account).map_err(source_failure)?;
            // Queue admission may precede a temporary grant's expiry or a
            // revocation. Check current access before requesting any image index.
            let detail = client.comic_detail(comic_id).await.map_err(source_failure)?;
            let episode = detail.episodes.iter().find(|episode| episode.id == episode_id
                && episode.comic_id == comic_id)
                .ok_or_else(|| AcquisitionFailure::new("episode_access_missing", false))?;
            if !episode.readable_at(chrono::Utc::now().timestamp()) {
                let code = match episode.access {
                    EpisodeAccess::Locked | EpisodeAccess::Temporary => "episode_locked",
                    EpisodeAccess::Unavailable => "episode_unavailable",
                    _ => "episode_access_unknown",
                };
                return Err(AcquisitionFailure::new(code, false));
            }
            BiliClient::image_index(&client, episode_id).await.map_err(source_failure)
        })
    }

    fn download_page<'a>(&'a self, index: &'a ImageIndex, page_index: usize)
        -> AcquisitionFuture<'a, AcquiredPage>
    {
        Box::pin(async move {
            BiliClient::download_page(self, index, page_index).await.map(acquired_page).map_err(source_failure)
        })
    }

    fn page_batch_size(&self) -> usize { IMAGE_BATCH_SIZE }

    fn prepare_page_batch<'a>(&'a self, index: &'a ImageIndex, page_indices: &'a [usize])
        -> AcquisitionFuture<'a, Arc<dyn PreparedPageBatch>>
    {
        Box::pin(async move {
            Ok(Arc::new(BiliPageBatch::prepare(self, index, page_indices).await?) as Arc<dyn PreparedPageBatch>)
        })
    }
}

fn acquired_page(image: VerifiedImage) -> AcquiredPage {
    AcquiredPage { bytes: image.bytes, width: image.width, height: image.height, sha256: Some(image.sha256) }
}

fn source_failure(error: SourceError) -> AcquisitionFailure {
    AcquisitionFailure { code: error.code, retryable: error.retryable }
}

#[cfg(all(test, target_os = "linux"))]
#[path = "bili_source_tests.rs"]
mod tests;
