//! Stable application data shared by source adapters, persistence, and presentation.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Comic {
    pub id: String,
    pub title: String,
    pub author: String,
    pub cover_url: String,
    pub description: String,
    pub genre: String,
    pub latest_episode_id: Option<String>,
    pub latest_episode_title: String,
    pub latest_order: f64,
    pub chapter_count: u32,
    pub progress: u32,
    pub favorite: bool,
    pub finished: bool,
    pub updated_at: i64,
    pub last_episode_id: Option<String>,
    pub last_read_at: i64,
    pub extra: Value,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EpisodeAccess {
    Free,
    Owned,
    Temporary,
    Locked,
    Unavailable,
    #[default]
    Unknown,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Episode {
    pub id: String,
    pub comic_id: String,
    pub title: String,
    pub short_title: String,
    pub order: f64,
    pub access: EpisodeAccess,
    pub expires_at: Option<i64>,
    pub pay_gold: Option<f64>,
    pub read: bool,
    pub extra: Value,
}

impl Episode {
    pub fn readable_at(&self, timestamp: i64) -> bool {
        matches!(self.access, EpisodeAccess::Free | EpisodeAccess::Owned | EpisodeAccess::Temporary)
            && self.expires_at.is_none_or(|expires| expires <= 0 || expires > timestamp)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ComicDetail {
    pub comic: Comic,
    pub episodes: Vec<Episode>,
    pub extra: Value,
}

/// A persistent page identity contains no signed URL or acquisition credential.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct PageDescriptor {
    pub id: String,
    pub index: usize,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ImageIndex {
    pub episode_id: String,
    pub revision: String,
    pub images: Vec<PageDescriptor>,
    /// Acquisition paths are transient; restart uses the stable descriptor and a fresh index.
    #[serde(skip)]
    pub paths: Vec<String>,
    pub extra: Value,
}

impl std::fmt::Debug for ImageIndex {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("ImageIndex").field("episode_id", &self.episode_id)
            .field("revision", &self.revision).field("images", &self.images)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReadingMode {
    #[default]
    Page,
    Panel,
    Strip,
    /// Retained for older source anchors; new vertical-strip reading uses Strip.
    Continuous,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ReadingAnchor {
    pub episode_id: String,
    pub revision: String,
    pub page_id: String,
    /// Zero-based page position in the application's ordered descriptor.
    pub page_index: usize,
    /// Normalized coordinates in the original source image.
    pub source_x: f64,
    pub source_y: f64,
    pub zoom_ratio: f64,
    pub rotation: u16,
    pub mode: ReadingMode,
    pub finished: bool,
    pub updated_at: i64,
}
