use super::*;
use crate::{
    domain::EpisodeAccess,
    source::{PurchaseMethod, PurchaseRequest, PurchaseScope, QuoteScope, RechargeConfig, RechargeOrder, SourceError},
    ui::{PurchaseRangeItem, PurchaseReceiptItem},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::sync::{Mutex, OnceLock, atomic::{AtomicBool, AtomicU64, Ordering}};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

const QUOTE_MAX_AGE: i64 = 120;
const JOURNAL_KEY: &str = "purchase_journal";
const RECHARGE_KEY: &str = "recharge_intent";
const MAX_IMPORT_BYTES: u64 = 128 * 1024;

pub(super) struct AccountState {
    section: String,
    wallet: Option<Value>,
    wallet_at: i64,
    wallet_attempted: bool,
    preview_wallet_refreshed: bool,
    preview_cache_cleared: bool,
    busy: bool,
    message: String,
    comic: Option<Comic>,
    episode: Option<Episode>,
    selection: String,
    payment: String,
    phase: String,
    quote_loading: bool,
    quote: Option<OfferSnapshot>,
    quote_read: Option<QuoteRead>,
    quote_error: String,
    journals: Vec<PurchaseIntent>,
    current_journal: Option<String>,
    reconciling: bool,
    storage: Option<StorageSummary>,
    storage_pending: bool,
    storage_checked: Option<Instant>,
    outbox_checked: Option<Instant>,
    outbox_generation: u64,
    recharge: RechargeUi,
}

impl Default for AccountState {
    fn default() -> Self {
        Self {
            section: "account".into(), wallet: None, wallet_at: 0, wallet_attempted: false,
            preview_wallet_refreshed: false, preview_cache_cleared: false,
            busy: false, message: String::new(), comic: None, episode: None,
            selection: "single".into(), payment: "coin".into(), phase: "review".into(),
            quote_loading: false, quote: None, quote_read: None, quote_error: String::new(),
            journals: Vec::new(), current_journal: None, reconciling: false,
            storage: None, storage_pending: false, storage_checked: None,
            outbox_checked: None, outbox_generation: OUTBOX_GENERATION.load(Ordering::Acquire),
            recharge: RechargeUi::default(),
        }
    }
}

impl AccountState {
    /// Keep a newly activated account visibly empty when preparation cannot finish.
    pub(super) fn failed() -> Self {
        Self { phase: "error".into(), message: i18n::text("common_storage_error"), ..Self::default() }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct WalletSnapshot { value: Value, observed_at: i64 }

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PurchaseIntent {
    id: String,
    account: String,
    quote: OfferSnapshot,
    state: String,
    created_at: i64,
    updated_at: i64,
    attempts: u8,
    acknowledgement: Option<Value>,
    error_code: Option<String>,
    entitlement_observed: bool,
    #[serde(default)]
    persistence_pending: bool,
}

impl PurchaseIntent {
    fn unresolved(&self) -> bool {
        self.persistence_pending || matches!(self.state.as_str(), "submitting" | "unknown" | "acknowledged")
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct RechargeIntent {
    id: String,
    account: String,
    amount_cents: u64,
    fingerprint: String,
    state: String,
    created_at: i64,
    updated_at: i64,
    order: Option<RechargeOrder>,
    reconciliation_id: Option<String>,
    error_code: Option<String>,
    #[serde(default)]
    persistence_pending: bool,
}

#[derive(Default)]
struct RechargeUi {
    visible: bool,
    phase: String,
    title: String,
    config: Option<RechargeConfig>,
    selected: usize,
    journal: Option<RechargeIntent>,
    qr: slint::Image,
    message: String,
    rows: Vec<String>,
    busy: bool,
}

#[derive(Clone, Default)]
struct StorageSummary {
    bytes: u64,
    manual_bytes: u64,
    cache_bytes: u64,
    free_bytes: Option<u64>,
    volume_bytes: Option<u64>,
}

/// Account startup data contains no UI object and can move from the preparation worker.
pub(super) struct PreparedAccount {
    wallet: Option<Value>,
    wallet_at: i64,
    journals: Vec<PurchaseIntent>,
    recharge: Option<RechargeIntent>,
}

impl PreparedAccount {
    pub(super) fn load(store: &Store, account: &str, demo: Option<&DemoData>) -> Result<Self> {
        let (mut wallet, mut wallet_at) = match store.preference::<WalletSnapshot>(account, "wallet")? {
            Some(snapshot) => (Some(snapshot.value), snapshot.observed_at),
            None => (None, 0),
        };
        let mut journals: Vec<PurchaseIntent> = store.preference(account, JOURNAL_KEY)?.unwrap_or_default();
        let mut changed = false;
        for intent in &mut journals {
            if intent.state == "submitting" {
                intent.state = "unknown".into();
                intent.error_code = Some("interrupted_after_dispatch_intent".into());
                changed = true;
            }
        }
        if changed { journals = merge_purchase_journal(store.path(), account, &journals)?; }
        let mut recharge = store.preference::<RechargeIntent>(account, RECHARGE_KEY)?;
        if let Some(intent) = &mut recharge {
            if intent.state == "creating" {
                intent.state = "unknown".into();
                intent.error_code = Some("interrupted_order_creation".into());
                *intent = merge_recharge_intent(store.path(), intent)?;
            }
        }
        if let Some(demo) = demo {
            let account = &demo.fixture["account"];
            wallet = Some(json!({
                "remain_gold": account.get("balance"), "remain_coupon": account.get("coupon_count"),
            }));
            wallet_at = 0;
        }
        Ok(Self { wallet, wallet_at, journals, recharge })
    }
}

#[derive(Clone)]
enum PendingPayment {
    Purchase { database: PathBuf, intent: PurchaseIntent, result: source::Result<Value> },
    Recharge { database: PathBuf, intent: RechargeIntent, result: source::Result<RechargeOrder> },
}

static PAYMENT_OUTBOX: OnceLock<Mutex<BTreeMap<String, PendingPayment>>> = OnceLock::new();
static OUTBOX_GENERATION: AtomicU64 = AtomicU64::new(1);
static OUTBOX_RETRY_ACTIVE: AtomicBool = AtomicBool::new(false);

struct OutboxRetryGuard;
impl Drop for OutboxRetryGuard {
    fn drop(&mut self) { OUTBOX_RETRY_ACTIVE.store(false, Ordering::Release); }
}

impl App {
    pub(super) fn tick_account(&mut self) -> Result<()> {
        let generation = OUTBOX_GENERATION.load(Ordering::Acquire);
        if self.account_ui.outbox_generation != generation {
            match self.store.preference::<Vec<PurchaseIntent>>(&self.account, JOURNAL_KEY) {
                Ok(Some(journals)) => {
                    self.account_ui.journals = journals;
                    if let Some(intent) = self.current_purchase_intent() {
                        if intent.state == "confirmed" && !intent.persistence_pending { self.account_ui.phase = "done".into(); }
                    }
                }
                Ok(None) => {},
                Err(_) => self.toast("common_storage_error"),
            }
            if let Ok(Some(intent)) = self.store.preference::<RechargeIntent>(&self.account, RECHARGE_KEY) {
                if self.account_ui.recharge.visible && self.account_ui.recharge.phase != "history" {
                    self.account_ui.recharge.phase = match intent.state.as_str() { "qr" | "observed" => "qr", "rejected" => "error", _ => "pending" }.into();
                    if let Some(order) = &intent.order { self.account_ui.recharge.qr = super::auth::qr_image(&order.code_url).unwrap_or_default(); }
                }
                self.account_ui.recharge.journal = Some(intent);
            }
            self.account_ui.outbox_generation = generation;
            self.dirty = true;
        }
        if self.account_ui.outbox_checked.is_none_or(|value| value.elapsed() >= Duration::from_secs(5)) {
            self.account_ui.outbox_checked = Some(Instant::now());
            if payment_outbox_has_entries() && OUTBOX_RETRY_ACTIVE.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_ok() {
                self.handle.spawn_blocking(|| { let _guard = OutboxRetryGuard; retry_payment_outbox(); });
            }
        }
        Ok(())
    }

    pub(super) fn flush_account_receipts(&mut self) -> Result<()> {
        retry_payment_outbox();
        anyhow::ensure!(!payment_outbox_has_entries(), "Observed payment results still need local storage");
        Ok(())
    }

    pub(super) fn account_receipts_pending(&self) -> bool {
        payment_outbox_has_entries()
    }

    pub(super) fn account_wallet_labels(&self) -> (String, String) {
        let label = |field: &str| self.account_ui.wallet.as_ref().and_then(|value| amount(value.get(field)))
            .map(display_amount).unwrap_or_else(|| i18n::text("common_unknown"));
        (label("remain_gold"), label("remain_coupon"))
    }
    pub(super) fn initialize_account(&mut self) -> Result<()> {
        let prepared = PreparedAccount::load(&self.store, &self.account, self.demo.as_deref())?;
        self.initialize_account_prepared(prepared)
    }

    pub(super) fn initialize_account_prepared(&mut self, prepared: PreparedAccount) -> Result<()> {
        self.account_ui = AccountState::default();
        self.account_ui.wallet = prepared.wallet;
        self.account_ui.wallet_at = prepared.wallet_at;
        self.account_ui.journals = prepared.journals;
        self.account_ui.recharge.journal = prepared.recharge;
        if let Some(demo) = &self.demo {
            let defaults = &demo.fixture["account"]["defaults"];
            if let Some(value) = defaults["reading_mode"].as_str().filter(|value| ["page", "panel", "strip"].contains(value)) { self.preferences.mode = value.into(); }
            if let Some(value) = defaults["direction"].as_str().filter(|value| ["ltr", "rtl"].contains(value)) { self.preferences.direction = value.into(); }
            if let Some(value) = defaults["refresh"].as_str().filter(|value| ["smart", "0", "1", "6", "12"].contains(value)) { self.preferences.every = value.into(); }
            if let Some(value) = defaults["launch"].as_str().filter(|value| ["resume", "shelf"].contains(value)) { self.preferences.launch = value.into(); }
            if let Some(value) = defaults["sleep_cover"].as_str() { self.preferences.sleep_cover = value == "cover"; }
        }
        if self.demo.is_none() {
            self.preferences.smart = false;
            self.preferences.sleep_cover = false;
            self.preferences.orientation = "portrait".into();
        }
        self.platform.set_flash_on_navigation(self.preferences.flashui);
        let interval = match self.preferences.every.as_str() { "0" => 0, "1" => 1, "12" => 12, _ => 6 };
        self.platform.set_reader_refresh_interval(interval)?;
        self.schedule_storage_summary();
        Ok(())
    }

    pub(super) fn refresh_account_view(&mut self, ui: &AppWindow) -> Result<()> {
        let session = self.client.session();
        let signed_in = self.demo.is_some() || session.as_ref().and_then(AuthSession::account_key).is_some();
        if self.route == "account" && signed_in && !self.account_ui.wallet_attempted
            && self.account_ui.wallet.is_none() && !self.options.offline {
            self.refresh_wallet();
        }
        if self.route == "account" && self.account_ui.storage_checked.is_none_or(|value| value.elapsed() > Duration::from_secs(30)) {
            self.schedule_storage_summary();
        }
        let account = ui.global::<AccountModel>();
        account.set_section(self.account_ui.section.clone().into());
        account.set_signed_in(signed_in);
        let name = self.demo.as_ref().map(|demo| demo.account_name.clone())
            .or_else(|| session.as_ref().map(|session| session.account_name().to_owned())).unwrap_or_default();
        account.set_initial(name.chars().next().map(|value| value.to_string()).unwrap_or_default().into());
        account.set_username(name.into());
        account.set_session_status(tr(if self.demo.is_some() { "acct_preview_session" } else { "acct_session_active" }));
        account.set_balance(self.account_ui.wallet.as_ref().and_then(|value| amount(value.get("remain_gold")))
            .map(display_amount).unwrap_or_else(|| i18n::text("common_unknown")).into());
        account.set_coupons(self.account_ui.wallet.as_ref().and_then(|value| amount(value.get("remain_coupon")))
            .map(display_amount).unwrap_or_else(|| i18n::text("common_unknown")).into());
        account.set_balance_time(self.wallet_time().into());
        account.set_busy(self.account_ui.busy);
        account.set_message(self.account_ui.message.clone().into());
        account.set_mode(self.preferences.mode.clone().into());
        account.set_direction(self.preferences.direction.clone().into());
        account.set_every(self.preferences.every.clone().into());
        account.set_landscape(self.preferences.landscape.clone().into());
        account.set_smart(if self.preferences.smart { "on" } else { "off" }.into());
        account.set_ahead(self.preferences.ahead as i32);
        account.set_concurrency(self.preferences.concurrency as i32);
        account.set_preload(self.preferences.preload as i32);
        account.set_wifi(self.preferences.wifi.clone().into());
        account.set_cache_limit(self.preferences.cache_limit as i32);
        account.set_launch(self.preferences.launch.clone().into());
        account.set_sleep_cover(if self.preferences.sleep_cover { "cover" } else { "system" }.into());
        account.set_flashui(if self.preferences.flashui { "on" } else { "off" }.into());
        account.set_orientation(self.preferences.orientation.clone().into());
        account.set_can_wifi(self.demo.is_some());
        account.set_can_orient(self.demo.is_some());
        account.set_can_sleep_cover(self.demo.is_some());
        account.set_can_smart(self.demo.is_some());
        account.set_can_update(self.demo.is_some());
        account.set_version(env!("CARGO_PKG_VERSION").into());
        let (width, height) = self.options.profile.physical_size();
        account.set_device(format!("{} · {width}×{height}", self.options.profile.name()).into());
        account.set_device_short(self.options.profile.name().trim_start_matches("Kindle ").replace(" (2022 / 2024)", "").into());
        account.set_firmware(std::env::var("BILICOMICS_APPROVED_FIRMWARE")
            .unwrap_or_else(|_| i18n::text("common_unknown")).into());
        account.set_data_directory(self.data_dir.display().to_string().into());
        account.set_last_purchase(self.account_ui.journals.last().map(|intent| {
            format!("{} · {}", intent.quote.expected_ids.len(), journal_label(&intent.state))
        }).unwrap_or_else(|| i18n::text("acct_no_records")).into());
        let unknown = i18n::text("common_unknown");
        if let Some(storage) = &self.account_ui.storage {
            account.set_storage_used(byte_label(storage.bytes).into());
            account.set_storage_free(storage.free_bytes.map(byte_label).unwrap_or_else(|| unknown.clone()).into());
            account.set_manual_used(byte_label(storage.manual_bytes).into());
            account.set_cache_used(byte_label(storage.cache_bytes).into());
            account.set_cache_fill((storage.cache_bytes as f64 / (u64::from(self.preferences.cache_limit) * 1024 * 1024) as f64).clamp(0.0, 1.0) as f32);
            account.set_storage_manual_fill(storage.volume_bytes.filter(|bytes| *bytes > 0).map(|total| (storage.manual_bytes as f64 / total as f64).clamp(0.0, 1.0) as f32).unwrap_or_default());
            account.set_storage_cache_fill(storage.volume_bytes.filter(|bytes| *bytes > 0).map(|total| (storage.cache_bytes as f64 / total as f64).clamp(0.0, 1.0) as f32).unwrap_or_default());
        } else {
            account.set_storage_used(unknown.clone().into()); account.set_storage_free(unknown.clone().into());
            account.set_manual_used(unknown.clone().into()); account.set_cache_used(unknown.into());
            account.set_cache_fill(0.0);
            account.set_storage_manual_fill(0.0); account.set_storage_cache_fill(0.0);
        }
        account.set_cache_note(tr("acct_cache_preserve_retained"));
        if let Some(demo) = &self.demo {
            let presentation = &demo.fixture["account"]["presentation"];
            if let Some(value) = presentation[if self.is_scribe() { "storage_used_sc" } else { "storage_used_pw" }].as_str() { account.set_storage_used(value.into()); }
            if let Some(value) = presentation[if self.is_scribe() { "free_space_sc" } else { "free_space_pw" }].as_str() { account.set_storage_free(value.into()); }
            if let Some(value) = presentation["manual_used"].as_str() { account.set_manual_used(value.into()); }
            let cache_used = if self.account_ui.preview_cache_cleared { "0 MB".to_owned() }
                else { presentation["cache_used"].as_str().unwrap_or_default().split(" / ").next().unwrap_or_default().to_owned() };
            account.set_cache_used(format!("{cache_used} / {}", if self.preferences.cache_limit >= 1024 { format!("{} GB", self.preferences.cache_limit / 1024) } else { format!("{} MB", self.preferences.cache_limit) }).into());
            account.set_storage_manual_fill(presentation["manual_pct"].as_f64().unwrap_or_default().clamp(0.0, 1.0) as f32);
            account.set_storage_cache_fill(if self.account_ui.preview_cache_cleared { 0.0 } else { presentation["cache_pct"].as_f64().unwrap_or_default().clamp(0.0, 1.0) as f32 });
            if let Some(value) = presentation["last_purchase"].as_str() { account.set_last_purchase(value.into()); }
            if let Some(value) = presentation["firmware"].as_str() { account.set_firmware(value.into()); }
            if let Some(value) = presentation["data_directory"].as_str() { account.set_data_directory(value.into()); }
            account.set_cache_note(tr(if self.account_ui.preview_cache_cleared { "acct_preview_cache_released" } else { "acct_cache_preview_note" }));
        }
        self.refresh_recharge_view(ui)?;
        Ok(())
    }

    pub(super) fn account_action(&mut self, action: &str, value: &str) -> Result<()> {
        match action {
            "section" => {
                if ["account", "reading", "download", "storage", "system", "about"].contains(&value) {
                    self.account_ui.section = value.into(); self.account_ui.message.clear();
                }
            }
            "back" => self.account_ui.section = "account".into(),
            "login" => { self.auth_return = "account".into(); self.navigate("login"); }
            "logout" => self.logout()?,
            "refresh-wallet" => { if self.require_session() && self.require_network("account", action, value) { self.refresh_wallet(); } }
            "recharge" => self.open_recharge(),
            "purchase-history" => self.open_purchase_history(),
            "recharge-history" => self.open_recharge_history(),
            "import-session" => self.import_account_session(value),
            "mode" if ["page", "panel", "strip"].contains(&value) => self.preferences.mode = value.into(),
            "direction" if ["ltr", "rtl"].contains(&value) => self.preferences.direction = value.into(),
            "every" if ["smart", "0", "1", "6", "12"].contains(&value) => {
                self.preferences.every = value.into();
                self.platform.set_reader_refresh_interval(match value { "0" => 0, "1" => 1, "12" => 12, _ => 6 })?;
            }
            "landscape" if ["spread", "single"].contains(&value) => self.preferences.landscape = value.into(),
            "smart" if ["on", "off"].contains(&value) => {
                if self.demo.is_some() { self.preferences.smart = value == "on"; }
                else { self.toast("acct_manual_download_only"); }
            }
            "ahead" => {
                if self.demo.is_some() { if let Ok(count) = value.parse::<u32>() { if [1, 2, 3, 5].contains(&count) { self.preferences.ahead = count; } } }
                else { self.toast("acct_manual_download_only"); }
            }
            "concurrency" => {
                if let Ok(count) = value.parse::<u32>() {
                    if (1..=4).contains(&count) {
                        if let Some(manager) = &self.downloads { manager.set_concurrency(count as usize)?; }
                        self.preferences.concurrency = count;
                    }
                }
            }
            "preload" => {
                if let Ok(count) = value.parse::<u32>() {
                    if [0, 3, 5].contains(&count) {
                        self.preferences.preload = count;
                        self.reader_preload_changed();
                    }
                }
            }
            "wifi" if self.demo.is_some() && ["auto", "always"].contains(&value) => self.preferences.wifi = value.into(),
            "orientation" if self.demo.is_some() && ["auto", "portrait"].contains(&value) => self.preferences.orientation = value.into(),
            "sleep-cover" if self.demo.is_some() && ["cover", "system"].contains(&value) => self.preferences.sleep_cover = value == "cover",
            "wifi" | "orientation" | "sleep-cover" => self.toast("common_not_supported"),
            "cache-limit" => {
                if let Ok(limit) = value.parse::<u32>() {
                    if [256, 512, 1024, 2048].contains(&limit) { self.preferences.cache_limit = limit; self.enforce_cache_limit()?; }
                }
            }
            "launch" if ["resume", "shelf"].contains(&value) => self.preferences.launch = value.into(),
            "flashui" if ["on", "off"].contains(&value) => {
                self.preferences.flashui = value == "on"; self.platform.set_flash_on_navigation(self.preferences.flashui);
            }
            "clear-cache" => self.clear_account_cache()?,
            "export-diagnostics" => self.export_diagnostics()?,
            "check-update" => self.toast("acct_update_unconfigured"),
            "onboarding" => self.navigate("welcome"),
            "exit" => self.request_quit(),
            "recharge-close" => self.account_ui.recharge.visible = false,
            "recharge-option" => {
                if !self.account_ui.recharge.busy && self.account_ui.recharge.phase == "select" {
                    if let Ok(index) = value.parse::<usize>() {
                        if self.account_ui.recharge.config.as_ref().is_some_and(|config| index < config.options.len()) {
                            self.account_ui.recharge.selected = index;
                        }
                    }
                }
            }
            "recharge-create" => self.create_recharge_order()?,
            "recharge-check" => self.check_recharge(),
            "recharge-config" => self.load_recharge_config(),
            _ => {}
        }
        self.save_preferences()?;
        Ok(())
    }

    fn refresh_wallet(&mut self) {
        self.account_ui.wallet_attempted = true;
        if self.account_ui.busy { return; }
        if self.demo.is_some() {
            self.account_ui.preview_wallet_refreshed = true;
            self.account_ui.message.clear();
            return;
        }
        self.account_ui.busy = true;
        let client = self.client.clone();
        self.source_job("account-wallet", async move { client.wallet().await }, |app, result| {
            app.account_ui.busy = false;
            match result {
                Ok(wallet) => {
                    let snapshot = WalletSnapshot { value: wallet.clone(), observed_at: now() };
                    match app.store.set_preference(&app.account, "wallet", &snapshot) {
                        Ok(()) => { app.account_ui.wallet = Some(wallet); app.account_ui.wallet_at = snapshot.observed_at; }
                        Err(_) => app.toast("common_storage_error"),
                    }
                }
                Err(error) => app.source_failure(&error),
            }
        });
    }

    fn wallet_time(&self) -> String {
        if let Some(demo) = &self.demo {
            if self.account_ui.preview_wallet_refreshed { return i18n::text("acct_just_updated"); }
            return i18n::format("acct_balance_time", &[("time", demo.fixture["account"]["balance_time"].as_str().unwrap_or("—").into())]);
        }
        if self.account_ui.wallet_at <= 0 { return i18n::text("acct_balance_not_loaded"); }
        let timestamp = chrono::DateTime::from_timestamp(self.account_ui.wallet_at, 0)
            .map(|value| (value + chrono::Duration::hours(8)).format("%H:%M").to_string()).unwrap_or_default();
        i18n::format("acct_balance_time", &[("time", timestamp)])
    }

    fn import_account_session(&mut self, value: &str) {
        if self.demo.is_some() { self.toast("acct_preview_import"); return; }
        if !self.require_network("account", "import-session", value) { return; }
        let path = if value.is_empty() { self.data_dir.join("imports").join("bilibili.txt") } else { PathBuf::from(value) };
        if !path.is_file() {
            self.account_ui.message = i18n::format("acct_import_instructions", &[("path", path.display().to_string())]);
            return;
        }
        let data_dir = self.data_dir.clone();
        self.account_ui.busy = true;
        self.source_job("account-import", async move {
            let imported = tokio::task::spawn_blocking(move || {
                let metadata = std::fs::symlink_metadata(&path).map_err(|_| source::SourceError::storage("session_import_unreadable"))?;
                if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > MAX_IMPORT_BYTES {
                    return Err(source::SourceError::invalid("session_import_size_invalid"));
                }
                let mut bytes = Vec::new();
                std::fs::File::open(path).map_err(|_| source::SourceError::storage("session_import_unreadable"))?
                    .take(MAX_IMPORT_BYTES + 1).read_to_end(&mut bytes).map_err(|_| source::SourceError::storage("session_import_unreadable"))?;
                if bytes.len() as u64 > MAX_IMPORT_BYTES { return Err(source::SourceError::invalid("session_import_size_invalid")); }
                let text = std::str::from_utf8(&bytes).map_err(|_| source::SourceError::invalid("session_import_encoding_invalid"))?;
                AuthSession::from_cookie_text(text)
            }).await.map_err(|_| source::SourceError::storage("session_import_worker_failed"))??;
            let temporary = BiliClient::new(Some(imported), &data_dir)?;
            temporary.validate_session().await?;
            temporary.ensure_site_context().await?;
            temporary.validate_session().await?;
            temporary.session().ok_or_else(|| source::SourceError::authentication("session_import_unverified"))
        }, |app, result| {
            app.account_ui.busy = false;
            match result {
                Ok(session) => match app.switch_session(session) {
                    Ok(()) => app.toast("acct_import_complete"), Err(_) => app.toast("common_storage_error"),
                },
                Err(error) => app.source_failure(&error),
            }
        });
    }

    fn schedule_storage_summary(&mut self) {
        if self.account_ui.storage_pending { return; }
        self.account_ui.storage_pending = true;
        let database = self.store.path().to_owned(); let data = self.data_dir.clone(); let account = self.account.clone();
        self.background("account-storage", move || storage_summary(&database, &data, &account), |app, result| {
            app.account_ui.storage_pending = false; app.account_ui.storage_checked = Some(Instant::now());
            match result { Ok(summary) => app.account_ui.storage = Some(summary), Err(_) => app.toast("common_storage_error") }
        });
    }

    pub(super) fn enforce_cache_limit(&mut self) -> Result<()> {
        self.evict_account_cache(Some(u64::from(self.preferences.cache_limit) * 1024 * 1024))?;
        Ok(())
    }

    fn clear_account_cache(&mut self) -> Result<()> {
        if self.demo.is_some() { self.account_ui.preview_cache_cleared = true; return Ok(()); }
        let freed = self.evict_account_cache(None)?;
        self.account_ui.message = i18n::format("acct_cache_cleared", &[("size", byte_label(freed))]);
        self.schedule_storage_summary();
        Ok(())
    }

    fn evict_account_cache(&mut self, limit: Option<u64>) -> Result<u64> {
        let mut receipts_may_have_changed = false;
        let result = (|| -> Result<u64> {
            let Some(manager) = &mut self.downloads else { return Ok(0); };
            let mut records = self.store.downloads(&self.account)?;
            records.sort_by_key(|record| record.updated_at);
            let protected: BTreeSet<_> = records.iter().filter(|record| record.retained
                || record.revision == crate::store::PENDING_REVISION
                || matches!(record.status, crate::store::DownloadStatus::Running | crate::store::DownloadStatus::Queued))
                .map(|record| record.episode_id.clone()).collect();
            let mut candidates = Vec::new(); let mut seen = BTreeSet::new();
            for record in &records {
                if !seen.insert((record.episode_id.clone(), record.revision.clone())) { continue; }
                if protected.contains(&record.episode_id) { continue; }
                let pages = self.store.page_receipts(&self.account, &record.episode_id, &record.revision)?;
                if pages.iter().any(|page| page.retained) { continue; }
                candidates.push((record.episode_id.clone(), record.revision.clone()));
            }
            let before = directory_bytes(manager.pages_root())?;
            let mut cache_bytes = unique_cache_bytes(&self.store, &self.account)?;
            for (episode, revision) in candidates {
                if limit.is_some_and(|limit| cache_bytes <= limit) { break; }
                // The aggregate above protects all live/retained tasks; the manager also checks reader pins.
                match manager.remove_episode(&episode, &revision) {
                    Ok(()) => {
                        receipts_may_have_changed = true;
                        cache_bytes = unique_cache_bytes(&self.store, &self.account)?;
                    }
                    Err(error) if error.downcast_ref::<crate::downloads::DownloadControlError>() == Some(&crate::downloads::DownloadControlError::ReaderInUse) => continue,
                    Err(error) => {
                        receipts_may_have_changed = true;
                        return Err(error).context("Remove an automatic-cache episode");
                    }
                }
            }
            Ok(before.saturating_sub(directory_bytes(manager.pages_root())?))
        })();
        if receipts_may_have_changed { self.invalidate_catalog_local_state(); }
        result
    }

    fn export_diagnostics(&mut self) -> Result<()> {
        let directory = self.data_dir.join("diagnostics");
        std::fs::create_dir_all(&directory)?;
        let path = directory.join(format!("bilicomics-{}.json", now()));
        let caps = self.platform.capabilities();
        let value = json!({
            "application": "bilicomics", "version": env!("CARGO_PKG_VERSION"),
            "source_revision": source::REFERENCE_REVISION, "profile": self.options.profile.id(),
            "design_geometry": self.options.profile.logical_size(), "native": self.platform.is_kindle,
            "demo": self.demo.is_some(), "authenticated": self.client.session().is_some(),
            "capabilities": {"brightness": caps.brightness, "warmth": caps.warmth,
                "inversion": caps.hardware_inversion, "explicit_refresh": caps.explicit_refresh},
            "page_bytes": self.store.storage_usage(&self.account)?,
            "unresolved_purchase_count": self.account_ui.journals.iter().filter(|intent| intent.unresolved()).count(),
        });
        std::fs::write(&path, serde_json::to_vec_pretty(&value)?)?;
        self.account_ui.message = i18n::format("acct_diagnostics_exported", &[("path", path.display().to_string())]);
        Ok(())
    }

    fn save_purchase_journal(&mut self) -> Result<()> {
        self.account_ui.journals = merge_purchase_journal(self.store.path(), &self.account, &self.account_ui.journals)?;
        Ok(())
    }

    pub(super) fn open_purchase(&mut self, episode_id: &str) -> Result<()> {
        if !self.require_session() || !self.require_network("purchase", "open", episode_id) { return Ok(()); }
        self.invalidate("purchase-preflight");
        let unresolved = self.account_ui.journals.iter().rev().find(|intent| intent.unresolved()).cloned();
        let target = unresolved.as_ref().map(|intent| intent.quote.episode_id.as_str()).unwrap_or(episode_id);
        let Some((comic, episode)) = self.find_purchase_episode(target)? else {
            self.toast("acct_purchase_missing_episode"); return Ok(());
        };
        self.account_ui.comic = Some(comic);
        self.account_ui.episode = Some(episode);
        self.account_ui.quote = None; self.account_ui.quote_read = None;
        self.account_ui.quote_error.clear(); self.account_ui.reconciling = false;
        if let Some(intent) = unresolved {
            self.account_ui.current_journal = Some(intent.id.clone());
            self.account_ui.selection = intent.quote.selection.clone();
            self.account_ui.payment = intent.quote.payment.clone();
            self.account_ui.quote = Some(intent.quote);
            self.account_ui.phase = "pending".into();
        } else {
            self.account_ui.current_journal = None;
            self.account_ui.selection = "single".into(); self.account_ui.payment = "coin".into();
            self.account_ui.phase = "review".into();
        }
        self.navigate("purchase");
        if self.account_ui.phase == "review" { self.load_purchase_quote(); }
        Ok(())
    }

    pub(super) fn refresh_purchase_view(&mut self, ui: &AppWindow) -> Result<()> {
        let purchase = ui.global::<PurchaseModel>();
        purchase.set_phase(self.account_ui.phase.clone().into());
        purchase.set_selected_range(self.account_ui.selection.clone().into());
        purchase.set_payment(self.account_ui.payment.clone().into());
        purchase.set_quote_loading(self.account_ui.quote_loading);
        purchase.set_reconciling(self.account_ui.reconciling);
        if let Some(comic) = self.account_ui.comic.clone() {
            purchase.set_comic_title(comic.title.clone().into());
            purchase.set_cover(self.cover_image(&comic));
        }
        let episode_title = self.account_ui.episode.as_ref().map(|episode| {
            i18n::format("acct_episode_title", &[("number", display_order(episode.order)), ("title", episode.title.clone())])
        }).unwrap_or_default();
        purchase.set_episode_title(episode_title.into());
        let mut choices = Vec::new();
        if let Some(read) = &self.account_ui.quote_read {
            for (id, count, displayed, original, usable) in quote_selections(read) {
                let fixture_label = self.demo.as_ref().and_then(|demo| demo.fixture["purchase"]["ranges"].as_array())
                    .and_then(|ranges| ranges.iter().find(|range| range["key"].as_str() == Some(id.as_str())))
                    .and_then(|range| range["label"].as_str()).map(str::to_owned);
                let label = fixture_label.unwrap_or_else(|| if id == "single" {
                    i18n::format("acct_offer_single", &[("number", self.account_ui.episode.as_ref().map(|episode| display_order(episode.order)).unwrap_or_default())])
                } else { candidate_scope_label(read, count, true) });
                choices.push(PurchaseRangeItem { id: id.into(), label: label.into(),
                    price: format!("{} {}", display_amount(displayed), i18n::text("buy_coin")).into(),
                    original: if original != displayed { i18n::format("acct_original_price", &[("amount", display_amount(original))]).into() } else { "".into() },
                    enabled: usable });
            }
        }
        if choices.is_empty() {
            if let Some(quote) = &self.account_ui.quote {
                choices.push(PurchaseRangeItem { id: quote.selection.clone().into(),
                    label: quote_scope_label(quote, self.account_ui.quote_read.as_ref().map(|read| &read.detail), self.account_ui.episode.as_ref(), true).into(),
                    price: format!("{} {}", display_amount(quote.amount), i18n::text(if quote.payment == "coupon" { "acct_coupon_unit" } else { "buy_coin" })).into(),
                    original: "".into(), enabled: false });
            }
        }
        purchase.set_ranges(model(choices));
        let wallet = self.account_ui.wallet.as_ref();
        purchase.set_wallet_label(wallet.and_then(|value| amount(value.get("remain_gold")))
            .map(|value| i18n::format("acct_wallet_amount", &[("amount", display_amount(value))]))
            .unwrap_or_else(|| i18n::text("buy_wallet_unknown")).into());
        purchase.set_coupon_label(wallet.and_then(|value| amount(value.get("remain_coupon")))
            .map(|value| i18n::format("acct_coupon_amount", &[("amount", display_amount(value))]))
            .unwrap_or_else(|| i18n::text("buy_coupon_unknown")).into());
        if self.account_ui.selection != "single" { purchase.set_coupon_label(tr("acct_coupon_single_only")); }
        let coupons_allowed = self.account_ui.selection == "single" && self.account_ui.quote_read.as_ref()
            .is_some_and(|read| value_true(read.basic.get("allow_coupon")));
        purchase.set_coupons_allowed(coupons_allowed);
        let quote = self.account_ui.quote.as_ref();
        let fresh = quote.is_some_and(|quote| now() >= quote.observed_at && now() - quote.observed_at <= QUOTE_MAX_AGE);
        purchase.set_quote_ready(fresh && !self.account_ui.quote_loading
            && !self.account_ui.journals.iter().any(PurchaseIntent::unresolved)
            && ["review", "error"].contains(&self.account_ui.phase.as_str()));
        purchase.set_sufficient(quote.is_none_or(|quote| quote.balance >= quote.amount));
        purchase.set_total(quote.map(|quote| display_amount(quote.amount)).unwrap_or_else(|| i18n::text("buy_unknown")).into());
        let unit = i18n::text(if self.account_ui.payment == "coupon" { "acct_coupon_unit" } else { "buy_coin" });
        purchase.set_unit(unit.clone().into());
        purchase.set_after_text(quote.map(|quote| {
            if quote.balance < quote.amount {
                i18n::format("acct_balance_shortfall", &[("amount", display_amount(quote.amount - quote.balance)), ("unit", unit.clone())])
            } else { i18n::format("acct_after_payment", &[("amount", display_amount(quote.balance - quote.amount)), ("unit", unit.clone())]) }
        }).unwrap_or_default().into());
        let preview_coupon_label = self.demo.as_ref().filter(|_| self.account_ui.payment == "coupon")
            .and_then(|demo| demo.fixture["purchase"]["confirm_coupon_label"].as_str()).map(str::to_owned);
        purchase.set_pay_label(preview_coupon_label.or_else(|| quote.map(|quote| i18n::format(if quote.payment == "coupon" { "acct_use_coupon" } else { "acct_confirm_amount" }, &[("amount", display_amount(quote.amount)), ("unit", unit.clone())])))
            .unwrap_or_else(|| i18n::text("buy_confirm")).into());
        let preview_scope = self.demo.as_ref().and_then(|demo| demo.fixture["purchase"]["ranges"].as_array())
            .and_then(|ranges| ranges.iter().find(|range| range["key"].as_str() == Some(self.account_ui.selection.as_str())))
            .and_then(|range| range["scope_label"].as_str()).map(str::to_owned);
        purchase.set_scope_label(preview_scope.or_else(|| quote.map(|quote| quote_scope_label(quote, self.account_ui.quote_read.as_ref().map(|read| &read.detail), self.account_ui.episode.as_ref(), false))).unwrap_or_default().into());
        purchase.set_hint(if self.account_ui.quote_error.is_empty() { tr("buy_quote_hint") } else { self.account_ui.quote_error.clone().into() });
        purchase.set_error_text(self.account_ui.quote_error.clone().into());
        let current = self.current_purchase_intent().cloned();
        let mut rows = Vec::new();
        if let Some(intent) = &current {
            let pending = intent.unresolved();
            if pending {
                rows.push(PurchaseReceiptItem { label: tr("acct_receipt_chapter"), value: quote_scope_label(&intent.quote, self.account_ui.quote_read.as_ref().map(|read| &read.detail), self.account_ui.episode.as_ref(), false).into() });
            }
            if intent.state == "preview" {
                rows.push(PurchaseReceiptItem { label: tr("acct_record_state"), value: journal_label(&intent.state).into() });
            } else {
                rows.push(PurchaseReceiptItem { label: tr(if pending { "acct_submitted_quote" } else { "acct_confirmed_quote" }), value: format!("{} {}", display_amount(intent.quote.amount), unit).into() });
                if !pending {
                    let balance = self.account_ui.wallet.as_ref().and_then(|wallet| amount(wallet.get(if intent.quote.payment == "coupon" { "remain_coupon" } else { "remain_gold" })));
                    rows.push(PurchaseReceiptItem { label: tr("acct_current_balance"), value: balance.map(|value| format!("{} {}", display_amount(value), i18n::text(if intent.quote.payment == "coupon" { "acct_coupon_unit" } else { "buy_coin" })))
                        .unwrap_or_else(|| i18n::text("common_unknown")).into() });
                }
                rows.push(PurchaseReceiptItem { label: tr(if pending { "acct_submitted_at" } else { "acct_local_confirmed_at" }),
                    value: receipt_time(if pending { intent.created_at } else { intent.updated_at }).into() });
                if pending {
                    rows.push(PurchaseReceiptItem { label: tr("acct_record_state"), value: intent.id.clone().into() });
                }
                if let Some(acknowledgement) = &intent.acknowledgement {
                    for field in ["order_id", "transaction_id", "orderId", "buy_id"] {
                        if let Some(value) = acknowledgement.get(field).and_then(|value| value.as_str().map(str::to_owned).or_else(|| value.as_u64().map(|value| value.to_string()))) {
                            rows.push(PurchaseReceiptItem { label: tr("acct_server_reference"), value: value.into() }); break;
                        }
                    }
                }
            }
            if intent.entitlement_observed && intent.state == "unknown" {
                rows.push(PurchaseReceiptItem { label: tr("acct_entitlement"), value: tr("acct_access_observed_unknown_charge") });
            }
        }
        purchase.set_result_rows(model(rows));
        let preview = current.as_ref().is_some_and(|intent| intent.state == "preview");
        purchase.set_result_title(tr(if preview { "acct_preview_complete" } else { "buy_success_title" }));
        purchase.set_result_body(tr(if preview { "acct_preview_no_transaction" } else { "buy_success_body" }));
        purchase.set_result_can_read(current.as_ref().is_some_and(|intent| intent.state == "confirmed"));
        purchase.set_pending_body(if current.as_ref().is_some_and(|intent| intent.entitlement_observed) {
            tr("acct_access_observed_unknown_charge")
        } else if current.as_ref().is_some_and(|intent| intent.state == "acknowledged") {
            tr("acct_accepted_waiting_entitlement")
        } else { tr("buy_pending_body") });
        Ok(())
    }

    pub(super) fn purchase_action(&mut self, action: &str, value: &str) -> Result<()> {
        if action == "open" { return self.open_purchase(value); }
        if self.account_ui.phase == "submitting" && action == "reconcile" { return Ok(()); }
        if ["submitting", "pending"].contains(&self.account_ui.phase.as_str())
            && !["close", "back", "reconcile"].contains(&action) { return Ok(()); }
        match action {
            "back" | "close" => {
                if self.account_ui.phase == "submitting" && self.account_ui.current_journal.is_none() {
                    self.invalidate("purchase-preflight"); self.account_ui.phase = "review".into();
                }
                self.go_back()?;
            }
            "range" => {
                let valid = self.account_ui.quote_read.as_ref().is_some_and(|read| quote_selections(read).iter().any(|(id, _, _, _, usable)| id == value && *usable));
                if valid {
                    self.account_ui.selection = value.into();
                    if value != "single" { self.account_ui.payment = "coin".into(); }
                    self.load_purchase_quote();
                }
            }
            "payment" if ["coin", "coupon"].contains(&value) => {
                if value == "coupon" && (self.account_ui.selection != "single" || !self.account_ui.quote_read.as_ref().is_some_and(|read| value_true(read.basic.get("allow_coupon")))) {
                    self.toast("acct_coupon_unverified");
                } else { self.account_ui.payment = value.into(); self.load_purchase_quote(); }
            }
            "refresh-wallet" => self.load_purchase_quote(),
            "refresh-quote" => self.load_purchase_quote(),
            "submit" => self.submit_purchase()?,
            "reconcile" => self.reconcile_purchase(),
            "recharge" => { self.navigate("account"); self.open_recharge(); }
            "detail" => { if let Some(comic) = self.account_ui.comic.clone() { self.open_detail(&comic.id)?; } }
            "download" | "read" => {
                if let Some(intent) = self.current_purchase_intent().cloned().filter(|intent| intent.state == "confirmed") {
                    if action == "download" { for id in intent.quote.expected_ids { self.enqueue_episode(&id, true)?; } }
                    else { self.open_episode(&intent.quote.comic_id, &intent.quote.episode_id)?; }
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn find_purchase_episode(&self, episode_id: &str) -> Result<Option<(Comic, Episode)>> {
        for comic in self.store.load_comics(&self.account)? {
            if let Some(detail) = self.store.load_detail(&self.account, &comic.id)? {
                if let Some(episode) = detail.episodes.iter().find(|episode| episode.id == episode_id) {
                    return Ok(Some((detail.comic, episode.clone())));
                }
            }
        }
        Ok(None)
    }

    fn load_purchase_quote(&mut self) {
        if ["submitting", "pending"].contains(&self.account_ui.phase.as_str()) { return; }
        let Some(comic) = self.account_ui.comic.clone() else { return; };
        let Some(episode) = self.account_ui.episode.clone() else { return; };
        if !self.require_session() || !self.require_network("purchase", "refresh-quote", "") { return; }
        self.account_ui.phase = "review".into(); self.account_ui.quote_loading = true;
        self.account_ui.quote = None; self.account_ui.quote_error.clear();
        if self.demo.is_some() {
            let detail = match self.store.load_detail(&self.account, &comic.id) {
                Ok(Some(detail)) => detail, _ => { self.account_ui.quote_loading = false; return; }
            };
            match preview_quote_read(self.demo.as_ref().expect("The preview branch has fixture data"), &episode, detail) {
                Ok(read) => self.apply_quote_read(read),
                Err(_) => { self.account_ui.quote_loading = false; self.account_ui.quote_error = i18n::text("acct_quote_unverified"); }
            }
            return;
        }
        let selection = self.account_ui.selection.clone(); let client = self.client.clone();
        self.source_job("purchase-quote", fetch_quote_read(client, comic.id, episode.id, selection), |app, result| match result {
            Ok(read) => app.apply_quote_read(read),
            Err(error) => {
                app.account_ui.quote_loading = false; app.account_ui.quote_error = i18n::text("acct_quote_unverified");
                app.source_failure(&error);
            }
        });
    }

    fn apply_quote_read(&mut self, read: QuoteRead) {
        self.account_ui.quote_loading = false;
        self.account_ui.wallet = Some(read.wallet.clone());
        self.account_ui.wallet_at = read.observed_at;
        let _ = self.store.save_detail(&self.account, &read.detail);
        let episode_id = self.account_ui.episode.as_ref().map(|episode| episode.id.clone()).unwrap_or_default();
        let normalized = if self.demo.is_some() {
            preview_offer(&self.account, &episode_id, &self.account_ui.selection, &self.account_ui.payment, &read)
        } else { normalize_offer(&self.account, &episode_id, &self.account_ui.selection, &self.account_ui.payment, &read) };
        match normalized {
            Ok(quote) => { self.account_ui.quote = Some(quote); self.account_ui.quote_error.clear(); }
            Err(_) => {
                self.account_ui.quote = None; self.account_ui.quote_error = i18n::text("acct_quote_unverified");
            }
        }
        self.account_ui.quote_read = Some(read);
        self.dirty = true;
    }

    fn submit_purchase(&mut self) -> Result<()> {
        if !["review", "error"].contains(&self.account_ui.phase.as_str()) || self.account_ui.quote_loading { return Ok(()); }
        if self.account_ui.journals.iter().any(PurchaseIntent::unresolved) {
            self.toast("acct_resolve_previous_purchase"); return Ok(());
        }
        let Some(quote) = self.account_ui.quote.clone() else { self.toast("acct_quote_unverified"); return Ok(()); };
        if quote.account != self.account || now() < quote.observed_at || now() - quote.observed_at > QUOTE_MAX_AGE {
            self.account_ui.quote_error = i18n::text("acct_quote_expired"); self.load_purchase_quote(); return Ok(());
        }
        if quote.balance < quote.amount { self.toast("acct_insufficient_balance"); return Ok(()); }
        if !self.require_session() || !self.require_network("purchase", "submit", "") { return Ok(()); }
        if self.demo.is_some() {
            let intent = PurchaseIntent { id: transaction_id("preview", &self.account), account: self.account.clone(), quote,
                state: "preview".into(), created_at: now(), updated_at: now(), attempts: 0,
                acknowledgement: None, error_code: None, entitlement_observed: false, persistence_pending: false };
            self.account_ui.current_journal = Some(intent.id.clone());
            self.account_ui.journals.push(intent); self.save_purchase_journal()?;
            self.account_ui.phase = "done".into(); return Ok(());
        }
        self.account_ui.phase = "submitting".into();
        self.invalidate("purchase-quote");
        let client = self.client.clone();
        let expected = quote.clone();
        self.source_job("purchase-preflight", fetch_quote_read(client, quote.comic_id.clone(), quote.episode_id.clone(), quote.selection.clone()), move |app, result| {
            if app.route != "purchase" || app.account_ui.phase != "submitting"
                || app.account_ui.selection != expected.selection || app.account_ui.payment != expected.payment
                || app.account_ui.episode.as_ref().is_none_or(|episode| episode.id != expected.episode_id)
            { return; }
            match result {
                Ok(read) => {
                    let refreshed = normalize_offer(&app.account, &expected.episode_id, &expected.selection, &expected.payment, &read);
                    match refreshed {
                        Ok(current) if current.fingerprint == expected.fingerprint => {
                            app.account_ui.quote_read = Some(read); app.account_ui.quote = Some(current.clone());
                            if app.dispatch_purchase(current).is_err() {
                                app.account_ui.phase = "error".into(); app.account_ui.quote_error = i18n::text("common_storage_error");
                                app.toast("common_storage_error");
                            }
                        }
                        _ => {
                            app.account_ui.phase = "review".into(); app.apply_quote_read(read);
                            app.account_ui.quote_error = i18n::text("acct_quote_changed");
                        }
                    }
                }
                Err(error) => {
                    app.account_ui.phase = "error".into(); app.account_ui.quote = None;
                    app.account_ui.quote_error = i18n::text("acct_quote_unverified"); app.source_failure(&error);
                }
            }
        });
        Ok(())
    }

    fn dispatch_purchase(&mut self, quote: OfferSnapshot) -> Result<()> {
        anyhow::ensure!(self.demo.is_none(), "Preview requests cannot enter the live purchase path");
        if self.account_receipts_pending() {
            self.account_ui.phase = "review".into();
            self.toast("acct_receipt_storage_pending");
            return Ok(());
        }
        if self.account_ui.journals.iter().any(PurchaseIntent::unresolved) { anyhow::bail!("A transaction is already unresolved"); }
        anyhow::ensure!(quote.account == self.account && now() >= quote.observed_at
            && now() - quote.observed_at <= QUOTE_MAX_AGE, "The confirmed quote is stale");
        let intent = PurchaseIntent { id: transaction_id("purchase", &self.account), account: self.account.clone(), quote,
            state: "submitting".into(), created_at: now(), updated_at: now(), attempts: 1,
            acknowledgement: None, error_code: None, entitlement_observed: false, persistence_pending: false };
        let job = format!("purchase-submit:{}", intent.id);
        let admission = match self.begin_job(&job, true) {
            Ok(admission) => admission,
            Err(_) => {
                self.account_ui.phase = "review".into();
                self.toast("common_background_busy");
                return Ok(());
            }
        };
        self.account_ui.current_journal = Some(intent.id.clone());
        self.account_ui.journals.push(intent.clone()); trim_journal(&mut self.account_ui.journals);
        self.save_purchase_journal()?;
        self.account_ui.phase = "submitting".into();
        let client = self.client.clone(); let database = self.store.path().to_owned();
        self.source_job_reserved(&job, admission, async move {
            let result = client.buy_episode_for_account(&intent.quote.request, &intent.account).await;
            let error = result.as_ref().err().cloned();
            let result = result.map(|value| public_purchase_acknowledgement(&value));
            let pending = PendingPayment::Purchase { database: database.clone(), intent: intent.clone(), result: result.clone() };
            let key = remember_payment(pending);
            let mut observed = observed_purchase_outcome(&intent, &result);
            observed.persistence_pending = true;
            match tokio::task::spawn_blocking(move || persist_purchase_outcome(&database, &intent, result)).await {
                Ok(Ok(saved)) => { forget_payment(&key); Ok((saved, error, true)) }
                _ => Ok((observed, error, false)),
            }
        }, |app, result| match result {
            Ok((intent, error, persisted)) => {
                app.replace_local_intent(intent.clone());
                app.account_ui.phase = if !persisted { "pending" } else if intent.state == "confirmed" { "done" } else if intent.state == "rejected" { "error" } else { "pending" }.into();
                if !persisted { app.account_ui.quote_error = i18n::text("acct_receipt_storage_pending"); }
                if let Some(error) = error {
                    app.account_ui.quote_error = i18n::text(if intent.state == "rejected" { "acct_purchase_rejected" } else { "buy_pending_body" });
                    if intent.state == "rejected" { app.account_ui.quote = None; }
                    app.source_failure(&error);
                }
                if persisted && intent.state == "acknowledged" { app.reconcile_purchase(); }
            }
            Err(error) => {
                app.account_ui.phase = "pending".into();
                app.account_ui.quote_error = i18n::text("acct_receipt_storage_pending"); app.source_failure(&error);
            }
        });
        Ok(())
    }

    fn current_purchase_intent(&self) -> Option<&PurchaseIntent> {
        let id = self.account_ui.current_journal.as_deref()?;
        self.account_ui.journals.iter().find(|intent| intent.id == id)
    }

    fn replace_local_intent(&mut self, intent: PurchaseIntent) {
        if let Some(current) = self.account_ui.journals.iter_mut().find(|current| current.id == intent.id) { *current = intent; }
    }

    fn reconcile_purchase(&mut self) {
        if self.account_ui.reconciling || self.demo.is_some() { return; }
        if self.save_purchase_journal().is_err() { self.toast("common_storage_error"); return; }
        if self.current_purchase_intent().is_some_and(|intent| intent.state == "confirmed" && !intent.persistence_pending) {
            self.account_ui.phase = "done".into(); return;
        }
        let Some(intent) = self.current_purchase_intent().cloned().filter(PurchaseIntent::unresolved) else { return; };
        if !self.require_session() || !self.require_network("purchase", "reconcile", "") { return; }
        self.account_ui.reconciling = true;
        let client = self.client.clone(); let comic_id = intent.quote.comic_id.clone();
        self.source_job("purchase-reconcile", async move {
            let detail = client.comic_detail(&comic_id).await?; let wallet = client.wallet().await?;
            Ok((detail, wallet))
        }, move |app, result| {
            app.account_ui.reconciling = false;
            match result {
                Ok((detail, wallet)) => {
                    let mut current = app.account_ui.journals.iter().find(|current| current.id == intent.id).cloned().unwrap_or(intent);
                    let previous = current.clone();
                    current.entitlement_observed = !current.quote.expected_ids.is_empty() && current.quote.expected_ids.iter().all(|id| {
                        detail.episodes.iter().find(|episode| &episode.id == id)
                            .is_some_and(|episode| episode.access == EpisodeAccess::Owned && episode.expires_at.is_none_or(|value| value <= 0))
                    });
                    current.updated_at = now();
                    if current.state == "acknowledged" && current.entitlement_observed { current.state = "confirmed".into(); }
                    let saved = app.store.save_detail(&app.account, &detail)
                        .and_then(|_| app.store.set_preference(&app.account, "wallet", &WalletSnapshot { value: wallet.clone(), observed_at: now() }));
                    if saved.is_err() { app.toast("common_storage_error"); return; }
                    app.replace_local_intent(current.clone());
                    if app.save_purchase_journal().is_err() {
                        app.replace_local_intent(previous); app.account_ui.phase = "pending".into();
                        app.toast("common_storage_error"); return;
                    }
                    app.account_ui.wallet = Some(wallet); app.account_ui.wallet_at = now();
                    app.account_ui.phase = if app.current_purchase_intent().is_some_and(|intent| intent.state == "confirmed" && !intent.persistence_pending) { "done" } else { "pending" }.into();
                    app.account_ui.quote_error.clear();
                }
                Err(error) => app.source_failure(&error),
            }
        });
    }

    fn open_recharge(&mut self) {
        if !self.require_session() || !self.require_network("account", "recharge", "") { return; }
        self.account_ui.recharge.visible = true;
        self.account_ui.recharge.title = i18n::text("acct_recharge");
        self.account_ui.recharge.rows.clear();
        if let Some(intent) = &self.account_ui.recharge.journal {
            if intent.persistence_pending || ["creating", "unknown", "qr"].contains(&intent.state.as_str()) {
                self.account_ui.recharge.phase = if intent.order.is_some() { "qr" } else { "pending" }.into();
                self.account_ui.recharge.message = i18n::text(if intent.order.is_some() { "acct_recharge_scan" } else { "acct_recharge_unknown" });
                if let Some(order) = &intent.order { self.account_ui.recharge.qr = super::auth::qr_image(&order.code_url).unwrap_or_default(); }
                return;
            }
        }
        self.load_recharge_config();
    }

    fn load_recharge_config(&mut self) {
        if self.account_ui.recharge.busy { return; }
        if self.account_ui.recharge.journal.as_ref().is_some_and(|intent| intent.persistence_pending || ["creating", "unknown", "qr"].contains(&intent.state.as_str())) { return; }
        if self.demo.is_some() {
            self.account_ui.recharge.phase = "error".into();
            self.account_ui.recharge.message = i18n::text("acct_preview_recharge");
            self.account_ui.recharge.config = None;
            return;
        }
        if !self.require_session() || !self.require_network("account", "recharge-config", "") { return; }
        self.account_ui.recharge.busy = true; self.account_ui.recharge.phase = "loading".into();
        self.account_ui.recharge.config = None; self.account_ui.recharge.message.clear();
        let client = self.client.clone();
        self.source_job("recharge-config", async move { client.recharge_config().await }, |app, result| {
            app.account_ui.recharge.busy = false;
            match result {
                Ok(config) => {
                    app.account_ui.recharge.phase = if config.options.is_empty() { "error" } else { "select" }.into();
                    app.account_ui.recharge.selected = 0;
                    app.account_ui.recharge.config = Some(config);
                    app.account_ui.recharge.message = i18n::text("acct_recharge_config_note");
                }
                Err(error) => {
                    app.account_ui.recharge.phase = "error".into();
                    app.account_ui.recharge.message = i18n::text("acct_recharge_config_error"); app.source_failure(&error);
                }
            }
        });
    }

    fn create_recharge_order(&mut self) -> Result<()> {
        if self.demo.is_some() { self.toast("acct_preview_no_transaction"); return Ok(()); }
        if self.account_ui.recharge.busy || self.account_ui.recharge.phase != "select" { return Ok(()); }
        if self.account_receipts_pending() { self.toast("acct_receipt_storage_pending"); return Ok(()); }
        if !self.require_session() || !self.require_network("account", "recharge-create", "") { return Ok(()); }
        let Some(config) = &self.account_ui.recharge.config else { return Ok(()); };
        if now() < config.observed_at || now() - config.observed_at > QUOTE_MAX_AGE {
            self.load_recharge_config(); self.toast("acct_quote_expired"); return Ok(());
        }
        let Some(option) = config.options.get(self.account_ui.recharge.selected).cloned() else { return Ok(()); };
        let intent = RechargeIntent {
            id: transaction_id("recharge", &self.account), account: self.account.clone(),
            amount_cents: option.amount_cents, fingerprint: option.fingerprint, state: "creating".into(),
            created_at: now(), updated_at: now(), order: None, reconciliation_id: None, error_code: None, persistence_pending: false,
        };
        let job = format!("recharge-create:{}", intent.id);
        let admission = match self.begin_job(&job, true) {
            Ok(admission) => admission,
            Err(_) => { self.toast("common_background_busy"); return Ok(()); }
        };
        merge_recharge_intent(self.store.path(), &intent)?;
        self.account_ui.recharge.journal = Some(intent.clone());
        self.account_ui.recharge.phase = "creating".into(); self.account_ui.recharge.busy = true;
        let client = self.client.clone(); let database = self.store.path().to_owned();
        self.source_job_reserved(&job, admission, async move {
            let result = client.create_recharge_order_for_account(intent.amount_cents, Some(&intent.fingerprint), &intent.account).await;
            let error = result.as_ref().err().cloned();
            let key = remember_payment(PendingPayment::Recharge { database: database.clone(), intent: intent.clone(), result: result.clone() });
            let mut observed = observed_recharge_outcome(&intent, &result); observed.persistence_pending = true;
            match tokio::task::spawn_blocking(move || persist_recharge_outcome(&database, &intent, result)).await {
                Ok(Ok(saved)) => { forget_payment(&key); Ok((saved, error, true)) }
                _ => Ok((observed, error, false)),
            }
        }, |app, result| {
            app.account_ui.recharge.busy = false;
            match result {
                Ok((intent, error, persisted)) => {
                    app.account_ui.recharge.phase = if persisted { match intent.state.as_str() { "qr" => "qr", "rejected" => "error", _ => "pending" } } else { "pending" }.into();
                    app.account_ui.recharge.message = i18n::text(match intent.state.as_str() {
                        "qr" => "acct_recharge_scan", "rejected" => "acct_recharge_rejected", _ => "acct_recharge_unknown",
                    });
                    if let Some(order) = &intent.order { app.account_ui.recharge.qr = super::auth::qr_image(&order.code_url).unwrap_or_default(); }
                    app.account_ui.recharge.journal = Some(intent);
                    if let Some(error) = error { app.source_failure(&error); }
                }
                Err(error) => {
                    app.account_ui.recharge.phase = "pending".into();
                    app.account_ui.recharge.message = i18n::text("acct_recharge_unknown"); app.source_failure(&error);
                }
            }
        });
        Ok(())
    }

    fn check_recharge(&mut self) {
        if self.account_ui.recharge.busy || self.demo.is_some() { return; }
        if !self.require_session() || !self.require_network("account", "recharge-check", "") { return; }
        self.account_ui.recharge.busy = true;
        let client = self.client.clone();
        self.source_job("recharge-check", async move {
            let history = client.recharge_history(1, 100).await?; let wallet = client.wallet().await?;
            Ok((history, wallet))
        }, |app, result| {
            app.account_ui.recharge.busy = false;
            match result {
                Ok((history, wallet)) => {
                    if let Some(mut intent) = app.account_ui.recharge.journal.clone() {
                        let identity = intent.order.as_ref().map(|order| order.order_id.clone()).or_else(|| intent.reconciliation_id.clone());
                        let observed = identity.as_ref().is_some_and(|id| history["records"].as_array()
                            .is_some_and(|records| records.iter().any(|record| record["order_id"].as_str() == Some(id.as_str()))));
                        if observed {
                            // An exact history observation is retained; a balance delta is never payment proof.
                            intent.state = "observed".into(); intent.updated_at = now();
                            match merge_recharge_intent(app.store.path(), &intent) {
                                Ok(saved) => app.account_ui.recharge.journal = Some(saved),
                                Err(_) => { app.toast("common_storage_error"); return; }
                            }
                        }
                        app.account_ui.recharge.message = i18n::text(if observed { "acct_recharge_order_observed" } else { "acct_recharge_not_observed" });
                    }
                    let snapshot = WalletSnapshot { value: wallet.clone(), observed_at: now() };
                    if app.store.set_preference(&app.account, "wallet", &snapshot).is_err() { app.toast("common_storage_error"); return; }
                    app.account_ui.wallet = Some(wallet); app.account_ui.wallet_at = snapshot.observed_at;
                }
                Err(error) => app.source_failure(&error),
            }
        });
    }

    fn refresh_recharge_view(&self, ui: &AppWindow) -> Result<()> {
        let model_ui = ui.global::<AccountModel>();
        let recharge = &self.account_ui.recharge;
        model_ui.set_recharge_visible(recharge.visible);
        model_ui.set_recharge_phase(recharge.phase.clone().into());
        model_ui.set_recharge_title(recharge.title.clone().into());
        model_ui.set_recharge_index(recharge.selected as i32);
        model_ui.set_recharge_busy(recharge.busy);
        model_ui.set_recharge_message(recharge.message.clone().into());
        model_ui.set_recharge_qr(recharge.qr.clone());
        model_ui.set_recharge_rows(model(recharge.rows.iter().cloned().map(SharedString::from).collect()));
        model_ui.set_recharge_options(model(recharge.config.as_ref().map(|config| config.options.iter().map(|option| {
            i18n::format("acct_recharge_option", &[("amount", yuan_label(option.amount_cents)), ("coins", option.coin_amount.to_string())]).into()
        }).collect()).unwrap_or_default()));
        let selected = recharge.config.as_ref().and_then(|config| config.options.get(recharge.selected));
        model_ui.set_recharge_total(selected.map(|option| yuan_label(option.amount_cents))
            .or_else(|| recharge.journal.as_ref().map(|intent| yuan_label(intent.amount_cents)))
            .unwrap_or_else(|| i18n::text("common_unknown")).into());
        model_ui.set_recharge_can_check(recharge.journal.is_some() && !recharge.busy);
        Ok(())
    }

    fn open_purchase_history(&mut self) {
        self.account_ui.recharge.visible = true;
        self.account_ui.recharge.title = i18n::text("acct_purchase_history");
        self.account_ui.recharge.phase = "history".into();
        self.account_ui.recharge.message = i18n::text("acct_local_purchase_history");
        self.account_ui.recharge.rows = self.account_ui.journals.iter().rev().take(16).map(|intent| {
            format!("{} · {}", journal_label(&intent.state), i18n::format("acct_offer_count", &[("count", intent.quote.expected_ids.len().to_string())]))
        }).collect();
        if self.account_ui.recharge.rows.is_empty() { self.account_ui.recharge.rows.push(i18n::text("acct_no_records")); }
    }

    fn open_recharge_history(&mut self) {
        if !self.require_session() || !self.require_network("account", "recharge-history", "") { return; }
        self.account_ui.recharge.visible = true;
        self.account_ui.recharge.title = i18n::text("acct_recharge_history");
        self.account_ui.recharge.phase = "history".into();
        self.account_ui.recharge.message = i18n::text("acct_loading_history");
        self.account_ui.recharge.rows.clear();
        if self.demo.is_some() {
            self.account_ui.recharge.message = i18n::text("acct_preview_no_transaction"); return;
        }
        let client = self.client.clone();
        self.source_job("recharge-history", async move { client.recharge_history(1, 16).await }, |app, result| match result {
            Ok(history) => {
                app.account_ui.recharge.rows = history["records"].as_array().into_iter().flatten().map(|record| {
                    let created = record["created_at"].as_str().unwrap_or("—");
                    let channel = record["pay_channel_name"].as_str().or(record["pay_channel"].as_str()).unwrap_or("—");
                    let coins = record.get("product_amount").and_then(Value::as_u64).map(|value| value.to_string()).unwrap_or_else(|| i18n::text("common_unknown"));
                    format!("{created} · {channel} · {coins} {}", i18n::text("buy_coin"))
                }).collect();
                app.account_ui.recharge.message = if app.account_ui.recharge.rows.is_empty() { i18n::text("acct_no_records") } else { String::new() };
            }
            Err(error) => { app.account_ui.recharge.message = i18n::text("common_source_error"); app.source_failure(&error); }
        });
    }
}

fn preview_quote_read(demo: &DemoData, episode: &Episode, detail: ComicDetail) -> source::Result<QuoteRead> {
    preview_quote_read_fixture(&demo.fixture, episode, detail)
}

fn preview_quote_read_fixture(catalog: &Value, episode: &Episode, detail: ComicDetail) -> source::Result<QuoteRead> {
    let fixture = &catalog["purchase"];
    if fixture["demo_only"] != true { return Err(SourceError::capability("preview_quote_fixture_missing")); }
    let matching = fixture["selected_episode_id"].as_u64().map(|value| value.to_string()).as_deref() == Some(episode.id.as_str());
    let ranges = if matching { fixture["ranges"].clone() } else {
        json!([{"key":"single","start_ordinal":episode.order,"end_ordinal":episode.order,"count":1,
            "coin_price":episode.pay_gold,"original_coin_price":null,"coupon_eligible":false,"coupon_count":null}])
    };
    let ranges_array = ranges.as_array().ok_or_else(|| SourceError::capability("preview_ranges_missing"))?;
    let single = ranges_array.iter().find(|range| range["key"] == "single")
        .ok_or_else(|| SourceError::capability("preview_single_range_missing"))?;
    let wallet = json!({"remain_gold":fixture["wallet"]["coins"],"remain_coupon":fixture["wallet"]["coupons"]});
    let batches: Vec<_> = ranges_array.iter().filter(|range| range["key"] != "single").map(|range| json!({
        "batch_limit":range["count"], "amount":range["count"], "usable":true,
        "original_gold":range.get("original_coin_price").filter(|value| !value.is_null()).unwrap_or(&range["coin_price"]),
        "pay_gold":range["coin_price"],
    })).collect();
    let basic = json!({"ep_id":episode.id,"comic_id":detail.comic.id,"is_locked":true,
        "ep_original_gold":single.get("original_coin_price").filter(|value| !value.is_null()).unwrap_or(&single["coin_price"]),
        "pay_gold":single["coin_price"],"remain_gold":wallet["remain_gold"],"remain_coupon":wallet["remain_coupon"],
        "allow_coupon":single["coupon_eligible"],"ep_pay_coupons":single["coupon_count"],
        "batch_buy":batches,"preview_only":true,"preview_ranges":ranges});
    Ok(QuoteRead { basic: basic.clone(), scoped: basic, wallet, detail, observed_at: now() })
}

fn preview_offer(account: &str, episode_id: &str, selection: &str, payment: &str, read: &QuoteRead) -> source::Result<OfferSnapshot> {
    if read.basic["preview_only"] != true || read.basic["ep_id"].as_str() != Some(episode_id) {
        return Err(SourceError::capability("preview_quote_binding_invalid"));
    }
    let range = read.basic["preview_ranges"].as_array().and_then(|ranges| ranges.iter().find(|range| range["key"].as_str() == Some(selection)))
        .ok_or_else(|| SourceError::capability("preview_quote_range_missing"))?;
    let coin = amount(range.get("coin_price")).ok_or_else(|| SourceError::capability("preview_quote_amount_missing"))?;
    let original = amount(range.get("original_coin_price")).unwrap_or(coin);
    let (price, balance) = if payment == "coupon" {
        if selection != "single" || !value_true(range.get("coupon_eligible")) { return Err(SourceError::capability("preview_coupon_unavailable")); }
        (amount(range.get("coupon_count")).ok_or_else(|| SourceError::capability("preview_coupon_amount_missing"))?,
            amount(read.wallet.get("remain_coupon")).ok_or_else(|| SourceError::capability("preview_wallet_missing"))?)
    } else if payment == "coin" {
        (coin, amount(read.wallet.get("remain_gold")).ok_or_else(|| SourceError::capability("preview_wallet_missing"))?)
    } else { return Err(SourceError::invalid("preview_payment_invalid")); };
    let start = amount(range.get("start_ordinal")).ok_or_else(|| SourceError::capability("preview_range_missing"))?;
    let end = amount(range.get("end_ordinal")).ok_or_else(|| SourceError::capability("preview_range_missing"))?;
    let members: Vec<_> = read.detail.episodes.iter().filter(|episode| episode.order >= start && episode.order <= end).collect();
    if members.len() != range["count"].as_u64().unwrap_or(0) as usize || members.is_empty() {
        return Err(SourceError::capability("preview_members_missing"));
    }
    let expected_ids = members.iter().map(|episode| episode.id.clone()).collect();
    let before_access = members.iter().map(|episode| (episode.id.clone(), episode.access)).collect();
    // This preview request is never passed to BiliClient; no synthetic coupon identity is created.
    let request = PurchaseRequest {
        scope: if selection == "single" { PurchaseScope::Single { episode_id: episode_id.into(), comic_id: Some(read.detail.comic.id.clone()) } }
            else { PurchaseScope::OrdinalRange { comic_id: read.detail.comic.id.clone(), start_order: start, limit: members.len() as u32 } },
        method: if payment == "coupon" { PurchaseMethod::Coupon } else { PurchaseMethod::Currency },
        pay_amount: if payment == "coin" { Some(price) } else { None },
        coupon_id: None, coupon_ids: Vec::new(), free_gold_card_id: None, free_gold_amount: None,
    };
    let fingerprint = format!("preview:{}", hex_digest(serde_json::to_string(&json!({"account":account,"episode_id":episode_id,
        "selection":selection,"payment":payment,"range":range,"wallet":read.wallet})).unwrap_or_default().as_bytes()));
    Ok(OfferSnapshot { selection: selection.into(), payment: payment.into(), account: account.into(),
        comic_id: read.detail.comic.id.clone(), episode_id: episode_id.into(), amount: price, original_amount: original,
        balance, request, expected_ids, before_access, raw_offer_index: selection.strip_prefix("batch:").and_then(|value| value.parse().ok()),
        observed_at: read.observed_at, fingerprint })
}

async fn fetch_quote_read(client: BiliClient, comic_id: String, episode_id: String, selection: String) -> source::Result<QuoteRead> {
    let observed_at = now();
    let basic = client.purchase_info(&episode_id, QuoteScope::Basic).await?;
    let scope = if selection == "single" { QuoteScope::Single { order: 1 } } else {
        let index = selection.strip_prefix("batch:").and_then(|value| value.parse::<usize>().ok())
            .and_then(|value| value.checked_sub(1)).ok_or_else(|| source::SourceError::invalid("quote_selection_invalid"))?;
        let raw = basic.get("batch_buy").and_then(Value::as_array).and_then(|offers| offers.get(index))
            .ok_or_else(|| source::SourceError::invalid("quote_offer_missing"))?;
        let limit = raw.get("batch_limit").and_then(Value::as_u64).filter(|value| *value <= i32::MAX as u64)
            .ok_or_else(|| source::SourceError::invalid("quote_limit_invalid"))? as u32;
        QuoteScope::Batch { limit, order: 1 }
    };
    let scoped = client.purchase_info(&episode_id, scope).await?;
    let detail = client.comic_detail(&comic_id).await?;
    let wallet = client.wallet().await?;
    if now() < observed_at || now() - observed_at > QUOTE_MAX_AGE {
        return Err(source::SourceError::capability("quote_read_window_expired"));
    }
    Ok(QuoteRead { basic, scoped, wallet, detail, observed_at })
}

fn persist_purchase_outcome(database: &Path, intent: &PurchaseIntent, result: source::Result<Value>) -> Result<PurchaseIntent> {
    let observed = observed_purchase_outcome(intent, &result);
    let merged = merge_purchase_journal(database, &intent.account, &[observed])?;
    merged.into_iter().find(|current| current.id == intent.id).context("The durable purchase intent is missing")
}

fn persist_recharge_outcome(database: &Path, intent: &RechargeIntent, result: source::Result<RechargeOrder>) -> Result<RechargeIntent> {
    merge_recharge_intent(database, &observed_recharge_outcome(intent, &result))
}

fn observed_purchase_outcome(intent: &PurchaseIntent, result: &source::Result<Value>) -> PurchaseIntent {
    let mut current = intent.clone(); current.updated_at = now(); current.persistence_pending = false;
    match result {
        Ok(receipt) => { current.state = "acknowledged".into(); current.acknowledgement = Some(public_purchase_acknowledgement(receipt)); current.error_code = None; }
        Err(error) => {
            current.state = if error.definitive || error.transmitted == Some(false) { "rejected" } else { "unknown" }.into();
            current.error_code = Some(error.code.clone());
        }
    }
    current
}

fn observed_recharge_outcome(intent: &RechargeIntent, result: &source::Result<RechargeOrder>) -> RechargeIntent {
    let mut current = intent.clone(); current.updated_at = now(); current.persistence_pending = false;
    match result {
        Ok(order) => { current.state = "qr".into(); current.order = Some(order.clone()); current.error_code = None; }
        Err(error) => {
            current.state = if error.definitive || error.transmitted == Some(false) { "rejected" } else { "unknown" }.into();
            current.error_code = Some(error.code.clone()); current.reconciliation_id = error.reconciliation_id.clone();
        }
    }
    current
}

fn journal_connection(database: &Path) -> Result<Connection> {
    let connection = Connection::open(database)?;
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.pragma_update(None, "synchronous", "FULL")?;
    Ok(connection)
}

fn merge_purchase_journal(database: &Path, account: &str, proposals: &[PurchaseIntent]) -> Result<Vec<PurchaseIntent>> {
    let mut connection = journal_connection(database)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let encoded: Option<String> = transaction.query_row("SELECT payload FROM preferences WHERE account_key=?1 AND key=?2",
        params![account, JOURNAL_KEY], |row| row.get(0)).optional()?;
    let mut current: Vec<PurchaseIntent> = encoded.map(|value| serde_json::from_str(&value)).transpose()?.unwrap_or_default();
    anyhow::ensure!(current.len() <= 4096, "The purchase ledger exceeds its bound");
    for incoming in proposals {
        anyhow::ensure!(incoming.account == account && incoming.quote.account == account && incoming.attempts <= 1,
            "The purchase ledger account binding changed");
        if let Some(existing) = current.iter_mut().find(|existing| existing.id == incoming.id) {
            anyhow::ensure!(existing.account == incoming.account && existing.quote.fingerprint == incoming.quote.fingerprint
                && existing.attempts == incoming.attempts, "The immutable purchase intent changed");
            existing.updated_at = existing.updated_at.max(incoming.updated_at);
            existing.entitlement_observed |= incoming.entitlement_observed;
            if existing.acknowledgement.is_none() { existing.acknowledgement = incoming.acknowledgement.clone(); }
            if incoming.error_code.is_some() { existing.error_code = incoming.error_code.clone(); }
            if existing.acknowledgement.is_some() {
                existing.state = if existing.entitlement_observed { "confirmed" } else { "acknowledged" }.into();
                existing.error_code = None;
            } else if purchase_state_rank(&incoming.state) > purchase_state_rank(&existing.state) {
                existing.state = incoming.state.clone();
            }
            existing.persistence_pending = false;
        } else {
            if incoming.state == "submitting" {
                anyhow::ensure!(!current.iter().any(PurchaseIntent::unresolved), "A purchase outcome is still unresolved");
            }
            let mut inserted = incoming.clone(); inserted.persistence_pending = false;
            current.push(inserted);
        }
    }
    trim_journal(&mut current);
    transaction.execute("INSERT INTO preferences(account_key,key,payload) VALUES(?1,?2,?3)
        ON CONFLICT(account_key,key) DO UPDATE SET payload=excluded.payload",
        params![account, JOURNAL_KEY, serde_json::to_string(&current)?])?;
    transaction.commit()?;
    Ok(current)
}

fn purchase_state_rank(state: &str) -> u8 {
    match state { "submitting" => 0, "unknown" => 1, "rejected" => 2, "acknowledged" => 3, "confirmed" => 4, "preview" => 5, _ => 1 }
}

fn merge_recharge_intent(database: &Path, incoming: &RechargeIntent) -> Result<RechargeIntent> {
    let mut connection = journal_connection(database)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let encoded: Option<String> = transaction.query_row("SELECT payload FROM preferences WHERE account_key=?1 AND key=?2",
        params![incoming.account, RECHARGE_KEY], |row| row.get(0)).optional()?;
    let stored: Option<RechargeIntent> = encoded.map(|value| serde_json::from_str(&value)).transpose()?;
    let mut current = match stored {
        Some(mut stored) if stored.id == incoming.id => {
            anyhow::ensure!(stored.account == incoming.account && stored.amount_cents == incoming.amount_cents
                && stored.fingerprint == incoming.fingerprint, "The immutable recharge intent changed");
            if stored.order.is_none() { stored.order = incoming.order.clone(); }
            else if let Some(order) = &incoming.order {
                anyhow::ensure!(stored.order.as_ref().is_some_and(|existing| existing.order_id == order.order_id), "The recharge order identity changed");
            }
            if stored.reconciliation_id.is_none() { stored.reconciliation_id = incoming.reconciliation_id.clone(); }
            if incoming.error_code.is_some() { stored.error_code = incoming.error_code.clone(); }
            stored.updated_at = stored.updated_at.max(incoming.updated_at);
            let rank = |state: &str| match state { "creating" => 0, "unknown" => 1, "rejected" => 2, "qr" => 3, "observed" => 4, _ => 1 };
            if rank(&incoming.state) > rank(&stored.state) { stored.state = incoming.state.clone(); }
            if stored.order.is_some() && stored.state != "observed" { stored.state = "qr".into(); }
            stored
        }
        Some(stored) => {
            anyhow::ensure!(!stored.persistence_pending && ["rejected", "observed"].contains(&stored.state.as_str()), "A recharge outcome is still unresolved");
            incoming.clone()
        }
        None => incoming.clone(),
    };
    current.persistence_pending = false;
    transaction.execute("INSERT INTO preferences(account_key,key,payload) VALUES(?1,?2,?3)
        ON CONFLICT(account_key,key) DO UPDATE SET payload=excluded.payload",
        params![current.account, RECHARGE_KEY, serde_json::to_string(&current)?])?;
    transaction.commit()?;
    Ok(current)
}

fn payment_outbox() -> &'static Mutex<BTreeMap<String, PendingPayment>> {
    PAYMENT_OUTBOX.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn remember_payment(outcome: PendingPayment) -> String {
    let key = match &outcome {
        PendingPayment::Purchase { database, intent, .. } => format!("{}:purchase:{}", database.display(), intent.id),
        PendingPayment::Recharge { database, intent, .. } => format!("{}:recharge:{}", database.display(), intent.id),
    };
    payment_outbox().lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(key.clone(), outcome);
    OUTBOX_GENERATION.fetch_add(1, Ordering::AcqRel);
    key
}

fn forget_payment(key: &str) {
    payment_outbox().lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(key);
    OUTBOX_GENERATION.fetch_add(1, Ordering::AcqRel);
}

fn payment_outbox_has_entries() -> bool {
    !payment_outbox().lock().unwrap_or_else(std::sync::PoisonError::into_inner).is_empty()
}

pub(super) fn retry_observed_receipts() -> Result<()> {
    if OUTBOX_RETRY_ACTIVE.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_err() {
        anyhow::ensure!(!payment_outbox_has_entries(), "Observed receipt persistence is already in progress");
        return Ok(());
    }
    let _guard = OutboxRetryGuard;
    retry_payment_outbox();
    anyhow::ensure!(!payment_outbox_has_entries(), "Observed payment results still need local storage");
    Ok(())
}

fn retry_payment_outbox() {
    let entries: Vec<_> = payment_outbox().lock().unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter().map(|(key, value)| (key.clone(), value.clone())).collect();
    for (key, entry) in entries {
        let saved = match entry {
            PendingPayment::Purchase { database, intent, result } => persist_purchase_outcome(&database, &intent, result).map(|_| ()),
            PendingPayment::Recharge { database, intent, result } => persist_recharge_outcome(&database, &intent, result).map(|_| ()),
        };
        if saved.is_ok() { forget_payment(&key); }
    }
}

fn public_purchase_acknowledgement(value: &Value) -> Value {
    let mut result = serde_json::Map::new();
    result.insert("server_accepted".into(), Value::Bool(true));
    for field in ["order_id", "transaction_id", "orderId", "buy_id"] {
        if let Some(value) = value.get(field).filter(|value| value.as_u64().is_some()
            || value.as_str().is_some_and(|value| !value.is_empty() && value.len() <= 128 && !value.chars().any(char::is_control))) {
            result.insert(field.into(), value.clone());
        }
    }
    Value::Object(result)
}

fn trim_journal(journals: &mut Vec<PurchaseIntent>) {
    while journals.len() > 128 {
        if let Some(index) = journals.iter().position(|intent| !intent.unresolved()) { journals.remove(index); }
        else { break; }
    }
}

fn transaction_id(kind: &str, account: &str) -> String {
    hex_digest(format!("{kind}\0{account}\0{}\0{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(now()), std::process::id()).as_bytes())
}

fn amount(value: Option<&Value>) -> Option<f64> {
    let value = value?;
    let number = value.as_f64().or_else(|| value.as_str().filter(|value| value.len() <= 64).and_then(|value| value.parse().ok()))?;
    (number.is_finite() && (0.0..=9_007_199_254_740_991.0).contains(&number)).then_some(number)
}

fn value_true(value: Option<&Value>) -> bool { value.is_some_and(|value| value == &Value::Bool(true) || value.as_i64() == Some(1)) }

fn display_amount(value: f64) -> String {
    let raw = value.to_string();
    let (whole, fraction) = raw.split_once('.').unwrap_or((&raw, ""));
    let mut result = String::new();
    for (index, character) in whole.chars().enumerate() {
        if index > 0 && (whole.len() - index) % 3 == 0 { result.push(','); }
        result.push(character);
    }
    if !fraction.is_empty() { result.push('.'); result.push_str(fraction); }
    result
}

fn yuan_label(cents: u64) -> String { format!("{}.{:02}", cents / 100, cents % 100) }
fn byte_label(bytes: u64) -> String {
    if bytes >= 1024 * 1024 * 1024 { format!("{:.2} GB", bytes as f64 / (1024f64 * 1024f64 * 1024f64)) }
    else { format!("{} MB", bytes.div_ceil(1024 * 1024)) }
}
fn journal_label(state: &str) -> String {
    i18n::text(match state { "confirmed" => "acct_record_confirmed", "acknowledged" => "acct_record_accepted", "rejected" => "acct_record_rejected", "preview" => "acct_record_preview", _ => "acct_record_unknown" })
}

fn receipt_time(timestamp: i64) -> String {
    if timestamp <= 0 { return i18n::text("common_unknown"); }
    chrono::DateTime::from_timestamp(timestamp, 0).map(|value| {
        (value + chrono::Duration::hours(8)).format("%m-%d %H:%M").to_string()
    }).unwrap_or_else(|| i18n::text("common_unknown"))
}

fn order_scope_label(first: f64, last: Option<f64>, count: usize, include_count: bool) -> String {
    if count == 1 { return i18n::format("acct_scope_single", &[("number", display_order(first))]); }
    let scope = match last {
        Some(last) => i18n::format("acct_scope_range", &[("first", display_order(first)), ("last", display_order(last))]),
        None => i18n::format("acct_scope_from", &[("first", display_order(first)), ("count", count.to_string())]),
    };
    if include_count && last.is_some() { format!("{scope} · {}", i18n::format("acct_offer_count", &[("count", count.to_string())])) }
    else { scope }
}

fn candidate_scope_label(read: &QuoteRead, count: u32, include_count: bool) -> String {
    let id = read.basic["ep_id"].as_str().unwrap_or_default();
    let Some(anchor) = read.detail.episodes.iter().find(|episode| episode.id == id) else {
        return i18n::format("acct_offer_count", &[("count", count.to_string())]);
    };
    let mut members = read.detail.episodes.iter().filter(|episode| episode.order >= anchor.order && episode.access == EpisodeAccess::Locked).collect::<Vec<_>>();
    members.sort_by(|left, right| left.order.total_cmp(&right.order));
    let last = count.checked_sub(1).and_then(|index| members.get(index as usize)).map(|episode| episode.order);
    order_scope_label(anchor.order, last, count as usize, include_count)
}

fn quote_scope_label(quote: &OfferSnapshot, detail: Option<&ComicDetail>, anchor: Option<&Episode>, include_count: bool) -> String {
    let orders = detail.map(|detail| quote.expected_ids.iter().map(|id| {
        detail.episodes.iter().find(|episode| &episode.id == id).map(|episode| episode.order)
    }).collect::<Option<Vec<_>>>()).flatten();
    if let Some(mut orders) = orders.filter(|orders| !orders.is_empty()) {
        orders.sort_by(f64::total_cmp);
        return order_scope_label(orders[0], orders.last().copied(), orders.len(), include_count);
    }
    match &quote.request.scope {
        PurchaseScope::OrdinalRange { start_order, .. } => order_scope_label(*start_order, None, quote.expected_ids.len(), include_count),
        PurchaseScope::Single { .. } => anchor.filter(|episode| episode.id == quote.episode_id)
            .map(|episode| order_scope_label(episode.order, None, 1, include_count))
            .unwrap_or_else(|| i18n::format("acct_offer_count", &[("count", quote.expected_ids.len().to_string())])),
    }
}

fn storage_summary(database: &Path, data: &Path, account: &str) -> Result<StorageSummary> {
    let store = Store::open(database)?;
    let mut pages = BTreeMap::<String, (u64, bool)>::new();
    let mut seen = BTreeSet::new();
    let records = store.downloads(account)?;
    let retained: BTreeSet<_> = records.iter().filter(|record| record.retained)
        .map(|record| (record.episode_id.clone(), record.revision.clone())).collect();
    for record in records {
        if !seen.insert((record.episode_id.clone(), record.revision.clone())) { continue; }
        let retained_revision = retained.contains(&(record.episode_id.clone(), record.revision.clone()));
        for page in store.page_receipts(account, &record.episode_id, &record.revision)? {
            pages.entry(page.sha256).and_modify(|entry| entry.1 |= page.retained || retained_revision)
                .or_insert((page.bytes, page.retained || retained_revision));
        }
    }
    let mut summary = StorageSummary { bytes: directory_bytes(data)?, free_bytes: available_bytes(data), volume_bytes: available_volume_bytes(data), ..Default::default() };
    for (bytes, retained) in pages.into_values() {
        if retained { summary.manual_bytes = summary.manual_bytes.saturating_add(bytes); }
        else { summary.cache_bytes = summary.cache_bytes.saturating_add(bytes); }
    }
    Ok(summary)
}

fn unique_cache_bytes(store: &Store, account: &str) -> Result<u64> {
    let mut objects = BTreeMap::<String, (u64, bool)>::new();
    let mut seen = BTreeSet::new();
    let records = store.downloads(account)?;
    let retained: BTreeSet<_> = records.iter().filter(|record| record.retained)
        .map(|record| (record.episode_id.clone(), record.revision.clone())).collect();
    for record in records {
        if !seen.insert((record.episode_id.clone(), record.revision.clone())) { continue; }
        let retained_revision = retained.contains(&(record.episode_id.clone(), record.revision.clone()));
        for page in store.page_receipts(account, &record.episode_id, &record.revision)? {
            objects.entry(page.sha256).and_modify(|entry| entry.1 |= page.retained || retained_revision)
                .or_insert((page.bytes, page.retained || retained_revision));
        }
    }
    Ok(objects.values().filter(|(_, retained)| !retained).map(|(bytes, _)| *bytes).sum())
}

fn directory_bytes(directory: &Path) -> Result<u64> {
    let mut queue = vec![(directory.to_owned(), 0usize)]; let mut total = 0u64; let mut count = 0usize;
    while let Some((path, depth)) = queue.pop() {
        anyhow::ensure!(depth <= 32, "The data directory nesting is excessive");
        for entry in std::fs::read_dir(path)? {
            let entry = entry?; let metadata = std::fs::symlink_metadata(entry.path())?;
            count += 1; anyhow::ensure!(count <= 100_000, "The data directory has too many entries");
            if metadata.file_type().is_symlink() { continue; }
            if metadata.is_dir() { queue.push((entry.path(), depth + 1)); }
            else if metadata.is_file() { total = total.checked_add(metadata.len()).context("The data directory byte count overflowed")?; }
        }
    }
    Ok(total)
}

#[cfg(target_os = "linux")]
fn available_bytes(path: &Path) -> Option<u64> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    let path = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(path.as_ptr(), &mut stat) } != 0 { return None; }
    (stat.f_bavail as u64).checked_mul(stat.f_frsize as u64)
}
#[cfg(not(target_os = "linux"))]
fn available_bytes(_: &Path) -> Option<u64> { None }

#[cfg(target_os = "linux")]
fn available_volume_bytes(path: &Path) -> Option<u64> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    let path = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(path.as_ptr(), &mut stat) } != 0 { return None; }
    (stat.f_blocks as u64).checked_mul(stat.f_frsize as u64)
}
#[cfg(not(target_os = "linux"))]
fn available_volume_bytes(_: &Path) -> Option<u64> { None }

const QUOTE_MAX_AMOUNT: f64 = 9_007_199_254_740_991.0;
const QUOTE_MAX_LIMIT: u32 = 2_147_483_647;
const QUOTE_MAX_OFFERS: usize = 128;
const QUOTE_MAX_EPISODES: usize = 20_000;
const QUOTE_DISCOUNT_FIELDS: &[&str] = &[
    "discount_type", "discount", "ep_discount_type", "ep_discount",
    "discount_ep_gold", "discount_remain_gold",
];

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct OfferSnapshot {
    pub selection: String,
    pub payment: String,
    pub account: String,
    pub comic_id: String,
    pub episode_id: String,
    pub amount: f64,
    pub original_amount: f64,
    pub balance: f64,
    pub request: PurchaseRequest,
    pub expected_ids: Vec<String>,
    pub before_access: Vec<(String, EpisodeAccess)>,
    pub raw_offer_index: Option<usize>,
    pub observed_at: i64,
    pub fingerprint: String,
}

#[derive(Clone, Debug)]
pub(super) struct QuoteRead {
    pub basic: Value,
    pub scoped: Value,
    pub wallet: Value,
    pub detail: ComicDetail,
    pub observed_at: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
struct QuoteBatchOffer {
    limit: u32,
    count: u32,
    usable: bool,
    original: f64,
    display: f64,
    discounts: BTreeMap<String, f64>,
}

#[derive(Clone, Debug, Serialize)]
struct QuoteCatalogRow {
    id: String,
    ordinal: f64,
    locked: bool,
    price: f64,
    access: &'static str,
}

fn quote_invalid(code: &str) -> SourceError { SourceError::invalid(code) }
fn quote_unsupported(code: &str) -> SourceError { SourceError::capability(code) }

fn quote_raw_number(value: Option<&Value>) -> Option<f64> {
    value?.as_f64().filter(|number| number.is_finite())
}

fn quote_raw_amount(value: Option<&Value>) -> Option<f64> {
    quote_raw_number(value).filter(|number| *number >= 0.0 && *number <= QUOTE_MAX_AMOUNT)
}

// Single-chapter Value.amount in the reference accepts a bounded numeric string.
fn quote_amount(value: Option<&Value>) -> Option<f64> {
    let value = value?;
    let number = value.as_f64().or_else(|| value.as_str().filter(|text| text.len() <= 64)
        .and_then(|text| text.trim().parse::<f64>().ok()))?;
    (number.is_finite() && number >= 0.0 && number <= QUOTE_MAX_AMOUNT).then_some(number)
}

fn quote_u32(value: Option<&Value>, minimum: u32) -> Option<u32> {
    let number = quote_raw_number(value)?;
    (number >= minimum as f64 && number <= QUOTE_MAX_LIMIT as f64 && number.fract() == 0.0)
        .then_some(number as u32)
}

fn quote_id_text(value: &str) -> Option<String> {
    (!value.is_empty() && value.len() <= 15 && !value.starts_with('0')
        && value.bytes().all(|byte| byte.is_ascii_digit())).then(|| value.to_owned())
}

fn quote_id(value: Option<&Value>) -> Option<String> {
    let value = value?;
    if let Some(text) = value.as_str() { return quote_id_text(text); }
    let number = quote_raw_number(Some(value))?;
    (number >= 1.0 && number <= 999_999_999_999_999.0 && number.fract() == 0.0)
        .then(|| format!("{number:.0}"))
}

fn quote_asset_id(value: &Value) -> Option<String> {
    let text = if let Some(text) = value.as_str() { text.to_owned() }
        else {
            let number = quote_raw_amount(Some(value))?;
            if number.fract() != 0.0 { return None; }
            format!("{number:.0}")
        };
    (!text.is_empty() && text.len() <= 256 && !text.chars().any(char::is_control))
        .then_some(text)
}

fn quote_asset_ids(value: Option<&Value>) -> source::Result<Vec<String>> {
    let values = value.and_then(Value::as_array).filter(|values| !values.is_empty() && values.len() <= 1024)
        .ok_or_else(|| quote_unsupported("quote_coupon_ids_unverified"))?;
    let mut seen = BTreeSet::new();
    let mut result = Vec::with_capacity(values.len());
    for value in values {
        let id = quote_asset_id(value).ok_or_else(|| quote_unsupported("quote_coupon_ids_unverified"))?;
        if !seen.insert(id.clone()) { return Err(quote_unsupported("quote_coupon_ids_unverified")); }
        result.push(id);
    }
    Ok(result)
}

fn quote_is_true(value: Option<&Value>) -> bool {
    value == Some(&Value::Bool(true)) || quote_raw_number(value) == Some(1.0)
}

fn quote_no_expiry(value: Option<&Value>) -> bool {
    match value {
        None => true,
        Some(Value::String(text)) => text.is_empty() || text == "0000-00-00 00:00:00",
        Some(value) => value.is_number() && quote_raw_number(Some(value)) == Some(0.0),
    }
}

fn quote_raw_access(row: &Value) -> &'static str {
    if ["unavailable", "is_available", "is_purchased"].iter()
        .any(|key| row.get(*key).is_some_and(|value| !value.is_boolean()))
        || row.get("status").is_some_and(|value| quote_raw_number(Some(value)).is_none()) {
        return "unknown";
    }
    if row.get("unavailable") == Some(&Value::Bool(true))
        || row.get("is_available") == Some(&Value::Bool(false))
        || quote_raw_number(row.get("status")) == Some(501.0) { return "unavailable"; }
    if row.get("is_in_free") != Some(&Value::Bool(false))
        || !quote_no_expiry(row.get("unlock_expire_at")) || !quote_no_expiry(row.get("expires_at"))
        || quote_raw_number(row.get("unlock_type")) == Some(2.0)
        || quote_raw_number(row.get("unlock_type")) == Some(3.0) {
        return "temporary_or_unknown";
    }
    let mode = quote_raw_number(row.get("pay_mode"));
    let unlock = quote_raw_number(row.get("unlock_type"));
    if row.get("is_locked") == Some(&Value::Bool(true)) {
        if mode != Some(1.0) || unlock != Some(0.0)
            || row.get("is_purchased") == Some(&Value::Bool(true)) { return "unknown"; }
        return "locked";
    }
    if mode == Some(0.0) && unlock == Some(0.0)
        && row.get("is_purchased") != Some(&Value::Bool(true)) { return "free"; }
    if mode == Some(1.0) && unlock == Some(1.0)
        && row.get("is_purchased") != Some(&Value::Bool(false)) { return "owned"; }
    "unknown"
}

fn quote_optional_numbers(value: &Value, fields: &[&str]) -> source::Result<BTreeMap<String, f64>> {
    let mut result = BTreeMap::new();
    for field in fields {
        if let Some(raw) = value.get(*field) {
            result.insert((*field).to_owned(), quote_raw_number(Some(raw))
                .ok_or_else(|| quote_unsupported("quote_discount_fields_unverified"))?);
        }
    }
    Ok(result)
}

fn quote_batch_offers(value: &Value) -> source::Result<Vec<QuoteBatchOffer>> {
    let values = value.get("batch_buy").and_then(Value::as_array)
        .filter(|values| !values.is_empty() && values.len() <= QUOTE_MAX_OFFERS)
        .ok_or_else(|| quote_unsupported("quote_batch_offers_unverified"))?;
    values.iter().map(|value| {
        if !value.is_object() { return Err(quote_unsupported("quote_batch_offers_unverified")); }
        Ok(QuoteBatchOffer {
            limit: quote_u32(value.get("batch_limit"), 0).ok_or_else(|| quote_unsupported("quote_batch_offers_unverified"))?,
            count: quote_u32(value.get("amount"), 0).ok_or_else(|| quote_unsupported("quote_batch_offers_unverified"))?,
            usable: value.get("usable").and_then(Value::as_bool).ok_or_else(|| quote_unsupported("quote_batch_offers_unverified"))?,
            original: quote_raw_amount(value.get("original_gold")).ok_or_else(|| quote_unsupported("quote_batch_offers_unverified"))?,
            display: quote_raw_amount(value.get("pay_gold")).ok_or_else(|| quote_unsupported("quote_batch_offers_unverified"))?,
            discounts: quote_optional_numbers(value, &["discount_type", "discount", "discount_batch_gold"] )?,
        })
    }).collect()
}

fn quote_sum(left: f64, right: f64) -> source::Result<f64> {
    let result = left + right;
    if !result.is_finite() || !(0.0..=QUOTE_MAX_AMOUNT).contains(&result) {
        return Err(quote_unsupported("quote_catalog_amount_unverified"));
    }
    Ok(result)
}

fn quote_identity(value: &Value, comic: &str, episode: &str, require_comic: bool) -> source::Result<()> {
    if !value.is_object() { return Err(quote_invalid("quote_response_invalid")); }
    if value.get("ep_id").is_some_and(|raw| quote_id(Some(raw)).as_deref() != Some(episode)) {
        return Err(quote_invalid("quote_episode_mismatch"));
    }
    if require_comic && quote_id(value.get("comic_id")).as_deref() != Some(comic)
        || value.get("comic_id").is_some_and(|raw| quote_id(Some(raw)).as_deref() != Some(comic)) {
        return Err(quote_invalid("quote_comic_mismatch"));
    }
    Ok(())
}

fn quote_single_lock_context(read: &QuoteRead, access: EpisodeAccess) -> source::Result<Value> {
    let read_flag = |value: &Value| -> source::Result<Option<bool>> {
        value.get("is_locked").map(|raw| raw.as_bool()
            .or_else(|| quote_raw_number(Some(raw)).and_then(|number|
                if number == 0.0 { Some(false) } else if number == 1.0 { Some(true) } else { None }))
            .ok_or_else(|| quote_unsupported("quote_lock_state_unverified"))).transpose()
    };
    let basic = read_flag(&read.basic)?;
    let scoped = read_flag(&read.scoped)?;
    if basic.zip(scoped).is_some_and(|(left, right)| left != right)
        || access == EpisodeAccess::Locked && (basic == Some(false) || scoped == Some(false)) {
        return Err(quote_unsupported("quote_access_changed"));
    }
    Ok(json!({"basic_locked":basic,"scoped_locked":scoped}))
}

fn quote_single_scoped_amounts(read: &QuoteRead, fields: &[&str])
    -> source::Result<BTreeMap<String, f64>> {
    let mut result = BTreeMap::new();
    for field in fields {
        if let Some(raw) = read.scoped.get(*field) {
            let actual = quote_amount(Some(raw)).ok_or_else(|| quote_unsupported("quote_scoped_amount_unverified"))?;
            if quote_amount(read.basic.get(*field)) != Some(actual) {
                return Err(quote_unsupported("quote_scoped_amount_changed"));
            }
            result.insert((*field).to_owned(), actual);
        }
    }
    Ok(result)
}

fn quote_totals(value: &Value, comic: &str, episode: &str, anchor: f64,
    whole_count: u32, whole_price: f64, after_count: u32, after_price: f64)
    -> source::Result<BTreeMap<String, f64>> {
    quote_identity(value, comic, episode, true)?;
    if value.get("is_locked") != Some(&Value::Bool(true))
        || quote_u32(value.get("remain_lock_ep_num"), 0) != Some(whole_count)
        || quote_u32(value.get("after_lock_ep_num"), 1) != Some(after_count) {
        return Err(quote_unsupported("quote_catalog_totals_unverified"));
    }
    for (field, expected) in [("ep_original_gold", anchor), ("pay_gold", anchor),
        ("original_gold", whole_price), ("remain_lock_ep_gold", whole_price),
        ("after_lock_ep_gold", after_price)] {
        if quote_raw_amount(value.get(field)) != Some(expected) {
            return Err(quote_unsupported("quote_catalog_totals_unverified"));
        }
    }
    quote_optional_numbers(value, QUOTE_DISCOUNT_FIELDS)
}

pub(super) fn normalize_offer(account: &str, episode_id: &str, selection: &str,
    payment: &str, read: &QuoteRead) -> source::Result<OfferSnapshot> {
    if account.is_empty() || account.len() > 256 || account.chars().any(char::is_control)
        || quote_id_text(episode_id).is_none() {
        return Err(quote_invalid("quote_identity_invalid"));
    }
    if payment != "coin" && payment != "coupon" { return Err(quote_invalid("quote_payment_invalid")); }
    let comic_id = quote_id_text(&read.detail.comic.id).ok_or_else(|| quote_invalid("quote_comic_identity_invalid"))?;
    let mut catalog = BTreeMap::new();
    for episode in &read.detail.episodes {
        if quote_id_text(&episode.id).is_none() || episode.comic_id != comic_id
            || catalog.insert(episode.id.clone(), episode).is_some() {
            return Err(quote_invalid("quote_catalog_identity_invalid"));
        }
    }
    let current = catalog.get(episode_id).copied().ok_or_else(|| quote_unsupported("quote_episode_access_unverified"))?;
    if !current.order.is_finite() || !matches!(current.access, EpisodeAccess::Locked | EpisodeAccess::Temporary) {
        return Err(quote_unsupported("quote_episode_not_purchasable"));
    }
    quote_identity(&read.basic, &comic_id, episode_id, false)?;
    quote_identity(&read.scoped, &comic_id, episode_id, false)?;
    let mut request = PurchaseRequest {
        scope: PurchaseScope::Single { episode_id: episode_id.to_owned(), comic_id: Some(comic_id.clone()) },
        method: if payment == "coin" { PurchaseMethod::Currency } else { PurchaseMethod::Coupon },
        pay_amount: None, coupon_id: None, coupon_ids: Vec::new(), free_gold_card_id: None, free_gold_amount: None,
    };
    let amount;
    let original_amount;
    let balance;
    let expected_ids;
    let raw_offer_index;
    let proof;
    if selection == "single" {
        expected_ids = vec![episode_id.to_owned()];
        raw_offer_index = None;
        let lock_context = quote_single_lock_context(read, current.access)?;
        if payment == "coin" {
            original_amount = quote_amount(read.basic.get("ep_original_gold"))
                .ok_or_else(|| quote_unsupported("quote_amount_unverified"))?;
            amount = quote_amount(read.basic.get("pay_gold"))
                .filter(|display| *display == original_amount)
                .ok_or_else(|| quote_unsupported("quote_amount_unverified"))?;
            balance = quote_amount(read.basic.get("remain_gold"))
                .ok_or_else(|| quote_unsupported("quote_balance_unverified"))?;
            request.pay_amount = (amount > 0.0).then_some(amount);
            proof = json!({"kind":"single_standard_currency", "original":original_amount,
                "display":amount, "lock_context":lock_context,
                "scoped_amounts":quote_single_scoped_amounts(read, &["ep_original_gold", "pay_gold", "remain_gold"])?,
                "discounts":quote_optional_numbers(&read.basic, QUOTE_DISCOUNT_FIELDS)?});
        } else {
            if !quote_is_true(read.basic.get("allow_coupon")) { return Err(quote_unsupported("quote_coupon_ineligible")); }
            amount = quote_amount(read.basic.get("ep_pay_coupons"))
                .filter(|amount| *amount >= 1.0 && *amount <= 1024.0 && amount.fract() == 0.0)
                .ok_or_else(|| quote_unsupported("quote_coupon_amount_unverified"))?;
            balance = quote_amount(read.basic.get("remain_coupon"))
                .ok_or_else(|| quote_unsupported("quote_balance_unverified"))?;
            let selected = quote_asset_ids(read.basic.get("recommend_coupon_ids"))?;
            let eligible = quote_asset_ids(read.basic.get("eligible_coupon_ids")
                .or_else(|| read.basic.get("recommend_coupon_ids")))?;
            let eligible_set: BTreeSet<_> = eligible.iter().collect();
            if selected.len() != amount as usize || selected.iter().any(|id| !eligible_set.contains(id)) {
                return Err(quote_unsupported("quote_coupon_ids_unverified"));
            }
            if read.scoped.get("allow_coupon").is_some() && !quote_is_true(read.scoped.get("allow_coupon")) {
                return Err(quote_unsupported("quote_coupon_eligibility_changed"));
            }
            let scoped_selected = read.scoped.get("recommend_coupon_ids")
                .map(|raw| quote_asset_ids(Some(raw))).transpose()?;
            if scoped_selected.as_ref().is_some_and(|ids| ids != &selected) {
                return Err(quote_unsupported("quote_coupon_selection_changed"));
            }
            let scoped_eligible = read.scoped.get("eligible_coupon_ids")
                .map(|raw| quote_asset_ids(Some(raw))).transpose()?;
            if scoped_eligible.as_ref().is_some_and(|ids| selected.iter().any(|id| !ids.contains(id))) {
                return Err(quote_unsupported("quote_coupon_eligibility_changed"));
            }
            request.coupon_ids = selected.clone();
            original_amount = amount;
            proof = json!({"kind":"single_reading_coupons", "required":amount,
                "selected_coupon_ids":selected, "eligible_coupon_ids":eligible,
                "lock_context":lock_context,
                "scoped_amounts":quote_single_scoped_amounts(read, &["ep_pay_coupons", "remain_coupon"])?,
                "scoped_selected_coupon_ids":scoped_selected,"scoped_eligible_coupon_ids":scoped_eligible});
        }
    } else {
        if payment != "coin" { return Err(quote_unsupported("quote_batch_coupon_unsupported")); }
        let suffix = selection.strip_prefix("batch:").ok_or_else(|| quote_invalid("quote_selection_invalid"))?;
        let index = suffix.parse::<usize>().ok().filter(|index| *index >= 1 && *index <= QUOTE_MAX_OFFERS
            && index.to_string() == suffix).ok_or_else(|| quote_invalid("quote_selection_invalid"))?;
        let basic_offers = quote_batch_offers(&read.basic)?;
        let scoped_offers = quote_batch_offers(&read.scoped)?;
        if basic_offers != scoped_offers { return Err(quote_unsupported("quote_offers_changed")); }
        let offer = basic_offers.get(index - 1).ok_or_else(|| quote_unsupported("quote_offer_missing"))?;
        if !offer.usable || offer.count == 0 { return Err(quote_unsupported("quote_offer_unavailable")); }
        if quote_id(read.detail.extra.get("id").or_else(|| read.detail.extra.get("comic_id"))).as_deref() != Some(comic_id.as_str()) {
            return Err(quote_unsupported("quote_original_catalog_unverified"));
        }
        let raw_rows = read.detail.extra.get("ep_list").and_then(Value::as_array)
            .filter(|rows| !rows.is_empty() && rows.len() <= QUOTE_MAX_EPISODES)
            .ok_or_else(|| quote_unsupported("quote_original_catalog_unverified"))?;
        let mut rows: Vec<QuoteCatalogRow> = Vec::with_capacity(raw_rows.len());
        let mut seen_ids = BTreeSet::new();
        let mut anchor_index = None;
        let mut whole_count = 0_u32;
        let mut whole_price = 0.0;
        for raw in raw_rows.iter().rev() {
            if !raw.is_object() { return Err(quote_unsupported("quote_original_catalog_unverified")); }
            let id = quote_id(raw.get("id")).ok_or_else(|| quote_unsupported("quote_original_catalog_unverified"))?;
            let ordinal = quote_raw_number(raw.get("ord")).ok_or_else(|| quote_unsupported("quote_catalog_order_unverified"))?;
            let locked = raw.get("is_locked").and_then(Value::as_bool).ok_or_else(|| quote_unsupported("quote_original_catalog_unverified"))?;
            let price = quote_raw_amount(raw.get("pay_gold")).ok_or_else(|| quote_unsupported("quote_catalog_amount_unverified"))?;
            if !seen_ids.insert(id.clone()) || rows.last().is_some_and(|previous| ordinal <= previous.ordinal)
                || raw.get("comic_id").is_some_and(|value| quote_id(Some(value)).as_deref() != Some(comic_id.as_str())) {
                return Err(quote_unsupported("quote_catalog_order_unverified"));
            }
            if id == episode_id { anchor_index = Some(rows.len()); }
            if locked { whole_count += 1; whole_price = quote_sum(whole_price, price)?; }
            rows.push(QuoteCatalogRow { id, ordinal, locked, price, access: quote_raw_access(raw) });
        }
        let anchor_index = anchor_index.ok_or_else(|| quote_unsupported("quote_anchor_unverified"))?;
        let anchor = &rows[anchor_index];
        if anchor.access != "locked" || current.order != anchor.ordinal {
            return Err(quote_unsupported("quote_anchor_unverified"));
        }
        let mut after = Vec::new();
        let mut after_price = 0.0;
        for row in &rows[anchor_index..] {
            if !matches!(row.access, "locked" | "free" | "owned") {
                return Err(quote_unsupported("quote_range_access_unverified"));
            }
            if row.locked { after.push(row); after_price = quote_sum(after_price, row.price)?; }
        }
        let basic_discounts = quote_totals(&read.basic, &comic_id, episode_id, anchor.price,
            whole_count, whole_price, after.len() as u32, after_price)?;
        let scoped_discounts = quote_totals(&read.scoped, &comic_id, episode_id, anchor.price,
            whole_count, whole_price, after.len() as u32, after_price)?;
        if basic_discounts != scoped_discounts { return Err(quote_unsupported("quote_discounts_changed")); }
        let count = if offer.limit == 0 { after.len() } else { offer.limit as usize };
        if count == 0 || count > after.len() || offer.count as usize != count {
            return Err(quote_unsupported("quote_range_count_unverified"));
        }
        let selected = &after[..count];
        let mut selected_price = 0.0;
        for row in selected { selected_price = quote_sum(selected_price, row.price)?; }
        if selected_price != offer.original || selected_price != offer.display {
            return Err(quote_unsupported("quote_range_amount_unverified"));
        }
        expected_ids = selected.iter().map(|row| row.id.clone()).collect::<Vec<_>>();
        if expected_ids.iter().any(|id| catalog.get(id).is_none_or(|episode| episode.access != EpisodeAccess::Locked)) {
            return Err(quote_unsupported("quote_normalized_access_unverified"));
        }
        amount = offer.original;
        original_amount = offer.original;
        balance = quote_amount(read.basic.get("remain_gold"))
            .ok_or_else(|| quote_unsupported("quote_balance_unverified"))?;
        request.scope = PurchaseScope::OrdinalRange { comic_id: comic_id.clone(), start_order: anchor.ordinal, limit: offer.limit };
        request.pay_amount = (amount > 0.0).then_some(amount);
        raw_offer_index = Some(index);
        proof = json!({"contract":"bilibili_pc_ordinal_range_v1",
            "provenance":"primary_sdk_ordinal_contract_and_quote_catalog_consistency",
            "server_confirmed_ids":false, "raw_source":"ComicDetail.ep_list",
            "catalog":rows, "offers":basic_offers, "discounts":basic_discounts,
            "anchor_ordinal":anchor.ordinal, "whole_count":whole_count, "whole_price":whole_price,
            "after_count":after.len(), "after_price":after_price});
    }
    let before_access: Vec<_> = expected_ids.iter().map(|id| {
        let episode = catalog.get(id).expect("Selected IDs were resolved against this catalog");
        (id.clone(), episode.access)
    }).collect();
    let access_evidence: Vec<_> = expected_ids.iter().map(|id| {
        let episode = catalog.get(id).expect("Selected IDs were resolved against this catalog");
        json!({"id":id,"access":episode.access,"expires_at":episode.expires_at,"order":episode.order})
    }).collect();
    let semantic = json!({"schema_version":1,"account":account,"comic_id":comic_id,
        "episode_id":episode_id,"selection":selection,"payment":payment,"request":request,
        "raw_offer_index":raw_offer_index,"amount":amount,"original_amount":original_amount,
        "balance":balance,"can_afford":balance >= amount,"expected_ids":expected_ids,
        "before_access":access_evidence,"proof":proof});
    let encoded = serde_json::to_vec(&semantic).map_err(|_| quote_invalid("quote_fingerprint_invalid"))?;
    let fingerprint = format!("sha256:{:x}", Sha256::digest(encoded));
    Ok(OfferSnapshot { selection: selection.to_owned(), payment: payment.to_owned(), account: account.to_owned(),
        comic_id, episode_id: episode_id.to_owned(), amount, original_amount, balance, request,
        expected_ids, before_access, raw_offer_index, observed_at: read.observed_at, fingerprint })
}

// Candidate visibility is not evidence that normalize_offer will permit submission.
// The returned batch IDs retain the original one-based offer positions.
pub(super) fn quote_selections(read: &QuoteRead) -> Vec<(String, u32, f64, f64, bool)> {
    let mut result = Vec::new();
    if let (Some(display), Some(original)) = (quote_amount(read.basic.get("pay_gold")),
        quote_amount(read.basic.get("ep_original_gold"))) {
        result.push(("single".into(), 1, display, original,
            read.basic.get("is_locked").and_then(Value::as_bool).unwrap_or(false)));
    }
    let Some(offers) = read.basic.get("batch_buy").and_then(Value::as_array)
        .filter(|offers| offers.len() <= QUOTE_MAX_OFFERS) else { return result; };
    for (index, offer) in offers.iter().enumerate() {
        if let (Some(count), Some(display), Some(original)) = (quote_u32(offer.get("amount"), 0),
            quote_raw_amount(offer.get("pay_gold")), quote_raw_amount(offer.get("original_gold"))) {
            result.push((format!("batch:{}", index + 1), count, display, original,
                offer.get("usable").and_then(Value::as_bool).unwrap_or(false)));
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn single_read() -> QuoteRead {
        let value = json!({"ep_id":"7","comic_id":"9","is_locked":true,
            "ep_original_gold":32,"pay_gold":32,"remain_gold":1280,
            "remain_coupon":2,"allow_coupon":true,"ep_pay_coupons":1,
            "recommend_coupon_ids":["coupon-a"],"eligible_coupon_ids":["coupon-a","coupon-b"]});
        QuoteRead {
            basic: value.clone(), scoped: value, wallet: json!({"remain_gold":1280,"remain_coupon":2}),
            detail: ComicDetail { comic: Comic { id: "9".into(), ..Default::default() },
                episodes: vec![Episode { id: "7".into(), comic_id: "9".into(), order: 13.0,
                    access: EpisodeAccess::Locked, pay_gold: Some(32.0), ..Default::default() }], extra: Value::Null },
            observed_at: 1_800_000_000,
        }
    }

    fn intent(id: &str) -> PurchaseIntent {
        PurchaseIntent { id: id.into(), account: "bili_1".into(),
            quote: normalize_offer("bili_1", "7", "single", "coin", &single_read()).unwrap(),
            state: "submitting".into(), created_at: 1, updated_at: 1, attempts: 1,
            acknowledgement: None, error_code: None, entitlement_observed: false, persistence_pending: false }
    }

    #[test]
    fn quote_fingerprint_ignores_fetch_time_but_binds_balance_and_assets() {
        let read = single_read();
        let original = normalize_offer("bili_1", "7", "single", "coin", &read).unwrap();
        let mut later = read.clone(); later.observed_at += 10;
        assert_eq!(original.fingerprint, normalize_offer("bili_1", "7", "single", "coin", &later).unwrap().fingerprint);
        later.basic["remain_gold"] = json!(1279); later.scoped["remain_gold"] = json!(1279);
        assert_ne!(original.fingerprint, normalize_offer("bili_1", "7", "single", "coin", &later).unwrap().fingerprint);
        later.basic.as_object_mut().unwrap().remove("pay_gold");
        later.wallet["remain_gold"] = json!(1_000_000);
        assert!(normalize_offer("bili_1", "7", "single", "coin", &later).is_err());
        let mut coupon = read;
        coupon.basic["recommend_coupon_ids"] = json!(["unconfirmed-coupon"]);
        coupon.scoped["recommend_coupon_ids"] = json!(["unconfirmed-coupon"]);
        assert!(normalize_offer("bili_1", "7", "single", "coupon", &coupon).is_err());
    }

    #[test]
    fn a_wallet_refresh_cannot_splice_a_new_balance_into_an_old_quote() {
        let mut read = single_read();
        read.basic["remain_gold"] = json!(20); read.scoped["remain_gold"] = json!(20);
        read.wallet["remain_gold"] = json!(20);
        let before = normalize_offer("bili_1", "7", "single", "coin", &read).unwrap();
        assert!(before.balance < before.amount);
        read.wallet["remain_gold"] = json!(1280);
        let wallet_only = normalize_offer("bili_1", "7", "single", "coin", &read).unwrap();
        assert_eq!(wallet_only.fingerprint, before.fingerprint);
        assert!(wallet_only.balance < wallet_only.amount);
        read.basic["remain_gold"] = json!(1280); read.scoped["remain_gold"] = json!(1280);
        let refreshed = normalize_offer("bili_1", "7", "single", "coin", &read).unwrap();
        assert_ne!(refreshed.fingerprint, before.fingerprint);
        assert!(refreshed.balance >= refreshed.amount);
    }

    #[test]
    fn ordinal_offer_requires_complete_catalog_proof_and_preserves_a_zero_limit() {
        let mut read = single_read();
        read.detail.episodes = [("6", 12.0, EpisodeAccess::Locked), ("7", 13.5, EpisodeAccess::Locked),
            ("8", 14.0, EpisodeAccess::Owned), ("9", 15.0, EpisodeAccess::Locked)].into_iter().map(|(id, order, access)| {
            Episode { id: id.into(), comic_id: "9".into(), order, access, pay_gold: Some(32.0), ..Default::default() }
        }).collect();
        let raw: Vec<_> = read.detail.episodes.iter().rev().map(|episode| {
            let locked = episode.access == EpisodeAccess::Locked;
            json!({"id":episode.id,"comic_id":"9","ord":episode.order,"is_locked":locked,
                "pay_gold":32,"pay_mode":1,"unlock_type":if locked {0} else {1},
                "is_in_free":false,"is_purchased":!locked,"is_available":true,
                "unlock_expire_at":"0000-00-00 00:00:00"})
        }).collect();
        read.detail.extra = json!({"id":"9","ep_list":raw});
        let value = json!({"ep_id":"7","comic_id":"9","is_locked":true,"ep_original_gold":32,
            "pay_gold":32,"remain_gold":1280,"original_gold":96,"remain_lock_ep_num":3,
            "remain_lock_ep_gold":96,"after_lock_ep_num":2,"after_lock_ep_gold":64,
            "batch_buy":[{"batch_limit":2,"amount":2,"usable":true,"original_gold":64,"pay_gold":64}]});
        read.basic = value.clone(); read.scoped = value;
        let quote = normalize_offer("bili_1", "7", "batch:1", "coin", &read).unwrap();
        assert_eq!(quote.expected_ids, vec!["7", "9"]);
        assert_eq!(quote.amount, 64.0);
        assert!(matches!(quote.request.scope, PurchaseScope::OrdinalRange { start_order, limit: 2, .. } if start_order == 13.5));
        read.basic["batch_buy"][0]["batch_limit"] = json!(0);
        read.scoped["batch_buy"][0]["batch_limit"] = json!(0);
        let remaining = normalize_offer("bili_1", "7", "batch:1", "coin", &read).unwrap();
        assert!(matches!(remaining.request.scope, PurchaseScope::OrdinalRange { limit: 0, .. }));
        read.scoped["after_lock_ep_gold"] = json!(63);
        assert!(normalize_offer("bili_1", "7", "batch:1", "coin", &read).is_err());
    }

    #[test]
    fn stale_account_snapshot_cannot_downgrade_a_persisted_acknowledgement() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let store = Store::open(directory.path().join("state.sqlite3"))?;
        let pending = intent("purchase-a");
        merge_purchase_journal(store.path(), "bili_1", std::slice::from_ref(&pending))?;
        let accepted = persist_purchase_outcome(store.path(), &pending, Ok(json!({"order_id":"receipt-42","private_url":"never-store"})))?;
        assert_eq!(accepted.state, "acknowledged");
        let mut stale = pending;
        stale.state = "unknown".into(); stale.updated_at = accepted.updated_at + 100;
        let merged = merge_purchase_journal(store.path(), "bili_1", &[stale.clone()])?;
        assert_eq!(merged[0].state, "acknowledged");
        assert_eq!(merged[0].acknowledgement.as_ref().unwrap()["order_id"], "receipt-42");
        assert!(merged[0].acknowledgement.as_ref().unwrap().get("private_url").is_none());
        assert!(store.preference::<Vec<PurchaseIntent>>("bili_2", JOURNAL_KEY)?.is_none());
        stale.entitlement_observed = true;
        let merged = merge_purchase_journal(store.path(), "bili_1", &[stale])?;
        assert_eq!(merged[0].state, "confirmed");
        Ok(())
    }

    #[test]
    fn entitlement_observation_without_server_ack_keeps_the_transaction_unknown() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let store = Store::open(directory.path().join("state.sqlite3"))?;
        let mut unknown = intent("unknown-purchase"); unknown.state = "unknown".into(); unknown.entitlement_observed = true;
        let saved = merge_purchase_journal(store.path(), "bili_1", &[unknown])?;
        assert_eq!(saved[0].state, "unknown");
        assert!(saved[0].unresolved());
        assert!(merge_purchase_journal(store.path(), "bili_1", &[intent("second-purchase")]).is_err());
        Ok(())
    }

    #[test]
    fn concurrent_stale_views_can_reserve_only_one_dispatch_intent() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("state.sqlite3");
        let _store = Store::open(&database)?;
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let workers: Vec<_> = (0..2).map(|index| {
            let database = database.clone(); let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                merge_purchase_journal(&database, "bili_1", &[intent(&format!("intent-{index}"))]).is_ok()
            })
        }).collect();
        let successes = workers.into_iter().map(|worker| worker.join().unwrap()).filter(|success| *success).count();
        assert_eq!(successes, 1);
        Ok(())
    }

    #[test]
    fn observed_purchase_and_recharge_results_survive_storage_failure_and_local_retry() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("blocked.sqlite3");
        std::fs::create_dir(&database)?;
        let purchase = intent("outbox-purchase");
        let purchase_result = Ok(json!({"order_id":"observed-receipt"}));
        let purchase_key = remember_payment(PendingPayment::Purchase { database: database.clone(), intent: purchase.clone(), result: purchase_result.clone() });
        assert!(persist_purchase_outcome(&database, &purchase, purchase_result).is_err());
        let recharge = RechargeIntent { id: "outbox-recharge".into(), account: "bili_1".into(), amount_cents: 500,
            fingerprint: "a".repeat(64), state: "creating".into(), created_at: 1, updated_at: 1,
            order: None, reconciliation_id: None, error_code: None, persistence_pending: false };
        let order = RechargeOrder { order_id: "90071992547409930".into(), code_url: "https://pay.bilibili.com/controlled-fixture".into(), amount_cents: 500, observed_at: 2 };
        let recharge_key = remember_payment(PendingPayment::Recharge { database: database.clone(), intent: recharge.clone(), result: Ok(order.clone()) });
        assert!(persist_recharge_outcome(&database, &recharge, Ok(order.clone())).is_err());
        {
            let held = payment_outbox().lock().unwrap();
            assert!(held.contains_key(&purchase_key) && held.contains_key(&recharge_key));
        }
        std::fs::remove_dir(&database)?;
        let store = Store::open(&database)?;
        merge_purchase_journal(&database, "bili_1", &[purchase])?;
        merge_recharge_intent(&database, &recharge)?;
        retry_payment_outbox();
        let saved: Vec<PurchaseIntent> = store.preference("bili_1", JOURNAL_KEY)?.unwrap();
        assert_eq!(saved[0].state, "acknowledged");
        assert_eq!(saved[0].acknowledgement.as_ref().unwrap()["order_id"], "observed-receipt");
        let saved: RechargeIntent = store.preference("bili_1", RECHARGE_KEY)?.unwrap();
        let saved_order = saved.order.unwrap();
        assert_eq!(saved_order.order_id, order.order_id);
        assert_eq!(saved_order.code_url, order.code_url);
        let held = payment_outbox().lock().unwrap();
        assert!(!held.contains_key(&purchase_key) && !held.contains_key(&recharge_key));
        Ok(())
    }

    #[test]
    fn account_preparation_moves_recovered_plain_data_from_a_worker() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("state.sqlite3");
        let store = Store::open(&database)?;
        store.set_preference("bili_1", "wallet", &WalletSnapshot {
            value: json!({"remain_gold":1280,"remain_coupon":2}), observed_at: 77,
        })?;
        merge_purchase_journal(&database, "bili_1", &[intent("prepared-purchase")])?;
        merge_recharge_intent(&database, &RechargeIntent {
            id: "prepared-recharge".into(), account: "bili_1".into(), amount_cents: 500,
            fingerprint: "f".repeat(64), state: "creating".into(), created_at: 1, updated_at: 1,
            order: None, reconciliation_id: None, error_code: None, persistence_pending: false,
        })?;
        let prepared = std::thread::spawn(move || -> Result<PreparedAccount> {
            let worker_store = Store::open(database)?;
            PreparedAccount::load(&worker_store, "bili_1", None)
        }).join().map_err(|_| anyhow::anyhow!("Account preparation worker failed"))??;
        assert_eq!(prepared.wallet_at, 77);
        assert_eq!(prepared.wallet.as_ref().unwrap()["remain_gold"], 1280);
        assert_eq!(prepared.journals[0].state, "unknown");
        assert_eq!(prepared.journals[0].attempts, 1);
        assert!(prepared.journals[0].acknowledgement.is_none());
        assert_eq!(prepared.recharge.as_ref().unwrap().state, "unknown");
        let saved: Vec<PurchaseIntent> = store.preference("bili_1", JOURNAL_KEY)?.unwrap();
        assert_eq!(saved[0].state, "unknown");
        Ok(())
    }

    #[test]
    fn design_preview_retains_all_three_prices_and_coupon_cost_without_live_proof() {
        let fixture: Value = serde_json::from_str(include_str!("../../assets/demo/catalog.json")).unwrap();
        let detail = ComicDetail { comic: Comic { id: "28601".into(), ..Default::default() },
            episodes: (13_u32..=22).map(|ordinal| Episode { id: (2_860_100 + ordinal).to_string(), comic_id: "28601".into(),
                order: f64::from(ordinal), access: EpisodeAccess::Locked, pay_gold: Some(32.0), ..Default::default() }).collect(), extra: Value::Null };
        let episode = detail.episodes[0].clone();
        let read = preview_quote_read_fixture(&fixture, &episode, detail).unwrap();
        let prices: Vec<_> = quote_selections(&read).into_iter().map(|(_, count, price, _, _)| (count, price)).collect();
        assert_eq!(prices, vec![(1, 32.0), (5, 150.0), (10, 288.0)]);
        assert_eq!(preview_offer("design-demo", "2860113", "batch:1", "coin", &read).unwrap().expected_ids.len(), 5);
        assert_eq!(preview_offer("design-demo", "2860113", "batch:2", "coin", &read).unwrap().amount, 288.0);
        let coupon = preview_offer("design-demo", "2860113", "single", "coupon", &read).unwrap();
        assert_eq!((coupon.amount, coupon.balance), (1.0, 2.0));
        assert!(coupon.request.coupon_ids.is_empty());
        assert!(normalize_offer("bili_1", "2860113", "batch:1", "coin", &read).is_err());
    }
}

#[cfg(test)]
#[path = "account_resource_tests.rs"]
mod resource_tests;
