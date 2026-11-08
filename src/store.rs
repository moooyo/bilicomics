//! Account-scoped SQLite metadata and durable page receipts.

use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::{
    domain::{Comic, ComicDetail, ImageIndex, ReadingAnchor},
    image_integrity::{DecodedImageProof, InvalidEncodingProof, VALIDATION_VERSION},
};

const SCHEMA_VERSION: i64 = 4;
const MAX_RECEIPT_BYTES: u64 = 128 * 1024 * 1024;
pub const PENDING_REVISION: &str = "__pending__";

/// A SQLite connection is owned by one application thread or one worker.
pub struct Store {
    path: PathBuf,
    connection: Connection,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DownloadStatus {
    #[default]
    Queued,
    Running,
    Paused,
    Failed,
    Completed,
    Cancelled,
}

impl DownloadStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Failed => "failed",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct DownloadRecord {
    pub id: String,
    pub comic_id: String,
    pub episode_id: String,
    pub revision: String,
    pub title: String,
    pub comic_title: String,
    pub status: DownloadStatus,
    pub total_pages: u32,
    pub completed_pages: u32,
    pub bytes: u64,
    pub error_code: Option<String>,
    pub generation: u64,
    /// Higher values receive earlier retained-download acquisition slots.
    pub priority: u64,
    pub created_at: i64,
    pub updated_at: i64,
    pub retained: bool,
    /// An explicit whole-chapter request must refresh access and source descriptors before reuse.
    pub refresh_source: bool,
    /// A bounded terminal notification remains replayable until its matching outbox entry is acknowledged.
    #[serde(default)]
    pub terminal_event: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct PageReceipt {
    pub episode_id: String,
    pub revision: String,
    pub page_id: String,
    pub page_index: usize,
    /// A content digest with an optional image extension, never a source path.
    pub file_name: String,
    pub sha256: String,
    pub bytes: u64,
    pub width: u32,
    pub height: u32,
    pub retained: bool,
    pub verified_at: i64,
    /// Zero denotes a legacy receipt that has not passed complete image decoding.
    pub decode_validation_version: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PendingPageCommit {
    pub task_id: String,
    pub generation: u64,
    pub receipt: PageReceipt,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingDownloadEvent {
    pub id: i64,
    pub task_id: String,
    pub generation: u64,
    pub payload: serde_json::Value,
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let requested = path.as_ref();
        let parent = requested.parent().filter(|value| !value.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent).context("Create the metadata directory")?;
        let parent = parent.canonicalize().context("Resolve the metadata directory")?;
        let file_name = requested.file_name().context("The metadata database needs a file name")?;
        let path = parent.join(file_name);
        if let Ok(metadata) = fs::symlink_metadata(&path) {
            ensure!(!metadata.file_type().is_symlink(), "The metadata database cannot be a symlink");
        }
        let connection = Connection::open(&path).context("Open the metadata database")?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        let store = Self { path, connection };
        store.migrate()?;
        set_private_permissions(&parent, true)?;
        set_private_permissions(&store.path, false)?;
        Ok(store)
    }

    pub fn path(&self) -> &Path { &self.path }

    pub fn reopen(&self) -> Result<Self> { Self::open(&self.path) }

    fn migrate(&self) -> Result<()> {
        let version: i64 = self.connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        ensure!(version <= SCHEMA_VERSION, "The metadata schema is newer than this application");
        if version == SCHEMA_VERSION { return Ok(()); }
        let transaction = Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let version: i64 = transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
        ensure!(version <= SCHEMA_VERSION, "The metadata schema is newer than this application");
        if version == SCHEMA_VERSION {
            transaction.commit()?;
            return Ok(());
        }
        if version == 1 {
            transaction.execute_batch(
                "CREATE TABLE IF NOT EXISTS page_identity_history (
                     account_key TEXT NOT NULL, episode_id TEXT NOT NULL, revision TEXT NOT NULL,
                     page_id TEXT NOT NULL, sha256 TEXT NOT NULL, bytes INTEGER NOT NULL,
                     PRIMARY KEY(account_key,episode_id,revision,page_id));
                 INSERT OR IGNORE INTO page_identity_history(account_key,episode_id,revision,page_id,sha256,bytes)
                     SELECT account_key,episode_id,revision,page_id,json_extract(payload,'$.sha256'),json_extract(payload,'$.bytes') FROM page_receipts;"
            ).context("Migrate retained page identities")?;
        }
        if version > 0 {
            if version == 1 || version == 2 {
                transaction.execute_batch(
                    "ALTER TABLE page_identity_history ADD COLUMN decode_validation_version INTEGER NOT NULL DEFAULT 0;"
                ).context("Migrate complete image validation proofs")?;
            }
            migrate_resource_schema(&transaction)?;
            transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            transaction.commit()?;
            return Ok(());
        }
        transaction.execute_batch(
            "CREATE TABLE IF NOT EXISTS comics (
                 account_key TEXT NOT NULL, comic_id TEXT NOT NULL, payload TEXT NOT NULL,
                 updated_at INTEGER NOT NULL, last_read_at INTEGER NOT NULL,
                 PRIMARY KEY(account_key,comic_id));
             CREATE INDEX IF NOT EXISTS comics_recent ON comics(account_key,last_read_at DESC,updated_at DESC);
             CREATE TABLE IF NOT EXISTS comic_details (
                 account_key TEXT NOT NULL, comic_id TEXT NOT NULL, payload TEXT NOT NULL,
                 PRIMARY KEY(account_key,comic_id));
             CREATE TABLE IF NOT EXISTS catalog_members (
                 account_key TEXT NOT NULL, scope TEXT NOT NULL, comic_id TEXT NOT NULL,
                 position INTEGER NOT NULL, PRIMARY KEY(account_key,scope,comic_id));
             CREATE TABLE IF NOT EXISTS image_indices (
                 account_key TEXT NOT NULL, episode_id TEXT NOT NULL, revision TEXT NOT NULL,
                 payload TEXT NOT NULL, created_at INTEGER NOT NULL,
                 PRIMARY KEY(account_key,episode_id,revision));
             CREATE TABLE IF NOT EXISTS reading_anchors (
                 account_key TEXT NOT NULL, comic_id TEXT NOT NULL, episode_id TEXT NOT NULL,
                 revision TEXT NOT NULL, payload TEXT NOT NULL, updated_at INTEGER NOT NULL,
                 PRIMARY KEY(account_key,comic_id));
             CREATE INDEX IF NOT EXISTS anchors_recent ON reading_anchors(account_key,updated_at DESC);
             CREATE TABLE IF NOT EXISTS preferences (
                 account_key TEXT NOT NULL, key TEXT NOT NULL, payload TEXT NOT NULL,
                 PRIMARY KEY(account_key,key));
             CREATE TABLE IF NOT EXISTS downloads (
                 account_key TEXT NOT NULL, task_id TEXT NOT NULL, episode_id TEXT NOT NULL,
                 revision TEXT NOT NULL, status TEXT NOT NULL, payload TEXT NOT NULL,
                 updated_at INTEGER NOT NULL, PRIMARY KEY(account_key,task_id));
             CREATE INDEX IF NOT EXISTS downloads_episode ON downloads(account_key,episode_id,revision);
             CREATE TABLE IF NOT EXISTS page_receipts (
                 account_key TEXT NOT NULL, episode_id TEXT NOT NULL, revision TEXT NOT NULL,
                 page_id TEXT NOT NULL, page_index INTEGER NOT NULL, payload TEXT NOT NULL,
                 PRIMARY KEY(account_key,episode_id,revision,page_id));
             CREATE INDEX IF NOT EXISTS pages_order ON page_receipts(account_key,episode_id,revision,page_index);
             CREATE TABLE IF NOT EXISTS page_commit_journal (
                 account_key TEXT NOT NULL, task_id TEXT NOT NULL, page_id TEXT NOT NULL,
                 generation INTEGER NOT NULL, payload TEXT NOT NULL, PRIMARY KEY(account_key,task_id,page_id));
             CREATE TABLE IF NOT EXISTS page_identity_history (
                 account_key TEXT NOT NULL, episode_id TEXT NOT NULL, revision TEXT NOT NULL,
                 page_id TEXT NOT NULL, sha256 TEXT NOT NULL, bytes INTEGER NOT NULL,
                 decode_validation_version INTEGER NOT NULL DEFAULT 0,
                 PRIMARY KEY(account_key,episode_id,revision,page_id));"
        ).context("Migrate the metadata schema")?;
        migrate_resource_schema(&transaction)?;
        transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn save_comics(&mut self, account: &str, comics: &[Comic]) -> Result<()> {
        validate_key(account)?;
        let transaction = self.connection.transaction()?;
        for comic in comics { upsert_comic(&transaction, account, comic)?; }
        transaction.commit()?;
        Ok(())
    }

    pub fn save_comic(&mut self, account: &str, comic: &Comic) -> Result<()> {
        self.save_comics(account, std::slice::from_ref(comic))
    }

    pub fn load_comics(&self, account: &str) -> Result<Vec<Comic>> {
        validate_key(account)?;
        let mut statement = self.connection.prepare(
            "SELECT payload FROM comics WHERE account_key=?1 ORDER BY last_read_at DESC,updated_at DESC,comic_id")?;
        let values = statement.query_map([account], |row| row.get::<_, String>(0))?;
        let mut comics = Vec::new();
        for value in values { comics.push(self.decorate_comic(account, from_json(&value?)?)?); }
        comics.sort_by(|left, right| right.last_read_at.cmp(&left.last_read_at)
            .then_with(|| right.updated_at.cmp(&left.updated_at)).then_with(|| left.id.cmp(&right.id)));
        Ok(comics)
    }

    pub fn comic(&self, account: &str, comic_id: &str) -> Result<Option<Comic>> {
        validate_key(account)?;
        validate_key(comic_id)?;
        let value: Option<String> = self.connection.query_row(
            "SELECT payload FROM comics WHERE account_key=?1 AND comic_id=?2",
            params![account, comic_id], |row| row.get(0)).optional()?;
        value.map(|value| self.decorate_comic(account, from_json(&value)?)).transpose()
    }

    /// A catalog is a cached ordered result set, independent of the local library.
    pub fn save_catalog(&mut self, account: &str, scope: &str, comics: &[Comic]) -> Result<()> {
        validate_key(account)?;
        validate_key(scope)?;
        let transaction = self.connection.transaction()?;
        transaction.execute("DELETE FROM catalog_members WHERE account_key=?1 AND scope=?2", params![account,scope])?;
        for (position, comic) in comics.iter().enumerate() {
            upsert_comic(&transaction, account, comic)?;
            transaction.execute(
                "INSERT OR REPLACE INTO catalog_members(account_key,scope,comic_id,position) VALUES(?1,?2,?3,?4)",
                params![account,scope,comic.id,position as i64])?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn load_catalog(&self, account: &str, scope: &str) -> Result<Vec<Comic>> {
        validate_key(account)?;
        validate_key(scope)?;
        let mut statement = self.connection.prepare(
            "SELECT c.payload FROM catalog_members m JOIN comics c ON c.account_key=m.account_key AND c.comic_id=m.comic_id
             WHERE m.account_key=?1 AND m.scope=?2 ORDER BY m.position")?;
        let values = statement.query_map(params![account,scope], |row| row.get::<_, String>(0))?;
        let mut comics = Vec::new();
        for value in values { comics.push(self.decorate_comic(account, from_json(&value?)?)?); }
        Ok(comics)
    }

    pub fn save_detail(&mut self, account: &str, detail: &ComicDetail) -> Result<()> {
        validate_key(account)?;
        let transaction = self.connection.transaction()?;
        upsert_comic(&transaction, account, &detail.comic)?;
        transaction.execute(
            "INSERT INTO comic_details(account_key,comic_id,payload) VALUES(?1,?2,?3)
             ON CONFLICT(account_key,comic_id) DO UPDATE SET payload=excluded.payload",
            params![account,detail.comic.id,to_json(detail)?])?;
        transaction.commit()?;
        Ok(())
    }

    pub fn load_detail(&self, account: &str, comic_id: &str) -> Result<Option<ComicDetail>> {
        validate_key(account)?;
        validate_key(comic_id)?;
        let value: Option<String> = self.connection.query_row(
            "SELECT payload FROM comic_details WHERE account_key=?1 AND comic_id=?2",
            params![account,comic_id], |row| row.get(0)).optional()?;
        value.map(|value| {
            let mut detail: ComicDetail = from_json(&value)?;
            detail.comic = self.decorate_comic(account, detail.comic)?;
            Ok(detail)
        }).transpose()
    }

    pub fn save_index(&self, account: &str, index: &ImageIndex) -> Result<()> {
        validate_key(account)?;
        validate_index(index)?;
        let transaction = Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let previous: Option<String> = transaction.query_row(
            "SELECT payload FROM image_indices WHERE account_key=?1 AND episode_id=?2 AND revision=?3",
            params![account,index.episode_id,index.revision], |row| row.get(0)).optional()?;
        if let Some(previous) = previous {
            let previous: ImageIndex = from_json(&previous)?;
            ensure!(previous.images == index.images,
                "An immutable source revision cannot change its page descriptors");
        }
        let mut persistent = index.clone();
        persistent.paths.clear();
        persistent.extra = serde_json::Value::Null;
        transaction.execute(
            "INSERT INTO image_indices(account_key,episode_id,revision,payload,created_at) VALUES(?1,?2,?3,?4,?5)
             ON CONFLICT(account_key,episode_id,revision) DO UPDATE SET payload=excluded.payload",
            params![account,index.episode_id,index.revision,to_json(&persistent)?,unix_seconds()])?;
        transaction.commit()?;
        Ok(())
    }

    pub fn load_index(&self, account: &str, episode_id: &str, revision: Option<&str>) -> Result<Option<ImageIndex>> {
        validate_key(account)?;
        validate_key(episode_id)?;
        let value: Option<String> = if let Some(revision) = revision {
            self.connection.query_row(
                "SELECT payload FROM image_indices WHERE account_key=?1 AND episode_id=?2 AND revision=?3",
                params![account,episode_id,revision], |row| row.get(0)).optional()?
        } else {
            self.connection.query_row(
                "SELECT payload FROM image_indices WHERE account_key=?1 AND episode_id=?2 ORDER BY created_at DESC,rowid DESC LIMIT 1",
                params![account,episode_id], |row| row.get(0)).optional()?
        };
        value.map(|value| from_json(&value)).transpose()
    }

    pub fn save_anchor(&self, account: &str, comic_id: &str, anchor: &ReadingAnchor) -> Result<()> {
        validate_key(account)?;
        validate_key(comic_id)?;
        validate_key(&anchor.episode_id)?;
        validate_key(&anchor.revision)?;
        validate_key(&anchor.page_id)?;
        ensure!(anchor.source_x.is_finite() && (0.0..=1.0).contains(&anchor.source_x)
            && anchor.source_y.is_finite() && (0.0..=1.0).contains(&anchor.source_y)
            && anchor.zoom_ratio.is_finite() && (0.1..=16.0).contains(&anchor.zoom_ratio), "Invalid source anchor geometry");
        self.connection.execute(
            "INSERT INTO reading_anchors(account_key,comic_id,episode_id,revision,payload,updated_at) VALUES(?1,?2,?3,?4,?5,?6)
             ON CONFLICT(account_key,comic_id) DO UPDATE SET episode_id=excluded.episode_id,revision=excluded.revision,payload=excluded.payload,updated_at=excluded.updated_at",
            params![account,comic_id,anchor.episode_id,anchor.revision,to_json(anchor)?,anchor.updated_at])?;
        Ok(())
    }

    pub fn load_anchor(&self, account: &str, comic_id: &str) -> Result<Option<ReadingAnchor>> {
        validate_key(account)?;
        validate_key(comic_id)?;
        let value: Option<String> = self.connection.query_row(
            "SELECT payload FROM reading_anchors WHERE account_key=?1 AND comic_id=?2",
            params![account,comic_id], |row| row.get(0)).optional()?;
        value.map(|value| from_json(&value)).transpose()
    }

    pub fn latest_anchor(&self, account: &str) -> Result<Option<(String, ReadingAnchor)>> {
        validate_key(account)?;
        let value: Option<(String, String)> = self.connection.query_row(
            "SELECT comic_id,payload FROM reading_anchors WHERE account_key=?1 ORDER BY updated_at DESC LIMIT 1",
            [account], |row| Ok((row.get(0)?,row.get(1)?))).optional()?;
        value.map(|(id, value)| Ok((id, from_json(&value)?))).transpose()
    }

    pub fn set_preference<T: Serialize>(&self, account: &str, key: &str, value: &T) -> Result<()> {
        validate_key(account)?;
        validate_key(key)?;
        self.connection.execute(
            "INSERT INTO preferences(account_key,key,payload) VALUES(?1,?2,?3)
             ON CONFLICT(account_key,key) DO UPDATE SET payload=excluded.payload",
            params![account,key,to_json(value)?])?;
        Ok(())
    }

    pub fn preference<T: DeserializeOwned>(&self, account: &str, key: &str) -> Result<Option<T>> {
        validate_key(account)?;
        validate_key(key)?;
        let value: Option<String> = self.connection.query_row(
            "SELECT payload FROM preferences WHERE account_key=?1 AND key=?2", params![account,key], |row| row.get(0)).optional()?;
        value.map(|value| from_json(&value)).transpose()
    }

    pub fn save_download(&self, account: &str, record: &DownloadRecord) -> Result<()> {
        validate_key(account)?;
        validate_download(record)?;
        let transaction = Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let mut binding_fresh = false;
        let mut discard_old_journals = record.refresh_source;
        let mut discard_old_events = false;
        if let Some(previous) = self.load_download(account, &record.id)? {
            ensure!(record.generation >= previous.generation, "A stale task generation cannot replace a download");
            discard_old_events = record.generation != previous.generation || record.terminal_event != previous.terminal_event;
            if !previous.refresh_source && record.refresh_source {
                ensure!(record.generation > previous.generation && record.status == DownloadStatus::Queued,
                    "A source refresh must start a new queued generation");
            }
            if previous.refresh_source && !record.refresh_source {
                ensure!(previous.retained && record.retained && previous.comic_id == record.comic_id
                    && record.generation == previous.generation && record.status == DownloadStatus::Running
                    && matches!(previous.status, DownloadStatus::Queued | DownloadStatus::Running)
                    && record.revision != PENDING_REVISION,
                    "A source refresh must bind a running retained generation before completion");
                let target = self.load_index(account, &record.episode_id, Some(&record.revision))?
                    .context("A refreshed download needs its checked source descriptor")?;
                let receipts = self.page_receipts(account, &record.episode_id, &record.revision)?;
                let current: Vec<_> = receipts.iter().filter(|receipt| target.images.get(receipt.page_index)
                    .is_some_and(|page| receipt.page_id == page.id && receipt.width == page.width
                        && receipt.height == page.height && receipt.decode_validation_version == VALIDATION_VERSION)).collect();
                ensure!(record.total_pages as usize == target.images.len() && record.completed_pages as usize == current.len()
                    && record.bytes == current.iter().try_fold(0_u64, |sum, receipt|
                        sum.checked_add(receipt.bytes).context("Downloaded byte count overflow"))?,
                    "A refreshed download must count only its current source revision");
                binding_fresh = true;
                discard_old_journals = true;
            }
            let binding_pending = previous.revision == PENDING_REVISION
                && record.revision != PENDING_REVISION
                && self.page_receipts(account, &previous.episode_id, PENDING_REVISION)?.is_empty()
                && !self.pending_page_commits(account)?.iter().any(|entry| entry.task_id == record.id);
            ensure!(previous.episode_id == record.episode_id
                && (previous.revision == record.revision || binding_pending || binding_fresh),
                "A task identity cannot be rebound to different content");
        }
        if record.status == DownloadStatus::Completed {
            let index = self.load_index(account, &record.episode_id, Some(&record.revision))?
                .context("A completed download needs a source descriptor")?;
            let receipts = self.page_receipts(account, &record.episode_id, &record.revision)?;
            ensure!(record.total_pages as usize == index.images.len() && receipts.len() == index.images.len()
                && index.images.iter().all(|page|
                receipts.iter().any(|receipt| receipt.page_index == page.index && receipt.page_id == page.id
                    && receipt.width == page.width && receipt.height == page.height
                    && (!record.retained || receipt.retained)
                    && receipt.decode_validation_version == VALIDATION_VERSION)),
                "A completed download needs every completely decoded page receipt");
            ensure!(record.bytes == receipts.iter().try_fold(0_u64, |sum, receipt|
                sum.checked_add(receipt.bytes).context("Downloaded byte count overflow"))?,
                "A completed download must count every verified byte");
        }
        let changed = transaction.execute(
            "INSERT INTO downloads(account_key,task_id,episode_id,revision,status,payload,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7)
             ON CONFLICT(account_key,task_id) DO UPDATE SET revision=excluded.revision,status=excluded.status,
               payload=json_set(excluded.payload,'$.priority',MAX(COALESCE(json_extract(downloads.payload,'$.priority'),0),?9)),
               updated_at=excluded.updated_at
             WHERE downloads.episode_id=excluded.episode_id
               AND (downloads.revision=excluded.revision OR ?10=1 OR
                    (downloads.revision='__pending__'
                     AND NOT EXISTS(SELECT 1 FROM page_receipts p WHERE p.account_key=downloads.account_key AND p.episode_id=downloads.episode_id AND p.revision=downloads.revision)
                     AND NOT EXISTS(SELECT 1 FROM page_commit_journal j WHERE j.account_key=downloads.account_key AND j.task_id=downloads.task_id)))
               AND CAST(json_extract(downloads.payload,'$.generation') AS INTEGER)<=?8",
            params![account,record.id,record.episode_id,record.revision,record.status.as_str(),to_json(record)?,record.updated_at,
                record.generation as i64,record.priority as i64,binding_fresh])?;
        ensure!(changed == 1, "A stale task generation cannot replace a download");
        if discard_old_journals {
            transaction.execute("DELETE FROM page_commit_journal WHERE account_key=?1 AND task_id=?2", params![account,record.id])?;
        }
        transaction.execute(
            "DELETE FROM download_event_outbox WHERE account_key=?1 AND task_id=?2 AND (generation<>?3 OR ?4=1)",
            params![account,record.id,record.generation as i64,discard_old_events])?;
        transaction.commit()?;
        Ok(())
    }

    /// Allocate and persist an account-local priority without changing task state or generation.
    pub fn prioritize_download(&self, account: &str, task_id: &str) -> Result<u64> {
        validate_key(account)?;
        validate_key(task_id)?;
        let transaction = Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let record = load_download_from(&transaction, account, task_id)?
            .context("The download task does not exist")?;
        validate_download(&record)?;
        ensure!(record.retained && matches!(record.status, DownloadStatus::Queued | DownloadStatus::Running),
            "Only active retained downloads can be prioritized");
        let maximum: Option<i64> = transaction.query_row(
            "SELECT MAX(json_extract(payload,'$.priority')) FROM downloads WHERE account_key=?1",
            [account], |row| row.get(0))?;
        let maximum = maximum.unwrap_or(0);
        ensure!(maximum >= 0, "The persisted download priority is invalid");
        let priority = maximum.checked_add(1).context("Download priority overflow")?;
        let changed = transaction.execute(
            "UPDATE downloads SET payload=json_set(payload,'$.priority',?3)
             WHERE account_key=?1 AND task_id=?2 AND status IN ('queued','running')
               AND json_extract(payload,'$.retained')=1
               AND CAST(json_extract(payload,'$.generation') AS INTEGER)=?4",
            params![account,task_id,priority,record.generation as i64])?;
        ensure!(changed == 1, "The download is no longer eligible for priority");
        transaction.commit()?;
        Ok(priority as u64)
    }

    pub fn load_download(&self, account: &str, task_id: &str) -> Result<Option<DownloadRecord>> {
        validate_key(account)?;
        validate_key(task_id)?;
        let value: Option<String> = self.connection.query_row(
            "SELECT payload FROM downloads WHERE account_key=?1 AND task_id=?2",
            params![account,task_id], |row| row.get(0)).optional()?;
        value.map(|value| from_json(&value)).transpose()
    }

    pub fn downloads(&self, account: &str) -> Result<Vec<DownloadRecord>> {
        validate_key(account)?;
        let mut statement = self.connection.prepare(
            "SELECT payload FROM downloads WHERE account_key=?1
             ORDER BY CASE status WHEN 'failed' THEN 0 WHEN 'running' THEN 1 WHEN 'queued' THEN 2 WHEN 'paused' THEN 3 WHEN 'completed' THEN 4 ELSE 5 END,updated_at DESC,task_id")?;
        let values = statement.query_map([account], |row| row.get::<_, String>(0))?;
        values.map(|value| from_json(&value?)).collect()
    }

    /// Read a bounded set of still-active tasks for a suspended checkpoint.
    pub(crate) fn active_downloads_page(&self, account: &str, after_task: Option<&str>, limit: usize) -> Result<Vec<DownloadRecord>> {
        validate_key(account)?;
        if let Some(task) = after_task { validate_key(task)?; }
        if limit == 0 { return Ok(Vec::new()); }
        let mut statement = self.connection.prepare(
            "SELECT payload FROM downloads WHERE account_key=?1 AND task_id>?2
             AND status IN ('queued','running') ORDER BY task_id LIMIT ?3")?;
        let rows = statement.query_map(params![account, after_task.unwrap_or(""), limit.min(64) as i64],
            |row| row.get::<_, String>(0))?;
        rows.map(|row| from_json(&row?)).collect()
    }

    /// A background sleep snapshot cannot commit after its UI scope is revoked.
    /// Foreground resume revokes before saving the live RAM state; transaction
    /// ordering therefore makes that newer state the final writer.
    pub(crate) fn write_sleep_checkpoint(
        &self, account: &str, anchor: Option<(&str, &ReadingAnchor)>,
        preferences: &[(String, serde_json::Value)], pause_candidates: &[DownloadRecord],
        now: i64, revoked: &std::sync::atomic::AtomicBool,
    ) -> Result<()> {
        use std::sync::atomic::Ordering;
        validate_key(account)?;
        ensure!(!revoked.load(Ordering::Acquire), "Sleep checkpoint was superseded");
        let transaction = Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        ensure!(!revoked.load(Ordering::Acquire), "Sleep checkpoint was superseded");
        if let Some((comic, anchor)) = anchor { self.save_anchor(account, comic, anchor)?; }
        for (key, value) in preferences {
            ensure!(!revoked.load(Ordering::Acquire), "Sleep checkpoint was superseded");
            self.set_preference(account, key, value)?;
        }
        for candidate in pause_candidates {
            ensure!(!revoked.load(Ordering::Acquire), "Sleep checkpoint was superseded");
            let Some(mut current) = self.load_download(account, &candidate.id)? else { continue; };
            if current.generation != candidate.generation || current.episode_id != candidate.episode_id
                || current.revision != candidate.revision || current.retained != candidate.retained
                || !matches!(current.status, DownloadStatus::Running | DownloadStatus::Queued) { continue; }
            current.generation = current.generation.checked_add(1).context("Download generation overflow")?;
            current.status = DownloadStatus::Paused;
            current.error_code = None;
            current.terminal_event = Some(serde_json::json!({"kind":"paused"}));
            current.updated_at = now;
            validate_download(&current)?;
            let changed = transaction.execute(
                "UPDATE downloads SET status='paused',payload=?1,updated_at=?2
                 WHERE account_key=?3 AND task_id=?4 AND status IN ('queued','running')
                 AND COALESCE(json_extract(payload,'$.generation'),0)=?5",
                params![to_json(&current)?, now, account, current.id, candidate.generation as i64])?;
            if changed == 1 {
                transaction.execute("DELETE FROM download_event_outbox WHERE account_key=?1 AND task_id=?2",
                    params![account, current.id])?;
            }
        }
        ensure!(!revoked.load(Ordering::Acquire), "Sleep checkpoint was superseded");
        transaction.commit()?;
        Ok(())
    }

    /// Project the global wake state and retained-comic error badge without
    /// materializing chapter payloads or rebuilding the downloads view.
    pub fn download_activity(&self, account: &str) -> Result<(bool, u32)> {
        validate_key(account)?;
        self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM downloads WHERE account_key=?1 AND status='running'),
                (SELECT COUNT(DISTINCT json_extract(payload,'$.comic_id')) FROM downloads
                 WHERE account_key=?1 AND status='failed' AND json_extract(payload,'$.retained')=1)",
            [account], |row| Ok((row.get(0)?, row.get(1)?)))
            .context("Read global download activity")
    }

    /// Reconcile terminal notifications with a bounded seek page rather than a full task snapshot.
    pub fn terminal_downloads_page(
        &self, account: &str, after_task_id: Option<&str>, limit: usize,
    ) -> Result<Vec<DownloadRecord>> {
        validate_key(account)?;
        if let Some(task_id) = after_task_id { validate_key(task_id)?; }
        if limit == 0 { return Ok(Vec::new()); }
        let mut statement = self.connection.prepare(
            "SELECT payload FROM downloads WHERE account_key=?1 AND task_id>?2
                 AND status IN ('completed','paused','failed','cancelled')
                 AND json_type(payload,'$.terminal_event')='object'
             ORDER BY task_id LIMIT ?3")?;
        let rows = statement.query_map(params![account,after_task_id.unwrap_or_default(),limit.min(1024) as i64],
            |row| row.get::<_, String>(0))?;
        rows.map(|row| {
            let record: DownloadRecord = from_json(&row?)?;
            validate_download(&record)?;
            Ok(record)
        }).collect()
    }

    /// The caller holds publication ownership and protects every live reader task,
    /// including paused prefetches, after invalidating all retired task controls.
    /// Page receipts, histories, source indexes, and journal references are not removed.
    pub(crate) fn prune_reader_downloads(&self, account: &str, protected_task_ids: &[String]) -> Result<usize> {
        validate_key(account)?;
        for task_id in protected_task_ids { validate_key(task_id)?; }
        let protected = to_json(&protected_task_ids)?;
        let transaction = Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let removed = transaction.execute(
            "DELETE FROM downloads WHERE account_key=?1
                 AND status IN ('completed','paused','failed','cancelled')
                 AND json_extract(payload,'$.retained')=0
                 AND COALESCE(json_extract(payload,'$.refresh_source'),0)=0
                 AND task_id NOT IN (SELECT value FROM json_each(?2))
                 AND NOT EXISTS(SELECT 1 FROM page_commit_journal j
                     WHERE j.account_key=downloads.account_key AND j.task_id=downloads.task_id)",
            params![account,protected])?;
        transaction.commit()?;
        Ok(removed)
    }

    /// Persist only the latest terminal notification for the current task generation.
    pub fn record_download_event(
        &self, account: &str, task_id: &str, generation: u64, payload: &serde_json::Value,
    ) -> Result<()> {
        validate_key(account)?;
        validate_key(task_id)?;
        ensure!(generation <= i64::MAX as u64, "Invalid event generation");
        let kind = validate_download_event(payload)?;
        let encoded = to_json(payload)?;
        ensure!(encoded.len() <= 1024, "The terminal event payload exceeds its bound");
        let transaction = Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let task = load_download_from(&transaction, account, task_id)?.context("The download task does not exist")?;
        validate_download(&task)?;
        ensure!(task.generation == generation && event_matches_status(kind, task.status) && task.terminal_event.as_ref() == Some(payload),
            "A terminal event no longer matches its download generation and state");
        let previous: Option<(i64, String)> = transaction.query_row(
            "SELECT generation,payload FROM download_event_outbox WHERE account_key=?1 AND task_id=?2",
            params![account,task_id], |row| Ok((row.get(0)?,row.get(1)?))).optional()?;
        if let Some((previous_generation, previous_payload)) = previous {
            if previous_generation >= 0 && previous_generation as u64 == generation
                && from_json::<serde_json::Value>(&previous_payload)? == *payload {
                transaction.commit()?;
                return Ok(());
            }
        }
        transaction.execute("DELETE FROM download_event_outbox WHERE account_key=?1 AND task_id=?2", params![account,task_id])?;
        transaction.execute(
            "INSERT INTO download_event_outbox(account_key,task_id,generation,payload) VALUES(?1,?2,?3,?4)",
            params![account,task_id,generation as i64,encoded])?;
        transaction.commit()?;
        Ok(())
    }

    /// The bounded outbox is replayed independently of transient progress notifications.
    pub fn pending_download_events(&self, account: &str, limit: usize) -> Result<Vec<PendingDownloadEvent>> {
        validate_key(account)?;
        if limit == 0 { return Ok(Vec::new()); }
        let mut statement = self.connection.prepare(
            "SELECT e.id,e.task_id,e.generation,e.payload FROM download_event_outbox e
             JOIN downloads d ON d.account_key=e.account_key AND d.task_id=e.task_id
                 AND CAST(json_extract(d.payload,'$.generation') AS INTEGER)=e.generation
             WHERE e.account_key=?1 AND (
                 (json_extract(e.payload,'$.kind')='completed' AND d.status='completed') OR
                 (json_extract(e.payload,'$.kind') IN ('paused','prefetch_ready') AND d.status='paused') OR
                 (json_extract(e.payload,'$.kind')='cancelled' AND d.status='cancelled') OR
                 (json_extract(e.payload,'$.kind')='failed' AND d.status='failed'))
             ORDER BY e.id LIMIT ?2")?;
        let rows = statement.query_map(params![account,limit.min(1024) as i64], |row| {
            Ok((row.get::<_, i64>(0)?,row.get::<_, String>(1)?,row.get::<_, i64>(2)?,row.get::<_, String>(3)?))
        })?;
        rows.map(|row| {
            let (id, task_id, generation, encoded) = row?;
            ensure!(id > 0 && generation >= 0, "Invalid durable terminal event identity");
            let payload = from_json(&encoded)?;
            validate_download_event(&payload)?;
            Ok(PendingDownloadEvent { id, task_id, generation: generation as u64, payload })
        }).collect()
    }

    /// A replaced event has a different monotonically allocated identifier.
    pub fn acknowledge_download_event(&self, account: &str, id: i64) -> Result<()> {
        validate_key(account)?;
        ensure!(id > 0, "Invalid durable terminal event identity");
        let transaction = Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let event: Option<(String, i64, String)> = transaction.query_row(
            "SELECT task_id,generation,payload FROM download_event_outbox WHERE account_key=?1 AND id=?2",
            params![account,id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional()?;
        if let Some((task_id, generation, payload)) = event {
            let payload: serde_json::Value = from_json(&payload)?;
            if let Some(record) = load_download_from(&transaction, account, &task_id)? {
                if generation >= 0 && record.generation == generation as u64 && record.terminal_event.as_ref() == Some(&payload) {
                    transaction.execute(
                        "UPDATE downloads SET payload=json_set(payload,'$.terminal_event',json('null')) WHERE account_key=?1 AND task_id=?2",
                        params![account,task_id])?;
                }
            }
        }
        transaction.execute("DELETE FROM download_event_outbox WHERE account_key=?1 AND id=?2", params![account,id])?;
        transaction.commit()?;
        Ok(())
    }

    pub fn update_download_state(&self, account: &str, task_id: &str, status: DownloadStatus, error_code: Option<&str>, now: i64) -> Result<()> {
        let mut record = self.load_download(account, task_id)?.context("The download task does not exist")?;
        record.status = status;
        record.error_code = error_code.map(str::to_owned);
        record.updated_at = now;
        record.terminal_event = match status {
            DownloadStatus::Queued | DownloadStatus::Running => None,
            DownloadStatus::Paused => Some(serde_json::json!({"kind":"paused"})),
            DownloadStatus::Cancelled => Some(serde_json::json!({"kind":"cancelled"})),
            DownloadStatus::Completed => Some(serde_json::json!({"kind":"completed"})),
            // This compatibility API has no retry policy; keep only an already exact failure intent.
            DownloadStatus::Failed => record.terminal_event.take().filter(|payload|
                payload.get("kind").and_then(serde_json::Value::as_str) == Some("failed")
                    && payload.get("code").and_then(serde_json::Value::as_str) == record.error_code.as_deref()),
        };
        self.save_download(account, &record)
    }

    pub fn page_receipt(&self, account: &str, episode_id: &str, revision: &str, page_id: &str) -> Result<Option<PageReceipt>> {
        validate_key(account)?;
        load_page_receipt_from(&self.connection, account, episode_id, revision, page_id)
    }

    pub fn page_receipts(&self, account: &str, episode_id: &str, revision: &str) -> Result<Vec<PageReceipt>> {
        validate_key(account)?;
        let mut statement = self.connection.prepare(
            "SELECT payload FROM page_receipts WHERE account_key=?1 AND episode_id=?2 AND revision=?3 ORDER BY page_index")?;
        let values = statement.query_map(params![account,episode_id,revision], |row| row.get::<_, String>(0))?;
        values.map(|value| from_json(&value?)).collect()
    }

    /// Bind an actual complete decode to the current object without losing a concurrent retain.
    pub(crate) fn promote_page_validation(
        &self, account: &str, expected: &PageReceipt, proof: &DecodedImageProof, verified_at: i64,
    ) -> Result<Option<PageReceipt>> {
        validate_key(account)?;
        validate_receipt(expected)?;
        validate_decode_proof(expected, proof)?;
        let transaction = Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let Some(mut current) = load_page_receipt_from(&transaction, account, &expected.episode_id,
            &expected.revision, &expected.page_id)? else { return Ok(None); };
        if !same_receipt_object(&current, expected) { return Ok(None); }
        validate_receipt(&current)?;
        let historical: Option<(String, i64, i64)> = transaction.query_row(
            "SELECT sha256,bytes,decode_validation_version FROM page_identity_history WHERE account_key=?1 AND episode_id=?2 AND revision=?3 AND page_id=?4",
            params![account,current.episode_id,current.revision,current.page_id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional()?;
        let mut validation_version = current.decode_validation_version.max(VALIDATION_VERSION);
        if let Some((digest, bytes, previous_version)) = historical {
            ensure!(digest == current.sha256 && bytes >= 0 && bytes as u64 == current.bytes,
                "The decoded object conflicts with its historical content identity");
            ensure!(previous_version >= 0 && previous_version <= VALIDATION_VERSION as i64,
                "Invalid historical image validation version");
            validation_version = validation_version.max(previous_version as u32);
        }
        current.decode_validation_version = validation_version;
        current.verified_at = current.verified_at.max(verified_at);
        transaction.execute(
            "UPDATE page_receipts SET payload=?5 WHERE account_key=?1 AND episode_id=?2 AND revision=?3 AND page_id=?4",
            params![account,current.episode_id,current.revision,current.page_id,to_json(&current)?])?;
        transaction.execute(
            "INSERT INTO page_identity_history(account_key,episode_id,revision,page_id,sha256,bytes,decode_validation_version) VALUES(?1,?2,?3,?4,?5,?6,?7)
             ON CONFLICT(account_key,episode_id,revision,page_id) DO UPDATE SET
                 decode_validation_version=MAX(page_identity_history.decode_validation_version,excluded.decode_validation_version)",
            params![account,current.episode_id,current.revision,current.page_id,current.sha256,current.bytes as i64,validation_version as i64])?;
        transaction.commit()?;
        Ok(Some(current))
    }

    /// Only self-consistent legacy bytes proven to have invalid encoding may lose their identity.
    pub(crate) fn reject_legacy_invalid_page(
        &self, account: &str, expected: &PageReceipt, evidence: &InvalidEncodingProof,
    ) -> Result<bool> {
        validate_key(account)?;
        validate_receipt(expected)?;
        if expected.decode_validation_version != 0 || !evidence.sha256().eq_ignore_ascii_case(&expected.sha256)
            || evidence.byte_len() != expected.bytes { return Ok(false); }
        let transaction = Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let Some(current) = load_page_receipt_from(&transaction, account, &expected.episode_id,
            &expected.revision, &expected.page_id)? else { return Ok(false); };
        if current.decode_validation_version != 0 || !same_receipt_object(&current, expected) { return Ok(false); }
        let historical: Option<(String, i64, i64)> = transaction.query_row(
            "SELECT sha256,bytes,decode_validation_version FROM page_identity_history WHERE account_key=?1 AND episode_id=?2 AND revision=?3 AND page_id=?4",
            params![account,current.episode_id,current.revision,current.page_id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional()?;
        if let Some((digest, bytes, validation_version)) = historical {
            if digest != current.sha256 || bytes < 0 || bytes as u64 != current.bytes || validation_version != 0 {
                return Ok(false);
            }
        }
        transaction.execute(
            "DELETE FROM page_identity_history WHERE account_key=?1 AND episode_id=?2 AND revision=?3 AND page_id=?4
                 AND sha256=?5 AND bytes=?6 AND decode_validation_version=0",
            params![account,current.episode_id,current.revision,current.page_id,current.sha256,current.bytes as i64])?;
        invalidate_page_from(&transaction, account, &current.episode_id, &current.revision, &current.page_id)?;
        transaction.commit()?;
        Ok(true)
    }

    /// An explicit download may promote an already cached reader page without fetching it again.
    pub fn retain_page(&self, account: &str, episode_id: &str, revision: &str, page_id: &str) -> Result<bool> {
        validate_key(account)?;
        Ok(self.connection.execute(
            "UPDATE page_receipts SET payload=json_set(payload,'$.retained',json('true'))
             WHERE account_key=?1 AND episode_id=?2 AND revision=?3 AND page_id=?4",
            params![account,episode_id,revision,page_id])? > 0)
    }

    pub fn begin_page_commit(&self, account: &str, task_id: &str, receipt: &PageReceipt) -> Result<()> {
        let generation = self.load_download(account, task_id)?.context("The download task does not exist")?.generation;
        self.begin_page_commit_generation(account, task_id, generation, receipt)
    }

    pub fn begin_page_commit_generation(&self, account: &str, task_id: &str, generation: u64, receipt: &PageReceipt) -> Result<()> {
        ensure!(receipt.decode_validation_version == 0, "A strict page publication requires a complete decode proof");
        self.begin_page_commit_inner(account, task_id, generation, receipt)
    }

    pub(crate) fn begin_verified_page_commit_generation(
        &self, account: &str, task_id: &str, generation: u64, receipt: &PageReceipt, proof: &DecodedImageProof,
    ) -> Result<()> {
        validate_decode_proof(receipt, proof)?;
        ensure!(receipt.decode_validation_version == VALIDATION_VERSION,
            "A verified page publication must record its complete decode proof version");
        self.begin_page_commit_inner(account, task_id, generation, receipt)
    }

    fn begin_page_commit_inner(&self, account: &str, task_id: &str, generation: u64, receipt: &PageReceipt) -> Result<()> {
        validate_key(account)?;
        validate_receipt(receipt)?;
        let transaction = Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let task = load_download_from(&transaction, account, task_id)?.context("The download task does not exist")?;
        ensure!(task.generation == generation && task.status == DownloadStatus::Running && !task.refresh_source,
            "The task generation is no longer active");
        ensure!(task.episode_id == receipt.episode_id && task.revision == receipt.revision, "The page belongs to different content");
        transaction.execute(
            "INSERT INTO page_commit_journal(account_key,task_id,page_id,generation,payload) VALUES(?1,?2,?3,?4,?5)
             ON CONFLICT(account_key,task_id,page_id) DO UPDATE SET generation=excluded.generation,payload=excluded.payload",
            params![account,task_id,receipt.page_id,generation as i64,to_json(receipt)?])?;
        transaction.commit()?;
        Ok(())
    }

    /// The caller has already synchronized and atomically published the verified object.
    pub fn commit_page(&mut self, account: &str, task_id: &str, receipt: &PageReceipt) -> Result<()> {
        let generation = self.load_download(account, task_id)?.context("The download task does not exist")?.generation;
        self.commit_page_generation(account, task_id, generation, receipt)
    }

    pub fn commit_page_generation(&mut self, account: &str, task_id: &str, generation: u64, receipt: &PageReceipt) -> Result<()> {
        ensure!(receipt.decode_validation_version == 0, "A strict page publication requires a complete decode proof");
        self.commit_page_inner(account, task_id, generation, receipt)
    }

    pub(crate) fn commit_verified_page_generation(
        &mut self, account: &str, task_id: &str, generation: u64, receipt: &PageReceipt, proof: &DecodedImageProof,
    ) -> Result<()> {
        validate_decode_proof(receipt, proof)?;
        ensure!(receipt.decode_validation_version == VALIDATION_VERSION,
            "A verified page publication must record its complete decode proof version");
        self.commit_page_inner(account, task_id, generation, receipt)
    }

    fn commit_page_inner(&mut self, account: &str, task_id: &str, generation: u64, receipt: &PageReceipt) -> Result<()> {
        validate_key(account)?;
        validate_receipt(receipt)?;
        let transaction = self.connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut task = load_download_from(&transaction, account, task_id)?.context("The download task does not exist")?;
        ensure!(task.generation == generation && task.status == DownloadStatus::Running && !task.refresh_source,
            "The task generation is no longer active");
        ensure!(task.episode_id == receipt.episode_id && task.revision == receipt.revision, "The page belongs to different content");
        let journal: Option<(i64, String)> = transaction.query_row(
            "SELECT generation,payload FROM page_commit_journal WHERE account_key=?1 AND task_id=?2 AND page_id=?3",
            params![account,task_id,receipt.page_id], |row| Ok((row.get(0)?,row.get(1)?))).optional()?;
        let (expected_generation, expected_receipt) = journal.context("The page commit journal is missing")?;
        ensure!(expected_generation >= 0 && expected_generation as u64 == generation
            && from_json::<PageReceipt>(&expected_receipt)? == *receipt, "The page commit journal does not match");
        let descriptor_path = format!("$.images[{}]", receipt.page_index);
        let index_page: Option<(Option<i64>, Option<String>)> = transaction.query_row(
            "SELECT json_array_length(payload,'$.images'),json_extract(payload,?4)
             FROM image_indices WHERE account_key=?1 AND episode_id=?2 AND revision=?3",
            params![account,receipt.episode_id,receipt.revision,descriptor_path],
            |row| Ok((row.get(0)?,row.get(1)?))).optional()?;
        let (page_count, descriptor) = index_page.context("The source descriptor is missing")?;
        let page_count = page_count.context("The source page count is invalid")?;
        ensure!(page_count > 0 && page_count <= 10_000, "The source page count is invalid");
        let descriptor: crate::domain::PageDescriptor = from_json(&descriptor.context("The source page position is invalid")?)?;
        ensure!(descriptor.index == receipt.page_index && descriptor.id == receipt.page_id
            && descriptor.width == receipt.width && descriptor.height == receipt.height,
            "The receipt has a different page descriptor");
        let previous: Option<String> = transaction.query_row(
            "SELECT payload FROM page_receipts WHERE account_key=?1 AND episode_id=?2 AND revision=?3 AND page_id=?4",
            params![account,receipt.episode_id,receipt.revision,receipt.page_id], |row| row.get(0)).optional()?;
        let mut committed = receipt.clone();
        if let Some(previous) = previous {
            let previous: PageReceipt = from_json(&previous)?;
            ensure!(previous.sha256 == receipt.sha256 && previous.bytes == receipt.bytes,
                "An immutable page identity cannot be replaced with different bytes");
            committed.retained |= previous.retained;
            committed.decode_validation_version = committed.decode_validation_version.max(previous.decode_validation_version);
        }
        let historical: Option<(String, i64, i64)> = transaction.query_row(
            "SELECT sha256,bytes,decode_validation_version FROM page_identity_history WHERE account_key=?1 AND episode_id=?2 AND revision=?3 AND page_id=?4",
            params![account,receipt.episode_id,receipt.revision,receipt.page_id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional()?;
        if let Some((digest, bytes, validation_version)) = historical {
            ensure!(digest == receipt.sha256 && bytes >= 0 && bytes as u64 == receipt.bytes,
                "Recovered page bytes differ from the previously verified content identity");
            ensure!(validation_version >= 0 && validation_version <= VALIDATION_VERSION as i64,
                "Invalid historical image validation version");
            committed.decode_validation_version = committed.decode_validation_version.max(validation_version as u32);
        }
        transaction.execute(
            "INSERT INTO page_identity_history(account_key,episode_id,revision,page_id,sha256,bytes,decode_validation_version) VALUES(?1,?2,?3,?4,?5,?6,?7)
             ON CONFLICT(account_key,episode_id,revision,page_id) DO UPDATE SET
                 decode_validation_version=MAX(page_identity_history.decode_validation_version,excluded.decode_validation_version)",
            params![account,receipt.episode_id,receipt.revision,receipt.page_id,receipt.sha256,receipt.bytes as i64,committed.decode_validation_version as i64])?;
        transaction.execute(
            "INSERT INTO page_receipts(account_key,episode_id,revision,page_id,page_index,payload) VALUES(?1,?2,?3,?4,?5,?6)
             ON CONFLICT(account_key,episode_id,revision,page_id) DO UPDATE SET page_index=excluded.page_index,payload=excluded.payload",
            params![account,receipt.episode_id,receipt.revision,receipt.page_id,receipt.page_index as i64,to_json(&committed)?])?;
        let progress = receipt_progress_from(&transaction, account, &receipt.episode_id, &receipt.revision, true)?;
        task.completed_pages = progress.pages;
        task.total_pages = u32::try_from(page_count)?;
        task.bytes = progress.bytes;
        task.updated_at = receipt.verified_at;
        // Only a subsequent whole-chapter file verification may publish Completed.
        task.error_code = None;
        transaction.execute(
            "UPDATE downloads SET status=?3,payload=?4,updated_at=?5 WHERE account_key=?1 AND task_id=?2",
            params![account,task_id,task.status.as_str(),to_json(&task)?,task.updated_at])?;
        transaction.execute("DELETE FROM page_commit_journal WHERE account_key=?1 AND task_id=?2 AND page_id=?3",
            params![account,task_id,receipt.page_id])?;
        transaction.commit()?;
        Ok(())
    }

    pub fn pending_page_commits(&self, account: &str) -> Result<Vec<PendingPageCommit>> {
        validate_key(account)?;
        let mut statement = self.connection.prepare("SELECT task_id,generation,payload FROM page_commit_journal WHERE account_key=?1 ORDER BY task_id,page_id")?;
        let values = statement.query_map([account], |row| Ok((row.get::<_, String>(0)?,row.get::<_, i64>(1)?,row.get::<_, String>(2)?)))?;
        values.map(|value| {
            let (task_id,generation,payload) = value?;
            ensure!(generation >= 0, "Invalid journal generation");
            Ok(PendingPageCommit { task_id, generation: generation as u64, receipt: from_json(&payload)? })
        }).collect()
    }

    pub fn discard_page_commit(&self, account: &str, task_id: &str, page_id: &str) -> Result<()> {
        validate_key(account)?;
        self.connection.execute("DELETE FROM page_commit_journal WHERE account_key=?1 AND task_id=?2 AND page_id=?3",
            params![account,task_id,page_id])?;
        Ok(())
    }

    pub fn remove_episode(&mut self, account: &str, episode_id: &str, revision: &str) -> Result<Vec<PageReceipt>> {
        let receipts = self.page_receipts(account, episode_id, revision)?;
        let transaction = self.connection.transaction()?;
        transaction.execute("DELETE FROM page_commit_journal WHERE account_key=?1 AND task_id IN (SELECT task_id FROM downloads WHERE account_key=?1 AND episode_id=?2 AND revision=?3)", params![account,episode_id,revision])?;
        transaction.execute("DELETE FROM downloads WHERE account_key=?1 AND episode_id=?2 AND revision=?3", params![account,episode_id,revision])?;
        transaction.execute("DELETE FROM page_receipts WHERE account_key=?1 AND episode_id=?2 AND revision=?3", params![account,episode_id,revision])?;
        transaction.execute("DELETE FROM page_identity_history WHERE account_key=?1 AND episode_id=?2 AND revision=?3", params![account,episode_id,revision])?;
        transaction.commit()?;
        Ok(receipts)
    }

    pub fn remove_page_receipt(&self, account: &str, episode_id: &str, revision: &str, page_id: &str) -> Result<()> {
        self.invalidate_page(account, episode_id, revision, page_id)
    }

    /// Missing or corrupt files revoke their receipts before download recovery resumes.
    pub fn invalidate_page(&self, account: &str, episode_id: &str, revision: &str, page_id: &str) -> Result<()> {
        validate_key(account)?;
        let transaction = Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        invalidate_page_from(&transaction, account, episode_id, revision, page_id)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn storage_usage(&self, account: &str) -> Result<u64> {
        validate_key(account)?;
        let mut statement = self.connection.prepare("SELECT payload FROM page_receipts WHERE account_key=?1")?;
        let values = statement.query_map([account], |row| row.get::<_, String>(0))?;
        let mut by_digest = std::collections::BTreeMap::new();
        for value in values {
            let receipt: PageReceipt = from_json(&value?)?;
            by_digest.entry(receipt.sha256).or_insert(receipt.bytes);
        }
        by_digest.values().try_fold(0_u64, |sum, bytes| sum.checked_add(*bytes).context("Storage byte count overflow"))
    }

    /// Pending publications also keep an object alive until their journal is resolved.
    pub fn page_object_referenced(&self, account: &str, file_name: &str) -> Result<bool> {
        validate_key(account)?;
        Ok(self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM page_receipts WHERE account_key=?1 AND json_extract(payload,'$.file_name')=?2)
               OR EXISTS(SELECT 1 FROM page_commit_journal WHERE account_key=?1 AND json_extract(payload,'$.file_name')=?2)",
            params![account,file_name], |row| row.get::<_, bool>(0))?)
    }

    pub fn checkpoint(&self) -> Result<()> {
        self.connection.execute_batch("PRAGMA wal_checkpoint(PASSIVE);").context("Checkpoint metadata")
    }

    fn decorate_comic(&self, account: &str, mut comic: Comic) -> Result<Comic> {
        if let Some(anchor) = self.load_anchor(account, &comic.id)? {
            comic.last_episode_id = Some(anchor.episode_id.clone());
            comic.last_read_at = anchor.updated_at;
            if let Some(index) = self.load_index(account, &anchor.episode_id, Some(&anchor.revision))? {
                if !index.images.is_empty() {
                    comic.progress = if anchor.finished { 100 } else {
                        ((anchor.page_index.min(index.images.len() - 1) as f64 + anchor.source_y)
                            / index.images.len() as f64 * 100.0).round().clamp(0.0,100.0) as u32
                    };
                }
            }
        }
        Ok(comic)
    }
}

fn migrate_resource_schema(transaction: &Transaction<'_>) -> Result<()> {
    transaction.execute_batch(
        "CREATE INDEX IF NOT EXISTS page_objects_by_name ON page_receipts(account_key,json_extract(payload,'$.file_name'));
         CREATE INDEX IF NOT EXISTS pending_objects_by_name ON page_commit_journal(account_key,json_extract(payload,'$.file_name'));
         CREATE INDEX IF NOT EXISTS reader_downloads_terminal ON downloads(account_key,status,task_id)
             WHERE json_extract(payload,'$.retained')=0 AND COALESCE(json_extract(payload,'$.refresh_source'),0)=0;
         CREATE INDEX IF NOT EXISTS download_terminal_reconcile ON downloads(account_key,task_id)
             WHERE status IN ('completed','paused','failed','cancelled') AND json_type(payload,'$.terminal_event')='object';
         CREATE INDEX IF NOT EXISTS downloads_activity ON downloads(
             account_key,status,json_extract(payload,'$.retained'),json_extract(payload,'$.comic_id'));
         CREATE TABLE IF NOT EXISTS download_event_outbox (
             id INTEGER PRIMARY KEY AUTOINCREMENT, account_key TEXT NOT NULL, task_id TEXT NOT NULL,
             generation INTEGER NOT NULL CHECK(generation>=0), payload TEXT NOT NULL CHECK(json_valid(payload)),
             UNIQUE(account_key,task_id),
             FOREIGN KEY(account_key,task_id) REFERENCES downloads(account_key,task_id) ON DELETE CASCADE);
         CREATE INDEX IF NOT EXISTS download_events_order ON download_event_outbox(account_key,id);"
    ).context("Migrate bounded download event storage and object lookup indexes")?;
    Ok(())
}

fn validate_download_event(payload: &serde_json::Value) -> Result<&str> {
    let object = payload.as_object().context("A terminal event must be an object")?;
    let kind = object.get("kind").and_then(serde_json::Value::as_str).context("A terminal event kind is missing")?;
    match kind {
        "completed" | "prefetch_ready" | "paused" | "cancelled" => {
            ensure!(object.len() == 1, "A terminal event cannot contain acquisition metadata");
        },
        "failed" => {
            let code = object.get("code").and_then(serde_json::Value::as_str).context("A failure code is missing")?;
            ensure!(object.len() == 3 && !code.is_empty() && code.len() <= 256
                && code.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
                && object.get("retryable").and_then(serde_json::Value::as_bool).is_some(),
                "A failed event must contain only a bounded machine code and retry flag");
        },
        _ => anyhow::bail!("Only terminal download events may enter durable storage"),
    }
    Ok(kind)
}

fn event_matches_status(kind: &str, status: DownloadStatus) -> bool {
    matches!((kind,status),
        ("completed",DownloadStatus::Completed) | ("prefetch_ready" | "paused",DownloadStatus::Paused)
        | ("cancelled",DownloadStatus::Cancelled) | ("failed",DownloadStatus::Failed))
}

fn upsert_comic(transaction: &Transaction<'_>, account: &str, comic: &Comic) -> Result<()> {
    validate_key(&comic.id)?;
    transaction.execute(
        "INSERT INTO comics(account_key,comic_id,payload,updated_at,last_read_at) VALUES(?1,?2,?3,?4,?5)
         ON CONFLICT(account_key,comic_id) DO UPDATE SET payload=excluded.payload,updated_at=excluded.updated_at,last_read_at=MAX(comics.last_read_at,excluded.last_read_at)",
        params![account,comic.id,to_json(comic)?,comic.updated_at,comic.last_read_at])?;
    Ok(())
}

fn validate_key(value: &str) -> Result<()> {
    ensure!(!value.is_empty() && value.len() <= 512 && !value.chars().any(char::is_control), "Invalid metadata identity");
    Ok(())
}

fn validate_index(index: &ImageIndex) -> Result<()> {
    validate_key(&index.episode_id)?;
    validate_key(&index.revision)?;
    ensure!(!index.images.is_empty() && index.images.len() <= 10_000, "Invalid source page count");
    let mut ids = HashSet::new();
    for (position, page) in index.images.iter().enumerate() {
        validate_key(&page.id)?;
        ensure!(page.index == position && ids.insert(page.id.as_str()), "Invalid source page ordering");
        ensure!(page.width > 0 && page.height > 0, "Invalid source page dimensions");
    }
    Ok(())
}

fn validate_download(record: &DownloadRecord) -> Result<()> {
    validate_key(&record.id)?;
    validate_key(&record.comic_id)?;
    validate_key(&record.episode_id)?;
    validate_key(&record.revision)?;
    ensure!(record.generation <= i64::MAX as u64, "Invalid download generation");
    ensure!(record.priority <= i64::MAX as u64, "Invalid download priority");
    ensure!(record.total_pages <= 10_000 && record.completed_pages <= record.total_pages,
        "Invalid download page counts");
    ensure!(record.status != DownloadStatus::Completed || record.total_pages > 0 && record.completed_pages == record.total_pages,
        "An incomplete download cannot be marked complete");
    ensure!(!record.refresh_source || record.retained && record.status != DownloadStatus::Completed,
        "A pending source refresh must remain an incomplete retained download");
    if let Some(payload) = record.terminal_event.as_ref() {
        let kind = validate_download_event(payload)?;
        ensure!(event_matches_status(kind, record.status), "The terminal notification does not match the download state");
    }
    ensure!(record.error_code.as_ref().is_none_or(|value| value.len() <= 256 && !value.chars().any(char::is_control)), "Invalid download error code");
    Ok(())
}

pub fn validate_receipt(receipt: &PageReceipt) -> Result<()> {
    validate_key(&receipt.episode_id)?;
    validate_key(&receipt.revision)?;
    validate_key(&receipt.page_id)?;
    ensure!(receipt.sha256.len() == 64 && receipt.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()), "Invalid page digest");
    let mut parts = receipt.file_name.split('.');
    let digest = parts.next().unwrap_or_default();
    ensure!(digest.eq_ignore_ascii_case(&receipt.sha256), "The object name does not match its digest");
    if let Some(extension) = parts.next() {
        ensure!(!extension.is_empty() && extension.len() <= 8 && extension.bytes().all(|byte| byte.is_ascii_alphanumeric()), "Invalid page object extension");
    }
    ensure!(parts.next().is_none() && receipt.bytes > 0 && receipt.bytes <= MAX_RECEIPT_BYTES
        && receipt.width > 0 && receipt.height > 0 && receipt.decode_validation_version <= VALIDATION_VERSION,
        "Invalid page receipt");
    Ok(())
}

fn to_json<T: Serialize>(value: &T) -> Result<String> { serde_json::to_string(value).context("Serialize metadata") }
fn from_json<T: DeserializeOwned>(value: &str) -> Result<T> { serde_json::from_str(value).context("Read persisted metadata") }
fn unix_seconds() -> i64 { SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |value| value.as_secs().min(i64::MAX as u64) as i64) }

fn load_download_from(connection: &Connection, account: &str, task_id: &str) -> Result<Option<DownloadRecord>> {
    let value: Option<String> = connection.query_row(
        "SELECT payload FROM downloads WHERE account_key=?1 AND task_id=?2",
        params![account,task_id], |row| row.get(0)).optional()?;
    value.map(|value| from_json(&value)).transpose()
}

fn load_page_receipt_from(connection: &Connection, account: &str, episode_id: &str, revision: &str, page_id: &str) -> Result<Option<PageReceipt>> {
    let value: Option<String> = connection.query_row(
        "SELECT payload FROM page_receipts WHERE account_key=?1 AND episode_id=?2 AND revision=?3 AND page_id=?4",
        params![account,episode_id,revision,page_id], |row| row.get(0)).optional()?;
    value.map(|value| from_json(&value)).transpose()
}

/// Retention and verification timestamps may change without replacing an object.
fn same_receipt_object(left: &PageReceipt, right: &PageReceipt) -> bool {
    left.episode_id == right.episode_id && left.revision == right.revision && left.page_id == right.page_id
        && left.page_index == right.page_index && left.file_name == right.file_name && left.sha256 == right.sha256
        && left.bytes == right.bytes && left.width == right.width && left.height == right.height
}

fn validate_decode_proof(receipt: &PageReceipt, proof: &DecodedImageProof) -> Result<()> {
    validate_receipt(receipt)?;
    ensure!(proof.validation_version() == VALIDATION_VERSION && proof.sha256().eq_ignore_ascii_case(&receipt.sha256)
        && proof.byte_len() == receipt.bytes && proof.width() == receipt.width && proof.height() == receipt.height,
        "The complete decode proof does not match the page receipt");
    Ok(())
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ReceiptProgress { pages: u32, bytes: u64 }

/// Committed histories hold the same immutable byte counts as their receipts.
/// SQL visits descriptor identities without materializing every receipt in Rust.
fn receipt_progress_from(
    connection: &Connection, account: &str, episode_id: &str, revision: &str, indexed: bool,
) -> Result<ReceiptProgress> {
    let sql = if indexed {
        "SELECT COUNT(*),COUNT(h.page_id),
             COALESCE(SUM(CASE WHEN typeof(h.bytes)='integer' AND h.bytes BETWEEN 1 AND ?4 THEN h.bytes ELSE 0 END),0),
             COALESCE(SUM(CASE WHEN typeof(h.bytes)='integer' AND h.bytes BETWEEN 1 AND ?4 THEN 0 ELSE 1 END),0)
         FROM image_indices i CROSS JOIN json_each(i.payload,'$.images') descriptor CROSS JOIN page_receipts p
         LEFT JOIN page_identity_history h ON h.account_key=p.account_key AND h.episode_id=p.episode_id
             AND h.revision=p.revision AND h.page_id=p.page_id
         WHERE i.account_key=?1 AND i.episode_id=?2 AND i.revision=?3
             AND p.account_key=i.account_key AND p.episode_id=i.episode_id AND p.revision=i.revision
             AND p.page_index=CAST(descriptor.key AS INTEGER) AND p.page_id=json_extract(descriptor.value,'$.id')"
    } else {
        "SELECT COUNT(*),COUNT(h.page_id),
             COALESCE(SUM(CASE WHEN typeof(h.bytes)='integer' AND h.bytes BETWEEN 1 AND ?4 THEN h.bytes ELSE 0 END),0),
             COALESCE(SUM(CASE WHEN typeof(h.bytes)='integer' AND h.bytes BETWEEN 1 AND ?4 THEN 0 ELSE 1 END),0)
         FROM page_receipts p LEFT JOIN page_identity_history h ON h.account_key=p.account_key AND h.episode_id=p.episode_id
             AND h.revision=p.revision AND h.page_id=p.page_id
         WHERE p.account_key=?1 AND p.episode_id=?2 AND p.revision=?3"
    };
    let (pages, histories, bytes, invalid_bytes): (i64, i64, i64, i64) = connection.query_row(
        sql, params![account,episode_id,revision,MAX_RECEIPT_BYTES as i64],
        |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)))?;
    ensure!((0..=10_000).contains(&pages) && histories == pages && invalid_bytes == 0 && bytes >= 0,
        "The committed page aggregate has invalid content history");
    Ok(ReceiptProgress { pages: u32::try_from(pages)?, bytes: u64::try_from(bytes)? })
}

fn invalidate_page_from(transaction: &Transaction<'_>, account: &str, episode_id: &str, revision: &str, page_id: &str) -> Result<()> {
    transaction.execute("DELETE FROM page_receipts WHERE account_key=?1 AND episode_id=?2 AND revision=?3 AND page_id=?4", params![account,episode_id,revision,page_id])?;
    transaction.execute(
        "DELETE FROM page_commit_journal WHERE account_key=?1 AND page_id=?4
         AND task_id IN (SELECT task_id FROM downloads WHERE account_key=?1 AND episode_id=?2 AND revision=?3)",
        params![account,episode_id,revision,page_id])?;
    let progress = receipt_progress_from(transaction, account, episode_id, revision, false)?;
    let mut statement = transaction.prepare("SELECT payload FROM downloads WHERE account_key=?1 AND episode_id=?2 AND revision=?3")?;
    let values = statement.query_map(params![account,episode_id,revision], |row| row.get::<_, String>(0))?;
    let mut records: Vec<DownloadRecord> = values.map(|value| from_json(&value?)).collect::<Result<_>>()?;
    drop(statement);
    for record in &mut records {
        record.completed_pages = progress.pages;
        record.bytes = progress.bytes;
        let mut changed_terminal = false;
        if record.status == DownloadStatus::Completed && record.completed_pages < record.total_pages {
            record.status = DownloadStatus::Failed;
            record.error_code = Some("local_page_missing".into());
            record.terminal_event = Some(serde_json::json!({"kind":"failed","code":"local_page_missing","retryable":true}));
            changed_terminal = true;
        }
        transaction.execute("UPDATE downloads SET status=?3,payload=?4 WHERE account_key=?1 AND task_id=?2",
            params![account,record.id,record.status.as_str(),to_json(record)?])?;
        if changed_terminal {
            transaction.execute("DELETE FROM download_event_outbox WHERE account_key=?1 AND task_id=?2", params![account,record.id])?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn set_private_permissions(path: &Path, directory: bool) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    match fs::set_permissions(path, fs::Permissions::from_mode(if directory { 0o700 } else { 0o600 })) {
        Ok(()) => Ok(()),
        Err(error) if permission_mode_unavailable(path, &error) => Ok(()),
        Err(error) => Err(error).context("Protect metadata permissions"),
    }
}

#[cfg(not(unix))]
fn set_private_permissions(_: &Path, _: bool) -> Result<()> { Ok(()) }

#[cfg(target_os = "linux")]
fn permission_mode_unavailable(path: &Path, error: &std::io::Error) -> bool {
    use std::{ffi::CString, mem::MaybeUninit, os::unix::ffi::OsStrExt};
    if !matches!(error.raw_os_error(), Some(libc::EPERM | libc::EOPNOTSUPP)) { return false; }
    let Ok(path) = CString::new(path.as_os_str().as_bytes()) else { return false; };
    let mut information = MaybeUninit::<libc::statfs>::uninit();
    // The path is NUL-terminated and the kernel initializes this statfs structure on success.
    if unsafe { libc::statfs(path.as_ptr(), information.as_mut_ptr()) } != 0 { return false; }
    let kind = unsafe { information.assume_init() }.f_type as u64;
    kind == 0x4d44 || kind == 0x2011_bab0
}

#[cfg(all(unix, not(target_os = "linux")))]
fn permission_mode_unavailable(_: &Path, _: &std::io::Error) -> bool { false }

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::PageDescriptor;

    fn index() -> ImageIndex {
        ImageIndex { episode_id: "episode".into(), revision: "revision".into(),
            images: vec![PageDescriptor { id: "page".into(), index: 0, width: 1200, height: 900 }],
            paths: vec!["https://temporary.invalid/token".into()], extra: serde_json::json!({"token":"private"}) }
    }

    #[test]
    fn account_namespaces_do_not_share_metadata_or_private_transport_state() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let mut store = Store::open(directory.path().join("metadata.sqlite3"))?;
        store.save_comics("first", &[Comic { id: "comic".into(), title: "First account".into(), ..Default::default() }])?;
        store.save_index("first", &index())?;
        assert!(store.load_comics("second")?.is_empty());
        assert!(store.load_index("second", "episode", None)?.is_none());
        let stored = store.load_index("first", "episode", None)?.unwrap();
        assert!(stored.paths.is_empty());
        assert!(stored.extra.is_null());
        Ok(())
    }

    #[test]
    fn a_page_commit_updates_receipt_task_and_journal_together() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let mut store = Store::open(directory.path().join("metadata.sqlite3"))?;
        store.save_index("account", &index())?;
        store.save_download("account", &DownloadRecord { id: "task".into(), comic_id: "comic".into(),
            episode_id: "episode".into(), revision: "revision".into(), total_pages: 1, generation: 1,
            status: DownloadStatus::Running, ..Default::default() })?;
        let digest = "a".repeat(64);
        let receipt = PageReceipt { episode_id: "episode".into(), revision: "revision".into(), page_id: "page".into(),
            file_name: format!("{digest}.png"), sha256: digest, bytes: 42, width: 1200, height: 900,
            retained: true, verified_at: 20, ..Default::default() };
        store.begin_page_commit("account", "task", &receipt)?;
        assert_eq!(store.pending_page_commits("account")?.len(), 1);
        store.commit_page("account", "task", &receipt)?;
        assert!(store.pending_page_commits("account")?.is_empty());
        let task = store.load_download("account", "task")?.unwrap();
        assert_eq!(task.status, DownloadStatus::Running);
        assert_eq!((task.completed_pages, task.total_pages), (1, 1));
        assert!(store.save_download("account", &DownloadRecord { status: DownloadStatus::Completed, ..task }).is_err());
        assert!(store.page_receipt("account", "episode", "revision", "page")?.is_some());
        Ok(())
    }

    #[test]
    fn source_anchor_survives_restart_without_using_a_screen_percentage() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("metadata.sqlite3");
        let anchor = ReadingAnchor { episode_id: "episode".into(), revision: "revision".into(), page_id: "page".into(),
            page_index: 0, source_x: 0.25, source_y: 0.7, zoom_ratio: 1.5, updated_at: 100, ..Default::default() };
        Store::open(&database)?.save_anchor("account", "comic", &anchor)?;
        assert_eq!(Store::open(&database)?.load_anchor("account", "comic")?, Some(anchor));
        Ok(())
    }

    #[test]
    fn a_paused_generation_rejects_a_page_that_finished_late() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("metadata.sqlite3");
        let mut worker = Store::open(&database)?;
        let control = Store::open(&database)?;
        worker.save_index("account", &index())?;
        let mut task = DownloadRecord { id: "task".into(), comic_id: "comic".into(), episode_id: "episode".into(),
            revision: "revision".into(), total_pages: 1, generation: 1, status: DownloadStatus::Running, ..Default::default() };
        worker.save_download("account", &task)?;
        let digest = "b".repeat(64);
        let receipt = PageReceipt { episode_id: "episode".into(), revision: "revision".into(), page_id: "page".into(),
            file_name: digest.clone(), sha256: digest, bytes: 42, width: 1200, height: 900, ..Default::default() };
        worker.begin_page_commit_generation("account", "task", 1, &receipt)?;
        task.generation = 2;
        task.status = DownloadStatus::Paused;
        control.save_download("account", &task)?;
        assert!(worker.commit_page_generation("account", "task", 1, &receipt).is_err());
        assert!(worker.page_receipt("account", "episode", "revision", "page")?.is_none());
        assert_eq!(worker.load_download("account", "task")?.unwrap().status, DownloadStatus::Paused);
        task.generation = 1;
        task.status = DownloadStatus::Running;
        assert!(worker.save_download("account", &task).is_err());
        Ok(())
    }

    #[test]
    fn pending_tasks_bind_the_indexed_revision_once() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let mut store = Store::open(directory.path().join("metadata.sqlite3"))?;
        let mut task = DownloadRecord { id: "task".into(), comic_id: "comic".into(), episode_id: "episode".into(),
            revision: PENDING_REVISION.into(), generation: 1, status: DownloadStatus::Queued, ..Default::default() };
        store.save_download("account", &task)?;
        store.save_index("account", &index())?;
        task.revision = "revision".into();
        task.status = DownloadStatus::Running;
        task.total_pages = 1;
        store.save_download("account", &task)?;
        let column: String = store.connection.query_row("SELECT revision FROM downloads WHERE account_key='account' AND task_id='task'", [], |row| row.get(0))?;
        assert_eq!(column, "revision");
        task.revision = "different-revision".into();
        assert!(store.save_download("account", &task).is_err());
        store.remove_episode("account", "episode", "revision")?;
        assert!(store.load_download("account", "task")?.is_none());
        Ok(())
    }

    #[test]
    fn invalidation_preserves_the_historical_content_identity() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let mut store = Store::open(directory.path().join("metadata.sqlite3"))?;
        store.save_index("account", &index())?;
        let mut task = DownloadRecord { id: "task".into(), comic_id: "comic".into(), episode_id: "episode".into(),
            revision: "revision".into(), total_pages: 1, generation: 1, status: DownloadStatus::Running, ..Default::default() };
        store.save_download("account", &task)?;
        let digest = "c".repeat(64);
        let original = PageReceipt { episode_id: "episode".into(), revision: "revision".into(), page_id: "page".into(),
            file_name: digest.clone(), sha256: digest, bytes: 42, width: 1200, height: 900, retained: true, ..Default::default() };
        store.begin_page_commit_generation("account", "task", 1, &original)?;
        store.commit_page_generation("account", "task", 1, &original)?;
        store.invalidate_page("account", "episode", "revision", "page")?;
        let invalidated = store.load_download("account", "task")?.unwrap();
        assert_eq!(invalidated.status, DownloadStatus::Running);
        assert_eq!(invalidated.completed_pages, 0);
        task.generation = 2;
        store.save_download("account", &task)?;
        let digest = "d".repeat(64);
        let changed = PageReceipt { file_name: digest.clone(), sha256: digest, ..original.clone() };
        store.begin_page_commit_generation("account", "task", 2, &changed)?;
        assert!(store.commit_page_generation("account", "task", 2, &changed).is_err());
        assert!(store.page_receipt("account", "episode", "revision", "page")?.is_none());
        store.begin_page_commit_generation("account", "task", 2, &original)?;
        store.commit_page_generation("account", "task", 2, &original)?;
        assert_eq!(store.load_download("account", "task")?.unwrap().status, DownloadStatus::Running);
        assert!(store.page_object_referenced("account", &original.file_name)?);
        assert!(!store.page_object_referenced("different-account", &original.file_name)?);
        Ok(())
    }
}

#[cfg(test)]
#[path = "store/integrity_tests.rs"]
mod integrity_tests;

#[cfg(test)]
#[path = "store/resource_tests.rs"]
mod resource_tests;

#[cfg(test)]
#[path = "store/sleep_checkpoint_tests.rs"]
mod sleep_checkpoint_tests;
