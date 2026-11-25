use super::*;
use crate::{
    domain::EpisodeAccess,
    source::{PurchaseMethod, PurchaseRequest, PurchaseScope, QuoteScope, RechargeConfig, RechargeOrder, SourceError},
    reader::FitMode,
    ui::{AccountInfoItem, AccountRecordItem, PurchaseIncludedItem, PurchaseRangeItem},
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
    wallet_refreshed: bool,
    cache_cleared: bool,
    diagnostics_exported: bool,
    session_imported: bool,
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
    storage_generation: u64,
    outbox_checked: Option<Instant>,
    outbox_generation: u64,
    recharge: RechargeUi,
}

impl Default for AccountState {
    fn default() -> Self {
        Self {
            section: "home".into(), wallet: None, wallet_at: 0, wallet_attempted: false,
            wallet_refreshed: false, cache_cleared: false, diagnostics_exported: false, session_imported: false,
            busy: false, message: String::new(), comic: None, episode: None,
            selection: "single".into(), payment: "coin".into(), phase: "review".into(),
            quote_loading: false, quote: None, quote_read: None, quote_error: String::new(),
            journals: Vec::new(), current_journal: None, reconciling: false,
            storage: None, storage_pending: false, storage_checked: None, storage_generation: 0,
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
    coin_amount: Option<u64>,
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
    config: Option<RechargeConfig>,
    selected: usize,
    journal: Option<RechargeIntent>,
    qr: slint::Image,
    message: String,
    busy: bool,
    history: Option<Value>,
    history_loading: bool,
    history_failed: bool,
}

impl RechargeUi {
    fn present_intent(&mut self, intent: &RechargeIntent) {
        self.qr = slint::Image::default();
        let (phase, message) = if intent.persistence_pending {
            ("pending", "acct_receipt_storage_pending")
        } else if intent.state == "rejected" {
            ("error", "acct_recharge_rejected")
        } else if let Some(order) = &intent.order {
            match super::auth::qr_image(&order.code_url) {
                Ok(qr) => { self.qr = qr; ("qr", "") }
                Err(_) => ("pending", "acct_recharge_qr_unavailable"),
            }
        } else {
            ("pending", "acct_recharge_unknown")
        };
        self.phase = phase.into();
        self.message = if message.is_empty() { String::new() } else { i18n::text(message) };
    }
}

#[derive(Clone, Default)]
struct StorageSummary {
    manual_bytes: u64,
    cache_bytes: u64,
    free_bytes: Option<u64>,
}

impl StorageSummary {
    fn used_bytes(&self) -> u64 { self.manual_bytes.saturating_add(self.cache_bytes) }

    fn composition(&self) -> (f32, f32) {
        let total = self.used_bytes();
        if total == 0 { return (0.0, 0.0); }
        (self.manual_bytes as f32 / total as f32, self.cache_bytes as f32 / total as f32)
    }
}

/// Account startup data contains no UI object and can move from the preparation worker.
pub(super) struct PreparedAccount {
    wallet: Option<Value>,
    wallet_at: i64,
    journals: Vec<PurchaseIntent>,
    recharge: Option<RechargeIntent>,
    demo_defaults_allowed: bool,
}

impl PreparedAccount {
    pub(super) fn load(store: &Store, account: &str, demo: Option<&DemoData>) -> Result<Self> {
        let demo_defaults_allowed = demo.is_some() && store.preference::<Preferences>(account, "preferences")?.is_none();
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
        Ok(Self { wallet, wallet_at, journals, recharge, demo_defaults_allowed })
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
                if self.account_ui.recharge.visible && self.account_ui.recharge.journal.as_ref()
                    .is_some_and(|previous| previous.id == intent.id && previous.persistence_pending)
                {
                    self.account_ui.recharge.present_intent(&intent);
                }
                self.account_ui.recharge.journal = Some(intent);
            }
            self.account_ui.outbox_generation = generation;
            self.dirty = true;
        }
        if matches!(self.route.as_str(), "account" | "downloads") { self.schedule_storage_summary(); }
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
        let demo_defaults_allowed = prepared.demo_defaults_allowed;
        self.account_ui = AccountState::default();
        self.account_ui.wallet = prepared.wallet;
        self.account_ui.wallet_at = prepared.wallet_at;
        self.account_ui.journals = prepared.journals;
        self.account_ui.recharge.journal = prepared.recharge;
        if let Some(demo) = self.demo.as_ref().filter(|_| demo_defaults_allowed) {
            let defaults = &demo.fixture["account"]["defaults"];
            if let Some(value) = defaults["reading_mode"].as_str().filter(|value| ["page", "panel", "strip"].contains(value)) { self.preferences.mode = value.into(); }
            if let Some(value) = defaults["direction"].as_str().filter(|value| ["ltr", "rtl"].contains(value)) { self.preferences.direction = value.into(); }
            if let Some(value) = defaults["refresh"].as_str().filter(|value| ["smart", "0", "1", "6", "12"].contains(value)) { self.preferences.every = value.into(); }
            if let Some(value) = defaults["launch"].as_str().filter(|value| ["resume", "shelf"].contains(value)) { self.preferences.launch = value.into(); }
            if let Some(value) = defaults["sleep_cover"].as_str() { self.preferences.sleep_cover = value == "cover"; }
        }
        if self.demo.is_none() {
            self.preferences.smart = false;
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
        if self.route == "account" && signed_in && self.demo.is_none() && !self.options.offline {
            if !self.account_ui.wallet_attempted && self.account_ui.wallet.is_none() { self.refresh_wallet(false); }
            if self.account_ui.section == "account" && self.account_ui.recharge.history.is_none()
                && !self.account_ui.recharge.history_failed
            { self.load_recharge_history(); }
        }
        let account = ui.global::<AccountModel>();
        account.set_section(self.account_section().into());
        account.set_signed_in(signed_in);
        let name = self.demo.as_ref().map(|demo| demo.account_name.clone())
            .or_else(|| session.as_ref().map(|session| session.account_name().to_owned())).unwrap_or_default();
        account.set_initial(name.chars().next().map(|value| value.to_string()).unwrap_or_default().into());
        account.set_username(name.into());
        account.set_session_status(tr(if self.demo.is_some() { "acct_preview_session" } else { "acct_session_active" }));
        let (coins, coupons) = self.account_wallet_labels();
        account.set_balance(coins.into());
        account.set_coupons(coupons.into());
        account.set_wallet_refreshed(self.account_ui.wallet_refreshed);
        account.set_busy(self.account_ui.busy);
        account.set_message(self.account_ui.message.clone().into());
        account.set_mode(self.preferences.mode.clone().into());
        account.set_fit(FitMode::parse(&self.preferences.fit).unwrap_or_default().as_str().into());
        account.set_direction(self.preferences.direction.clone().into());
        account.set_every(self.preferences.every.clone().into());
        let fast_refresh = self.preferences.refresh_mode == "fast";
        account.set_refresh_mode(if fast_refresh { "fast" } else { "normal" }.into());
        account.set_refresh_mode_note(i18n::text(if fast_refresh {
            "refresh_mode_fast_note"
        } else {
            "refresh_mode_normal_note"
        }).into());
        account.set_dither(self.preferences.dither);
        account.set_dither_method(normalized_dither_method(&self.preferences.dither_method).into());
        let algorithm = selected_spatial_dither(self.preferences.dither, &self.preferences.dither_method);
        let note = if fast_refresh {
            format!("{} {}", i18n::text("spatial_dither_fast_saved_note"), spatial_dither_note(algorithm))
        } else { spatial_dither_note(algorithm) };
        account.set_dither_note(note.into());
        account.set_landscape(self.preferences.landscape.clone().into());
        let media_availability = crate::native_media::availability();
        account.set_jpeg_acceleration(self.preferences.jpeg_acceleration);
        account.set_jpeg_acceleration_available(media_availability.available());
        account.set_jpeg_acceleration_note(crate::native_media_ui::availability_note(media_availability).into());
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
        account.set_can_sleep_cover(self.demo.is_some() || self.platform.is_kindle);
        account.set_can_smart(self.demo.is_some());
        account.set_can_update(false);
        account.set_version(env!("CARGO_PKG_VERSION").into());
        account.set_device_short(self.options.profile.name().trim_start_matches("Kindle ").replace(" (2022 / 2024)", "").into());
        account.set_firmware(std::env::var("BILICOMICS_APPROVED_FIRMWARE")
            .unwrap_or_else(|_| i18n::text("common_unknown")).into());
        account.set_data_directory(self.data_dir.display().to_string().into());
        account.set_diagnostics_result(if self.account_ui.diagnostics_exported { tr("acct_exported") } else { "".into() });
        account.set_session_imported(self.account_ui.session_imported);
        if let Some(demo) = &self.demo {
            let presentation = &demo.fixture["account"]["presentation"];
            if let Some(value) = presentation["firmware"].as_str() { account.set_firmware(value.into()); }
            if let Some(value) = presentation["data_directory"].as_str() { account.set_data_directory(value.into()); }
        }
        self.refresh_account_storage_view(&account);
        self.refresh_account_history_view(&account, signed_in)?;
        self.refresh_recharge_view(ui)
    }

    pub(super) fn account_action(&mut self, action: &str, value: &str) -> Result<()> {
        if self.ui.upgrade().is_some_and(|ui| !ui.global::<AccountModel>().get_local_dialog().is_empty())
            || (self.account_ui.recharge.visible && !action.starts_with("recharge-"))
        { return Ok(()); }
        match action {
            "section" => {
                if ["home", "account", "reading", "display", "download", "storage", "system", "about"].contains(&value) {
                    self.account_ui.section = value.into();
                    self.account_ui.message.clear();
                }
            }
            "back" => {
                self.account_ui.section = if matches!(self.account_ui.section.as_str(), "purchases" | "recharges") { "account" } else { "home" }.into();
                self.account_ui.message.clear();
            }
            "login" => { self.auth_return = "account".into(); self.navigate("login"); }
            "logout" => self.logout()?,
            "refresh-wallet" => { if self.require_session() && self.require_network("account", action, value) { self.refresh_wallet(true); } }
            "recharge" => self.open_recharge(),
            "purchase-history" => self.open_purchase_history(),
            "recharge-history" => self.open_recharge_history(),
            "import-session" => self.import_account_session(value),
            "mode" if ["page", "panel", "strip"].contains(&value) => self.preferences.mode = value.into(),
            "fit" if FitMode::parse(value).is_some() => self.preferences.fit = value.into(),
            "direction" if ["ltr", "rtl"].contains(&value) => self.preferences.direction = value.into(),
            "every" if self.preferences.refresh_mode != "fast" && ["smart", "0", "1", "6", "12"].contains(&value) => {
                self.preferences.every = value.into();
                self.platform.set_reader_refresh_interval(match value { "0" => 0, "1" => 1, "12" => 12, _ => 6 })?;
            }
            "refresh-mode" if ["normal", "fast"].contains(&value) => {
                self.preferences.refresh_mode = value.into();
                self.reader_refresh_mode_changed()?;
            }
            "dither-method" if self.preferences.refresh_mode != "fast" && ["off", "bayer8", "floyd_steinberg"].contains(&value) => {
                if value != "off" { self.preferences.dither_method = value.into(); }
                else { self.preferences.dither_method = normalized_dither_method(&self.preferences.dither_method).into(); }
                self.preferences.dither = value != "off";
                self.reader_dither_preferences_changed()?;
            }
            "landscape" if ["spread", "single"].contains(&value) => self.preferences.landscape = value.into(),
            "jpeg-acceleration" if ["on", "off"].contains(&value) => {
                if value == "off" || crate::native_media::availability().available() {
                    self.preferences.jpeg_acceleration = value == "on";
                    self.reader_jpeg_acceleration_changed()?;
                }
            }
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
            "sleep-cover" if (self.demo.is_some() || self.platform.is_kindle) && ["cover", "system"].contains(&value) => self.preferences.sleep_cover = value == "cover",
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
            "recharge-close" if !self.account_ui.recharge.busy => self.account_ui.recharge.visible = false,
            "recharge-option" => {
                if self.account_ui.recharge.visible && !self.account_ui.recharge.busy && self.account_ui.recharge.phase == "select" {
                    if let Ok(index) = value.parse::<usize>() {
                        if self.account_ui.recharge.config.as_ref().is_some_and(|config| index < config.options.len()) {
                            self.account_ui.recharge.selected = index;
                        }
                    }
                }
            }
            "recharge-create" if self.account_ui.recharge.visible => self.create_recharge_order()?,
            "recharge-check" if self.account_ui.recharge.visible => self.check_recharge(),
            "recharge-config" if self.account_ui.recharge.visible => self.load_recharge_config(),
            _ => {}
        }
        self.save_preferences()?;
        Ok(())
    }

    fn account_section(&self) -> &str {
        if self.is_scribe() && self.account_ui.section == "home" { "reading" } else { &self.account_ui.section }
    }

    pub(super) fn dismiss_account_overlays(&mut self) {
        self.account_ui.recharge.visible = false;
        self.account_ui.wallet_refreshed = false;
        if let Some(ui) = self.ui.upgrade() {
            ui.global::<AccountModel>().set_local_dialog("".into());
        }
    }

    pub(super) fn reset_account_presentation(ui: &AppWindow) {
        let account = ui.global::<AccountModel>();
        account.set_section("home".into());
        account.set_local_dialog("".into());
        account.set_wallet_refreshed(false);
        account.set_last_recharge("".into());
        account.set_purchase_rows(model(Vec::new()));
        account.set_recharge_history_status("".into());
        account.set_recharge_coins("".into());
        account.set_cache_action("".into());
        account.set_cache_result("".into());
        account.set_diagnostics_result("".into());
        account.set_session_imported(false);
    }

    fn refresh_wallet(&mut self, explicit: bool) {
        self.account_ui.wallet_attempted = true;
        if self.account_ui.busy { return; }
        self.account_ui.wallet_refreshed = false;
        if self.demo.is_some() {
            self.account_ui.wallet_refreshed = explicit;
            self.account_ui.message.clear();
            return;
        }
        self.account_ui.busy = true;
        let client = self.client.clone();
        self.source_job("account-wallet", async move { client.wallet().await }, move |app, result| {
            app.account_ui.busy = false;
            match result {
                Ok(wallet) => {
                    let snapshot = WalletSnapshot { value: wallet.clone(), observed_at: now() };
                    match app.store.set_preference(&app.account, "wallet", &snapshot) {
                        Ok(()) => {
                            app.account_ui.wallet = Some(wallet);
                            app.account_ui.wallet_at = snapshot.observed_at;
                            app.account_ui.wallet_refreshed = explicit && app.route == "account";
                        }
                        Err(_) => app.toast("common_storage_error"),
                    }
                }
                Err(error) => app.source_failure(&error),
            }
        });
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
                Ok(session) => {
                    let on_account_category = app.route == "account" && app.account_ui.section == "account";
                    match app.switch_session(session) {
                        Ok(()) => {
                            app.account_ui.session_imported = true;
                            if on_account_category { app.account_ui.section = "account".into(); }
                            app.toast("acct_import_complete");
                        }
                        Err(_) => app.toast("common_storage_error"),
                    }
                }
                Err(error) => app.source_failure(&error),
            }
        });
    }

    fn storage_snapshot(&self) -> Option<StorageSummary> {
        if let Some(demo) = &self.demo {
            let manual_bytes = self.demo_manual_download_usage()?;
            let cache = demo.fixture["download_summary"]["cache_mb"].as_u64().unwrap_or_default().saturating_mul(1024 * 1024);
            let free_key = if self.is_scribe() { "free_space_sc" } else { "free_space_pw" };
            let base_free = demo.fixture["account"]["presentation"][free_key].as_str()
                .and_then(|label| label.strip_suffix(" GB")).and_then(|value| value.parse::<f64>().ok());
            let freed_cache = if self.account_ui.cache_cleared { cache } else { 0 };
            let free_bytes = base_free.map(|value| ((value * 1_073_741_824.0).round() as u64)
                .saturating_add(freed_cache).saturating_add(self.demo_removed_download_bytes()));
            return Some(StorageSummary { manual_bytes, cache_bytes: cache - freed_cache, free_bytes });
        }
        self.account_ui.storage.clone()
    }

    fn storage_size_label(&self, bytes: u64) -> String {
        if self.demo.is_some() { super::download_ui::format_design_bytes(bytes) }
        else { super::download_ui::format_bytes(bytes) }
    }

    fn storage_labels(&self, storage: Option<&StorageSummary>) -> (String, String) {
        let unknown = || i18n::text("common_unknown");
        let manual = storage.map(|summary| self.storage_size_label(summary.manual_bytes)).unwrap_or_else(unknown);
        let free = storage.and_then(|summary| summary.free_bytes).map(|bytes| {
            if self.demo.is_some() { format!("{:.1} GB", bytes as f64 / 1_073_741_824.0) }
            else { self.storage_size_label(bytes) }
        }).unwrap_or_else(unknown);
        (manual, free)
    }

    pub(super) fn download_storage_labels(&self) -> (String, String) {
        self.storage_labels(self.storage_snapshot().as_ref())
    }

    fn refresh_account_storage_view(&self, account: &AccountModel<'_>) {
        let storage = self.storage_snapshot();
        let (manual, free) = self.storage_labels(storage.as_ref());
        let (manual_fill, cache_fill) = storage.as_ref().map(StorageSummary::composition).unwrap_or_default();
        account.set_manual_used(manual.into());
        account.set_storage_free(free.into());
        account.set_storage_manual_fill(manual_fill);
        account.set_storage_cache_fill(cache_fill);
        if let Some(storage) = storage {
            let cache = self.storage_size_label(storage.cache_bytes);
            account.set_storage_used(self.storage_size_label(storage.used_bytes()).into());
            account.set_cache_used(cache.clone().into());
            if storage.cache_bytes == 0 {
                account.set_cache_action("".into());
                account.set_cache_result(if self.account_ui.cache_cleared { tr("acct_cleared") } else { cache.into() });
            } else if self.account_ui.storage_pending && self.account_ui.cache_cleared {
                account.set_cache_action("".into());
                account.set_cache_result(tr("acct_cleared"));
            } else {
                account.set_cache_action(i18n::format("acct_clear_size", &[("size", cache)]).into());
                account.set_cache_result("".into());
            }
        } else {
            account.set_storage_used(tr("common_unknown"));
            account.set_cache_used(tr("common_unknown"));
            account.set_cache_action("".into());
            account.set_cache_result(tr("common_unknown"));
        }
    }

    pub(super) fn schedule_storage_summary(&mut self) {
        if self.demo.is_some() || self.is_suspended() || self.account_ui.storage_pending
            || self.account_ui.storage_checked.is_some_and(|value| value.elapsed() <= Duration::from_secs(30))
        { return; }
        self.account_ui.storage_pending = true;
        let generation = self.account_ui.storage_generation;
        let database = self.store.path().to_owned(); let data = self.data_dir.clone(); let account = self.account.clone();
        self.background("account-storage", move || storage_summary(&database, &data, &account), move |app, result| {
            app.account_ui.storage_pending = false;
            if app.account_ui.storage_generation != generation {
                app.schedule_storage_summary();
                return;
            }
            app.account_ui.storage_checked = Some(Instant::now());
            match result { Ok(summary) => app.account_ui.storage = Some(summary), Err(_) => app.toast("common_storage_error") }
            app.view_dirty.mark(View::Account);
            app.view_dirty.mark(View::Downloads);
        });
    }

    pub(super) fn invalidate_account_storage(&mut self) {
        self.account_ui.storage_generation = self.account_ui.storage_generation.wrapping_add(1);
        self.account_ui.storage_checked = None;
        self.view_dirty.mark(View::Account);
        self.view_dirty.mark(View::Downloads);
        self.schedule_storage_summary();
    }

    pub(super) fn enforce_cache_limit(&mut self) -> Result<()> {
        if self.evict_account_cache(Some(u64::from(self.preferences.cache_limit) * 1024 * 1024))? > 0 {
            self.invalidate_account_storage();
        }
        Ok(())
    }

    fn clear_account_cache(&mut self) -> Result<()> {
        if self.demo.is_some() {
            self.account_ui.cache_cleared = true;
            self.invalidate_account_storage();
            return Ok(());
        }
        if self.downloads.is_none() {
            self.toast("common_background_busy");
            return Ok(());
        }
        self.account_ui.cache_cleared = self.evict_account_cache(None)? > 0;
        if !self.account_ui.cache_cleared { self.toast("acct_nothing_to_clear"); }
        self.invalidate_account_storage();
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
        self.account_ui.diagnostics_exported = true;
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
        if let Some(comic) = self.account_ui.comic.clone() {
            purchase.set_comic_title(comic.title.clone().into());
            purchase.set_cover(self.cover_image(&comic));
        }
        purchase.set_back_label(self.purchase_back_label().into());
        let state = &self.account_ui;
        let current = self.current_purchase_intent();
        let preview = current.is_some_and(|intent| intent.state == "preview");
        let confirmed = current.is_some_and(|intent| intent.state == "confirmed" && !intent.persistence_pending);
        let phase = if state.phase == "done" && !confirmed && !preview { "pending" } else { state.phase.as_str() };
        let showing_result = matches!(phase, "pending" | "done");
        purchase.set_phase(phase.into());
        purchase.set_selected_range(state.selection.clone().into());
        purchase.set_payment(state.payment.clone().into());
        purchase.set_quote_loading(state.quote_loading);
        purchase.set_reconciling(state.reconciling);
        purchase.set_error_text(state.quote_error.clone().into());

        let quote = purchase_selected_offer(state);
        let read = state.quote_read.as_ref();
        // Recovery can reopen a journal without a fresh quote read. Resolve its
        // chapter IDs against the cached catalog, not a newly proposed scope.
        let cached = if read.is_none() && showing_result {
            current.and_then(|intent| self.store.load_detail(&self.account, &intent.quote.comic_id).ok().flatten())
        } else { None };
        let detail = read.map(|read| &read.detail).or(cached.as_ref());
        let anchor = state.episode.as_ref().map(|episode| {
            detail.and_then(|detail| detail.episodes.iter().find(|item| item.id == episode.id)).unwrap_or(episode)
        });
        purchase.set_start_label(anchor.map(|episode| {
            let key = if episode.title.is_empty() { "buy_start_without_title" } else { "buy_start" };
            i18n::format(key, &[("number", display_order(episode.order)), ("title", episode.title.clone())])
        }).unwrap_or_default().into());
        let presented_quote = if showing_result { current.map(|intent| &intent.quote) } else { quote };
        let members = presented_quote.map(|quote| purchase_members(quote, detail, anchor)).unwrap_or_default();
        let scope = presented_quote.map(|quote| purchase_scope_label(quote, &members, anchor)).unwrap_or_default();
        purchase.set_ranges(model(if showing_result { Vec::new() } else {
            purchase_range_items(read, anchor, quote.map(|quote| (quote.selection.as_str(), scope.as_str())))
        }));
        let timestamp = now();
        let included: Vec<_> = members.iter().map(|episode| PurchaseIncludedItem {
            number: display_order(episode.order).into(),
            title: episode.title.clone().into(),
            note: if episode.access == EpisodeAccess::Temporary && episode.readable_at(timestamp) {
                tr(if self.demo.is_some() || value_true(episode.extra.get("is_in_free")) { "buy_included_temporary" } else { "cat_temporary" })
            } else { "".into() },
        }).collect();
        let complete_scope = presented_quote.is_some_and(|quote| included.len() == quote.expected_ids.len());
        purchase.set_included_label(if complete_scope {
            i18n::format("buy_included", &[("count", included.len().to_string())]).into()
        } else { "".into() });
        purchase.set_included(model(if complete_scope { included } else { Vec::new() }));

        let wallet = state.wallet.as_ref();
        purchase.set_wallet_label(wallet.and_then(|value| amount(value.get("remain_gold")))
            .map(|value| i18n::format("acct_wallet_amount", &[("amount", display_amount(value))]))
            .unwrap_or_else(|| i18n::text("buy_wallet_unknown")).into());
        let (can_select_coupon, coupon_label) = purchase_coupon_choice(state);
        purchase.set_coupon_label(coupon_label.into());
        purchase.set_coupons_allowed(can_select_coupon);
        let fresh = quote.is_some_and(|quote| timestamp >= quote.observed_at && timestamp - quote.observed_at <= QUOTE_MAX_AGE);
        purchase.set_quote_ready(fresh && !state.quote_loading && !state.journals.iter().any(PurchaseIntent::unresolved)
            && matches!(phase, "review" | "error"));
        purchase.set_sufficient(quote.is_none_or(|quote| quote.balance >= quote.amount));
        purchase.set_total(quote.map(|quote| purchase_amount_label(quote.amount, &quote.payment))
            .unwrap_or_else(|| i18n::text("buy_unknown")).into());
        purchase.set_after_text(quote.map(|quote| {
            let key = match (quote.payment.as_str(), quote.balance < quote.amount) {
                ("coupon", true) => "buy_short_coupon",
                ("coupon", false) => "buy_after_coupon",
                (_, true) => "buy_short_coin",
                (_, false) => "buy_after_coin",
            };
            i18n::format(key, &[("amount", display_amount((quote.balance - quote.amount).abs()))])
        }).unwrap_or_default().into());
        purchase.set_pay_label(tr(if state.payment == "coupon" { "buy_use_coupon" } else { "buy_confirm" }));

        let result_scope = if presented_quote.is_some_and(|quote| quote.expected_ids.len() == 1) {
            members.first().filter(|episode| !episode.title.is_empty()).map(|episode| {
                i18n::format("buy_scope_with_title", &[("scope", scope.clone()), ("title", episode.title.clone())])
            }).unwrap_or_else(|| scope.clone())
        } else { scope.clone() };
        purchase.set_scope_label(result_scope.into());
        purchase.set_result_title(tr(if preview { "acct_preview_complete" } else { "buy_success_title" }));
        purchase.set_result_body(if preview { tr("acct_preview_no_transaction") } else { "".into() });
        purchase.set_result_can_read(confirmed);
        purchase.set_pending_summary(current.filter(|_| phase == "pending").map(|intent| {
            i18n::format("buy_pending_summary", &[
                ("comic", state.comic.as_ref().map(|comic| comic.title.clone()).unwrap_or_default()),
                ("scope", scope),
                ("total", purchase_amount_label(intent.quote.amount, &intent.quote.payment)),
            ])
        }).unwrap_or_default().into());
        purchase.set_pending_body(tr(match current {
            Some(intent) if intent.persistence_pending => "acct_receipt_storage_pending",
            Some(intent) if intent.entitlement_observed => "acct_access_observed_unknown_charge",
            Some(intent) if intent.state == "acknowledged" => "acct_accepted_waiting_entitlement",
            _ => "buy_pending_body",
        }));
        self.refresh_recharge_view(ui)
    }

    pub(super) fn purchase_action(&mut self, action: &str, value: &str) -> Result<()> {
        if self.account_ui.recharge.visible { return Ok(()); }
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
                if value == "coupon" && !purchase_coupon_choice(&self.account_ui).0 {
                    self.toast("acct_coupon_unverified");
                } else { self.account_ui.payment = value.into(); self.load_purchase_quote(); }
            }
            "refresh-wallet" | "refresh-quote" => self.load_purchase_quote(),
            "submit" => {
                if self.account_ui.quote.is_some() && purchase_selected_offer(&self.account_ui).is_none() {
                    self.toast("acct_quote_unverified");
                } else { self.submit_purchase()?; }
            }
            "reconcile" => self.reconcile_purchase(),
            "recharge" => self.open_recharge(),
            "detail" => {
                if self.purchase_origin == "reader" && self.reading.is_some() { self.go_back()?; }
                else if let Some(comic) = self.account_ui.comic.clone() { self.open_detail(&comic.id)?; }
            }
            "download" | "read" => {
                if let Some(intent) = self.current_purchase_intent().cloned().filter(|intent| intent.state == "confirmed" && !intent.persistence_pending) {
                    if let Some(detail) = self.store.load_detail(&self.account, &intent.quote.comic_id)? {
                        self.catalog.detail = Some(detail);
                    }
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
        if self.account_ui.recharge.busy { return; }
        if let Some(intent) = self.account_ui.recharge.journal.clone().filter(|intent|
            intent.persistence_pending || matches!(intent.state.as_str(), "creating" | "unknown" | "qr"))
        {
            self.account_ui.recharge.present_intent(&intent);
        } else {
            self.load_recharge_config();
        }
    }

    fn load_recharge_config(&mut self) {
        if self.account_ui.recharge.busy { return; }
        if self.account_ui.recharge.journal.as_ref().is_some_and(|intent| intent.persistence_pending || ["creating", "unknown", "qr"].contains(&intent.state.as_str())) { return; }
        if self.demo.is_some() {
            self.account_ui.recharge.phase = "unavailable".into();
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
                    app.account_ui.recharge.message = if config.options.is_empty() { i18n::text("acct_recharge_config_error") } else { String::new() };
                    app.account_ui.recharge.selected = 0;
                    app.account_ui.recharge.config = Some(config);
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
            amount_cents: option.amount_cents, coin_amount: Some(option.coin_amount),
            fingerprint: option.fingerprint, state: "creating".into(),
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
                Ok((intent, error, _persisted)) => {
                    app.account_ui.recharge.present_intent(&intent);
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
        let Some(mut intent) = self.account_ui.recharge.journal.clone() else { return; };
        if !self.require_session() || !self.require_network("account", "recharge-check", "") { return; }
        self.account_ui.recharge.busy = true;
        self.account_ui.recharge.message = i18n::text("acct_recharge_checking");
        let client = self.client.clone();
        self.source_job("recharge-check", async move {
            let history = client.recharge_history(1, 100).await?;
            let wallet = client.wallet().await?;
            Ok((history, wallet))
        }, move |app, result| {
            app.account_ui.recharge.busy = false;
            match result {
                Ok((history, wallet)) => {
                    let identity = intent.order.as_ref().map(|order| order.order_id.as_str()).or(intent.reconciliation_id.as_deref());
                    let observed = identity.is_some_and(|id| recharge_records(&history)
                        .iter().any(|record| record["order_id"].as_str() == Some(id)));
                    let snapshot = WalletSnapshot { value: wallet.clone(), observed_at: now() };
                    if app.store.set_preference(&app.account, "wallet", &snapshot).is_err() {
                        app.account_ui.recharge.message = i18n::text("common_storage_error");
                        app.toast("common_storage_error"); return;
                    }
                    app.account_ui.wallet = Some(wallet);
                    app.account_ui.wallet_at = snapshot.observed_at;
                    app.account_ui.wallet_refreshed = app.route == "account";
                    // An older history-only request must not overwrite this reconciliation read.
                    app.invalidate("recharge-history");
                    app.account_ui.recharge.history_loading = false;
                    app.account_ui.recharge.history = Some(history);
                    app.account_ui.recharge.history_failed = false;
                    if observed {
                        // An exact history observation is retained; a balance delta is never payment proof.
                        intent.state = "observed".into(); intent.updated_at = now();
                        match merge_recharge_intent(app.store.path(), &intent) {
                            Ok(saved) => app.account_ui.recharge.journal = Some(saved),
                            Err(_) => {
                                app.account_ui.recharge.message = i18n::text("common_storage_error");
                                app.toast("common_storage_error"); return;
                            }
                        }
                        if app.account_ui.recharge.visible {
                            app.account_ui.recharge.visible = false;
                            if app.route == "purchase" && matches!(app.account_ui.phase.as_str(), "review" | "error") {
                                app.load_purchase_quote();
                            }
                        }
                    } else {
                        app.account_ui.recharge.message = i18n::text("acct_recharge_not_found");
                    }
                }
                Err(error) => {
                    app.account_ui.recharge.message = i18n::text("acct_recharge_refresh_failed");
                    app.source_failure(&error);
                }
            }
        });
    }

    fn refresh_recharge_view(&self, ui: &AppWindow) -> Result<()> {
        let model_ui = ui.global::<AccountModel>();
        let recharge = &self.account_ui.recharge;
        model_ui.set_recharge_visible(recharge.visible);
        model_ui.set_recharge_phase(recharge.phase.clone().into());
        model_ui.set_recharge_index(recharge.selected as i32);
        model_ui.set_recharge_busy(recharge.busy);
        model_ui.set_recharge_message(recharge.message.clone().into());
        model_ui.set_recharge_qr(recharge.qr.clone());
        let options = recharge.config.as_ref().map(|config| config.options.as_slice()).unwrap_or_default();
        model_ui.set_recharge_options(model(options.iter().map(|option| AccountInfoItem {
            label: wallet_amount_label(option.coin_amount as f64, "acct_coin").into(),
            value: recharge_price_label(option.amount_cents).into(),
        }).collect()));
        let (total, coins) = if recharge.phase == "select" {
            options.get(recharge.selected).map(|option| (Some(option.amount_cents), Some(option.coin_amount))).unwrap_or_default()
        } else if let Some(intent) = &recharge.journal {
            (Some(intent.amount_cents), intent.coin_amount)
        } else { (None, None) };
        model_ui.set_recharge_total(total.map(recharge_price_label).unwrap_or_default().into());
        model_ui.set_recharge_coins(coins.map(|coins| wallet_amount_label(coins as f64, "acct_coin")).unwrap_or_default().into());
        model_ui.set_recharge_can_check(recharge.journal.is_some() && !recharge.busy);
        Ok(())
    }

    fn refresh_account_history_view(&self, account: &AccountModel<'_>, signed_in: bool) -> Result<()> {
        let journal = &self.account_ui.journals;
        let last_purchase = journal.iter().map(|intent| intent.created_at).max()
            .and_then(local_receipt_time).map(|time| {
                let today = local_receipt_time(now()).is_some_and(|now| now.date_naive() == time.date_naive());
                recent_label(&time.format(if today { "%H:%M" } else { "%m-%d" }).to_string())
            });
        let recharge = &self.account_ui.recharge;
        let official = recharge.history.as_ref().map(recharge_records).unwrap_or_default();
        let last_recharge = official.iter().filter_map(|record| record["created_at"].as_str()).max()
            .map(|time| recent_label(time.get(5..10).filter(|_| time.as_bytes().get(4) == Some(&b'-')).unwrap_or(time)));
        account.set_last_purchase(if signed_in { last_purchase.unwrap_or_else(|| i18n::text("acct_no_records")) } else { String::new() }.into());
        account.set_last_recharge(if signed_in { last_recharge.unwrap_or_default() } else { String::new() }.into());
        let mut purchase_rows = Vec::new();
        if signed_in && self.account_ui.section == "purchases" {
            for intent in journal.iter().rev() {
                let quote = &intent.quote;
                let detail = self.store.load_detail(&self.account, &quote.comic_id)?;
                let fallback = if detail.is_none() { self.store.comic(&self.account, &quote.comic_id)? } else { None };
                let title = detail.as_ref().map(|detail| detail.comic.title.as_str()).or_else(|| fallback.as_ref().map(|comic| comic.title.as_str()));
                let members = purchase_members(quote, detail.as_ref(), None);
                let scope = history_scope_label(quote, &members);
                purchase_rows.push(AccountRecordItem {
                    title: title.filter(|title| !title.is_empty()).map_or(scope.clone(), |title| format!("{title} · {scope}")).into(),
                    status: purchase_history_status(intent).into(),
                    amount: purchase_history_amount(intent).into(),
                    time: receipt_time(intent.created_at).into(),
                });
            }
        }
        account.set_purchase_rows(model(purchase_rows));
        if signed_in && self.account_ui.section == "recharges" {
            account.set_recharge_rows(model(official.iter().map(recharge_record_row).collect()));
            account.set_recharge_history_status(if recharge.history_failed { tr("acct_recharge_history_failed") }
                else if recharge.history_loading { tr("acct_loading_history") }
                else if official.is_empty() { tr("acct_no_records") } else { "".into() });
        } else {
            account.set_recharge_rows(model(Vec::new()));
            account.set_recharge_history_status("".into());
        }
        Ok(())
    }

    fn open_purchase_history(&mut self) {
        if !self.require_session() { return; }
        self.account_ui.section = "purchases".into();
        self.account_ui.message.clear();
    }

    fn open_recharge_history(&mut self) {
        if !self.require_session() { return; }
        if self.demo.is_none() && !self.require_network("account", "recharge-history", "") { return; }
        self.account_ui.section = "recharges".into();
        self.account_ui.message.clear();
        self.load_recharge_history();
    }

    fn load_recharge_history(&mut self) {
        if self.demo.is_some() || self.account_ui.recharge.history_loading { return; }
        self.account_ui.recharge.history_loading = true;
        self.account_ui.recharge.history_failed = false;
        let client = self.client.clone();
        self.source_job("recharge-history", async move { client.recharge_history(1, 100).await }, |app, result| {
            app.account_ui.recharge.history_loading = false;
            match result {
                Ok(history) => app.account_ui.recharge.history = Some(history),
                Err(error) => {
                    app.account_ui.recharge.history_failed = true;
                    if app.account_ui.section == "recharges" && app.route == "account" { app.source_failure(&error); }
                }
            }
        });
    }
}

fn preview_quote_read(demo: &DemoData, episode: &Episode, detail: ComicDetail) -> source::Result<QuoteRead> {
    preview_quote_read_fixture(&demo.fixture, episode, detail)
}

fn preview_quote_read_fixture(catalog: &Value, episode: &Episode, detail: ComicDetail) -> source::Result<QuoteRead> {
    let fixture = &catalog["purchase"];
    if fixture["demo_only"] != true { return Err(SourceError::capability("preview_quote_fixture_missing")); }
    let matches_episode = |key| fixture[key].as_u64().map(|value| value.to_string()).as_deref() == Some(episode.id.as_str());
    let ranges = if matches_episode("selected_episode_id") { fixture["ranges"].clone() }
    else if matches_episode("late_episode_id") { fixture["late_ranges"].clone() }
    else {
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
                && stored.coin_amount == incoming.coin_amount
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

fn recharge_price_label(cents: u64) -> String {
    if cents % 100 == 0 { format!("¥{}", cents / 100) }
    else { format!("¥{}.{:02}", cents / 100, cents % 100) }
}

fn recharge_records(history: &Value) -> &[Value] {
    history["records"].as_array().map(Vec::as_slice).unwrap_or_default()
}

fn recharge_record_row(record: &Value) -> AccountRecordItem {
    let coin_amount = amount(record.get("product_amount"));
    let channel = record["pay_channel_name"].as_str().or(record["pay_channel"].as_str());
    AccountRecordItem {
        title: coin_amount.map(|coins| wallet_amount_label(coins, "acct_coin")).unwrap_or_else(|| i18n::text("common_unknown")).into(),
        status: channel.map(str::to_owned).unwrap_or_else(|| i18n::text("common_unknown")).into(),
        amount: "".into(),
        time: record["created_at"].as_str().map(str::to_owned).unwrap_or_else(|| i18n::text("common_unknown")).into(),
    }
}

fn wallet_amount_label(amount: f64, unit_key: &str) -> String {
    format!("{} {}", display_amount(amount), i18n::text(unit_key))
}

fn history_scope_label(quote: &OfferSnapshot, members: &[&Episode]) -> String {
    let consecutive = members.windows(2).all(|pair| pair[1].order - pair[0].order == 1.0);
    if consecutive && !members.is_empty() && members.len() == quote.expected_ids.len() {
        purchase_scope_label(quote, members, None)
    } else {
        i18n::format("acct_offer_count", &[("count", quote.expected_ids.len().to_string())])
    }
}

fn purchase_history_status(intent: &PurchaseIntent) -> String {
    i18n::text(if intent.persistence_pending { "acct_record_unknown" } else { match intent.state.as_str() {
        "confirmed" => "acct_access_confirmed", "acknowledged" => "acct_access_pending", "rejected" => "acct_denied",
        "preview" => "acct_record_preview", _ => "acct_record_unknown",
    } })
}

fn purchase_history_amount(intent: &PurchaseIntent) -> String {
    if intent.persistence_pending { return i18n::text("common_unknown"); }
    match intent.state.as_str() {
        "confirmed" => purchase_amount_label(intent.quote.amount, &intent.quote.payment),
        "rejected" | "preview" => i18n::text("acct_no_charge"),
        _ => i18n::text("common_unknown"),
    }
}

fn local_receipt_time(timestamp: i64) -> Option<chrono::DateTime<chrono::FixedOffset>> {
    if timestamp <= 0 { return None; }
    let offset = chrono::FixedOffset::east_opt(8 * 3600)?;
    chrono::DateTime::from_timestamp(timestamp, 0).map(|time| time.with_timezone(&offset))
}

fn receipt_time(timestamp: i64) -> String {
    local_receipt_time(timestamp).map(|time| time.format("%m-%d %H:%M").to_string())
        .unwrap_or_else(|| i18n::text("common_unknown"))
}

fn recent_label(time: &str) -> String { i18n::format("acct_recent", &[("time", time.to_owned())]) }

fn purchase_amount_label(amount: f64, payment: &str) -> String {
    i18n::format(if payment == "coupon" { "buy_amount_coupon" } else { "buy_amount_coin" },
        &[("amount", display_amount(amount))])
}

fn purchase_selected_offer(state: &AccountState) -> Option<&OfferSnapshot> {
    state.quote.as_ref().filter(|quote| quote.selection == state.selection && quote.payment == state.payment
        && state.episode.as_ref().is_some_and(|episode| quote.episode_id == episode.id))
}

fn purchase_coupon_choice(state: &AccountState) -> (bool, String) {
    if state.selection != "single" { return (false, i18n::text("buy_coupon_single_only")); }
    let read = state.quote_read.as_ref();
    let available = state.wallet.as_ref().and_then(|wallet| amount(wallet.get("remain_coupon")))
        .or_else(|| read.and_then(|read| amount(read.basic.get("remain_coupon"))));
    match available {
        Some(available) if available == 0.0 => (false, i18n::text("buy_coupon_empty")),
        Some(available) if read.is_some_and(|read| value_true(read.basic.get("allow_coupon"))) => {
            (true, i18n::format("acct_coupon_amount", &[("amount", display_amount(available))]))
        }
        Some(_) if read.is_some() => (false, i18n::text("buy_coupon_unavailable")),
        _ => (false, i18n::text("buy_coupon_unknown")),
    }
}

fn order_scope_label(first: f64, last: Option<f64>, count: usize) -> String {
    if count == 1 { return i18n::format("acct_scope_single", &[("number", display_order(first))]); }
    match last {
        Some(last) => i18n::format("acct_scope_range", &[("first", display_order(first)), ("last", display_order(last))]),
        None => i18n::format("acct_scope_from", &[("first", display_order(first)), ("count", count.to_string())]),
    }
}

fn purchase_members<'a>(quote: &OfferSnapshot, detail: Option<&'a ComicDetail>, anchor: Option<&'a Episode>) -> Vec<&'a Episode> {
    let ids: BTreeSet<_> = quote.expected_ids.iter().map(String::as_str).collect();
    let mut members: Vec<_> = detail.into_iter().flat_map(|detail| &detail.episodes)
        .filter(|episode| ids.contains(episode.id.as_str())).collect();
    if let Some(anchor) = anchor.filter(|anchor| ids.contains(anchor.id.as_str()) && !members.iter().any(|member| member.id == anchor.id)) {
        members.push(anchor);
    }
    members.sort_by(|left, right| left.order.total_cmp(&right.order));
    members
}

fn purchase_scope_label(quote: &OfferSnapshot, members: &[&Episode], anchor: Option<&Episode>) -> String {
    let count = quote.expected_ids.len();
    if let Some(first) = members.first().filter(|_| members.len() == count) {
        return order_scope_label(first.order, members.last().map(|last| last.order), count);
    }
    match &quote.request.scope {
        PurchaseScope::OrdinalRange { start_order, .. } => order_scope_label(*start_order, None, count),
        PurchaseScope::Single { .. } => anchor.filter(|episode| episode.id == quote.episode_id)
            .map(|episode| order_scope_label(episode.order, None, 1))
            .unwrap_or_else(|| i18n::format("acct_offer_count", &[("count", count.to_string())])),
    }
}

fn purchase_range_items(read: Option<&QuoteRead>, anchor: Option<&Episode>, selected_scope: Option<(&str, &str)>) -> Vec<PurchaseRangeItem> {
    let mut remaining: Vec<_> = read.zip(anchor).into_iter().flat_map(|(read, anchor)| {
        read.detail.episodes.iter().filter(move |episode| episode.order >= anchor.order)
    }).collect();
    remaining.sort_by(|left, right| left.order.total_cmp(&right.order));
    let locked: Vec<_> = remaining.iter().copied().filter(|episode| episode.access == EpisodeAccess::Locked).collect();
    let complete_catalog = read.is_some_and(|read| read.detail.comic.chapter_count > 0
        && read.detail.episodes.len() >= read.detail.comic.chapter_count as usize);
    let too_few = |count: u32| complete_catalog && remaining.len() < count as usize;
    let count_label = |count: u32| i18n::format("acct_offer_count", &[("count", count.to_string())]);
    let unavailable = |count: u32| tr(if too_few(count) { "buy_too_few_chapters" } else { "buy_range_unavailable" });
    let mut ranges = Vec::new();
    if let Some(read) = read {
        for (id, offered_count, price, original, usable) in quote_selections(read) {
            let count = if usable { offered_count } else {
                id.strip_prefix("batch:").and_then(|index| index.parse::<usize>().ok())
                    .and_then(|index| index.checked_sub(1)).and_then(|index| read.basic["batch_buy"].get(index))
                    .and_then(|offer| quote_u32(offer.get("batch_limit"), 1)).unwrap_or(offered_count)
            };
            if count == 0 { continue; }
            let enabled = usable && !too_few(count);
            let scope = if !enabled { unavailable(count) } else if let Some((_, scope)) = selected_scope.filter(|(selected, _)| *selected == id) {
                scope.into()
            } else if let Some(anchor) = anchor {
                let last = read.basic["preview_ranges"].as_array().and_then(|ranges| ranges.iter()
                    .find(|range| range["key"].as_str() == Some(id.as_str()) && amount(range.get("start_ordinal")) == Some(anchor.order)))
                    .and_then(|range| amount(range.get("end_ordinal")))
                    .filter(|last| remaining.iter().any(|episode| episode.order == *last))
                    .or_else(|| locked.get(count as usize - 1).map(|episode| episode.order));
                order_scope_label(anchor.order, last, count as usize).into()
            } else { count_label(count).into() };
            ranges.push((count, PurchaseRangeItem {
                id: id.into(), label: count_label(count).into(), scope,
                price: if enabled { purchase_amount_label(price, "coin").into() } else { "".into() },
                original: if enabled && original != price { purchase_amount_label(original, "coin").into() } else { "".into() },
                enabled,
            }));
        }
    }
    // The design reserves the standard three tiers. A missing tier is only a
    // disabled presentation slot; it is not an offer and has no action ID.
    for count in [1, 5, 10] {
        if !ranges.iter().any(|(tier, _)| *tier == count) {
            ranges.push((count, PurchaseRangeItem { id: "".into(), label: count_label(count).into(),
                scope: unavailable(count), price: "".into(), original: "".into(), enabled: false }));
        }
    }
    ranges.sort_by_key(|(count, _)| *count);
    ranges.into_iter().map(|(_, item)| item).collect()
}

fn storage_summary(database: &Path, data: &Path, account: &str) -> Result<StorageSummary> {
    let mut summary = account_page_usage(&Store::open(database)?, account)?;
    summary.free_bytes = super::available_bytes(data);
    Ok(summary)
}

fn unique_cache_bytes(store: &Store, account: &str) -> Result<u64> {
    Ok(account_page_usage(store, account)?.cache_bytes)
}

fn account_page_usage(store: &Store, account: &str) -> Result<StorageSummary> {
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
    let mut summary = StorageSummary::default();
    for (bytes, retained) in objects.into_values() {
        // A deduplicated object shared with a retained episode belongs to manual downloads.
        if retained { summary.manual_bytes = summary.manual_bytes.saturating_add(bytes); }
        else { summary.cache_bytes = summary.cache_bytes.saturating_add(bytes); }
    }
    Ok(summary)
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

    #[test]
    fn storage_composition_tracks_reader_page_usage_instead_of_volume_space() {
        let mut storage = StorageSummary { manual_bytes: 750, cache_bytes: 250, free_bytes: Some(99_000) };
        assert_eq!(storage.used_bytes(), 1_000);
        assert_eq!(storage.composition(), (0.75, 0.25));
        storage.cache_bytes = 0;
        assert_eq!(storage.composition(), (1.0, 0.0));
        assert_eq!(StorageSummary::default().composition(), (0.0, 0.0));
    }

    #[test]
    fn account_defaults_persist_fit_and_keep_normal_preferences_paused_in_fast_mode() -> Result<()> {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
        let directory = tempfile::tempdir()?;
        let client = BiliClient::offline(directory.path())?;
        let mut app = super::super::test_support::app(directory.path(), DeviceProfile::Paperwhite6,
            client, runtime.handle().clone())?;
        app.account_action("fit", "height")?;
        app.account_action("fit", "invalid")?;
        app.preferences.refresh_mode = "fast".into();
        app.account_action("every", "1")?;
        app.account_action("dither-method", "off")?;
        let saved: Preferences = app.store.preference(&app.account, "preferences")?.context("Read saved account defaults")?;
        assert_eq!(saved.fit, "height");
        assert_eq!(saved.every, Preferences::default().every);
        assert!(saved.dither);
        Ok(())
    }

    #[test]
    fn demo_account_attachment_preserves_saved_preferences_and_seeds_only_a_fresh_store() -> Result<()> {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
        let directory = tempfile::tempdir()?;
        let demo_root = directory.path().join("demo");
        std::fs::create_dir_all(&demo_root)?;
        let demo = Arc::new(crate::demo::load(&demo_root)?);
        let fresh_interval = demo.fixture["account"]["defaults"]["refresh"].as_str()
            .context("The fictional account must declare its initial refresh policy")?.to_owned();
        for saved in [false, true] {
            let data = directory.path().join(if saved { "saved" } else { "fresh" });
            std::fs::create_dir_all(&data)?;
            let client = BiliClient::offline(&data)?;
            let mut app = super::super::test_support::app(&data, DeviceProfile::Paperwhite6,
                client, runtime.handle().clone())?;
            app.account = "design-demo".into();
            app.options.demo = true;
            app.options.offline = true;
            app.demo = Some(demo.clone());
            let expected = Preferences { every: "6".into(), refresh_mode: "fast".into(),
                dither: false, dither_method: "floyd_steinberg".into(), ..Default::default() };
            if saved {
                app.store.set_preference(&app.account, "preferences", &expected)?;
                app.preferences = app.store.preference(&app.account, "preferences")?
                    .context("The saved demonstration preferences must reload")?;
            } else {
                app.preferences.every = "12".into();
            }
            let prepared = PreparedAccount::load(&app.store, &app.account, app.demo.as_deref())?;
            app.initialize_account_prepared(prepared)?;
            if saved {
                assert_eq!(app.preferences.every, expected.every);
                assert_eq!(app.preferences.refresh_mode, expected.refresh_mode);
                assert_eq!(app.preferences.dither, expected.dither);
                assert_eq!(app.preferences.dither_method, expected.dither_method);
                let durable: Preferences = app.store.preference(&app.account, "preferences")?
                    .context("Account attachment must preserve the durable preference row")?;
                assert_eq!(durable.every, expected.every);
                assert_eq!(durable.refresh_mode, expected.refresh_mode);
                assert_eq!(durable.dither, expected.dither);
                assert_eq!(durable.dither_method, expected.dither_method);
            } else {
                assert_eq!(app.preferences.every, fresh_interval);
                assert_eq!(app.preferences.refresh_mode, "normal");
            }
        }
        Ok(())
    }

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
    fn purchase_history_does_not_present_a_charge_for_uncertain_delivery() {
        let mut current = intent("history-charge");
        let unknown = i18n::text("common_unknown");
        for state in ["submitting", "unknown", "acknowledged"] {
            current.state = state.into();
            assert_eq!(purchase_history_amount(&current), unknown);
        }
        current.state = "confirmed".into();
        assert_eq!(purchase_history_amount(&current), purchase_amount_label(current.quote.amount, "coin"));
        current.persistence_pending = true;
        assert_eq!(purchase_history_amount(&current), unknown);
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
            coin_amount: Some(50), fingerprint: "a".repeat(64), state: "creating".into(), created_at: 1, updated_at: 1,
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
        assert_eq!(saved.coin_amount, Some(50));
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
            coin_amount: Some(50), fingerprint: "f".repeat(64), state: "creating".into(), created_at: 1, updated_at: 1,
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
