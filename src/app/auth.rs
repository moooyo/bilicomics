use super::*;
use crate::{downloads::DownloadManager, source::{QrLoginState, RenewalInfo}};

impl App {
    pub(super) fn start_login(&mut self) {
        self.client.cancel_login();
        self.qr_key = None;
        self.qr_polling = false;
        self.invalidate("qr-generate");
        self.invalidate("qr-poll");
        self.invalidate("network-check");
        let Some(ui) = self.ui.upgrade() else { return; };
        let auth = ui.global::<AuthModel>();
        let session = self.client.session();
        let verified = session.as_ref().and_then(AuthSession::account_key).is_some();
        auth.set_direction(self.preferences.direction.clone().into());
        auth.set_mode(self.preferences.mode.clone().into());
        auth.set_landscape(self.preferences.landscape.clone().into());
        auth.set_authenticated(verified);
        auth.set_account_name(if verified {
            session.as_ref().map(|session| session.account_name().to_owned()).unwrap_or_default().into()
        } else { "".into() });
        auth.set_status_message("".into());
        auth.set_net_phase("idle".into());
        auth.set_net_message("".into());
        auth.set_net_visible(false);
        if self.route == "welcome" { auth.set_step(1); }
        if self.options.offline {
            auth.set_phase("offline".into());
            auth.set_qr(slint::Image::default());
            return;
        }
        if self.demo.is_some() {
            auth.set_phase("waiting".into());
            let fixture = self.data_dir.join("fixtures/qr-login.png");
            auth.set_qr(if fixture.is_file() { load_image(&fixture) } else { qr_image("BiliComics design preview - not a login link").unwrap_or_default() });
            auth.set_expires_label(tr("auth_fixture_expiry"));
            return;
        }
        auth.set_phase("loading".into());
        auth.set_expires_label(tr("auth_service_expiry"));
        let client = self.client.clone();
        self.source_job("qr-generate", async move { client.begin_qr().await }, |app, result| {
            let Some(ui) = app.ui.upgrade() else { return; };
            match result {
                Ok(challenge) => {
                    match qr_image(&challenge.url) {
                        Ok(image) => {
                            ui.global::<AuthModel>().set_qr(image);
                            ui.global::<AuthModel>().set_phase("waiting".into());
                            app.qr_key = Some(challenge.key);
                            app.qr_next_poll = Instant::now() + Duration::from_secs(2);
                        }
                        Err(_) => {
                            ui.global::<AuthModel>().set_phase("error".into());
                            app.toast("common_error");
                        }
                    }
                }
                Err(error) => {
                    ui.global::<AuthModel>().set_phase(if matches!(error.kind, ErrorKind::Network | ErrorKind::Http) { "offline" } else { "error" }.into());
                    app.source_failure(&error);
                }
            }
        });
    }

    pub(super) fn tick_auth(&mut self) -> Result<()> {
        if !["welcome", "login"].contains(&self.route.as_str()) { return Ok(()); }
        let Some(ui) = self.ui.upgrade() else { return Ok(()); };
        let phase = ui.global::<AuthModel>().get_phase();
        if phase == "success" && Instant::now() >= self.qr_next_poll {
            if self.route == "welcome" {
                ui.global::<AuthModel>().set_step(2);
                ui.global::<AuthModel>().set_phase("waiting".into());
            } else {
                self.finish_auth_return();
            }
        }
        if self.demo.is_some() || self.options.offline || self.qr_polling
            || !["waiting", "scanned"].contains(&phase.as_str()) || Instant::now() < self.qr_next_poll
        { return Ok(()); }
        let Some(key) = self.qr_key.clone() else { return Ok(()); };
        self.qr_polling = true;
        self.qr_next_poll = Instant::now() + Duration::from_secs(2);
        let client = self.client.clone();
        self.source_job("qr-poll", async move { client.poll_qr(&key).await }, |app, result| {
            app.qr_polling = false;
            let Some(ui) = app.ui.upgrade() else { return; };
            match result {
                Ok(QrLoginState::Waiting) => ui.global::<AuthModel>().set_phase("waiting".into()),
                Ok(QrLoginState::Scanned) => ui.global::<AuthModel>().set_phase("scanned".into()),
                Ok(QrLoginState::Expired) => {
                    app.qr_key = None;
                    ui.global::<AuthModel>().set_phase("expired".into());
                }
                Ok(QrLoginState::Confirmed(session)) => {
                    match app.switch_session(session) {
                        Ok(()) => {
                            ui.global::<AuthModel>().set_phase("success".into());
                            ui.global::<AuthModel>().set_authenticated(true);
                            ui.global::<AuthModel>().set_account_name(app.client.session().map(|session| session.account_name().to_owned()).unwrap_or_default().into());
                            app.qr_key = None;
                            app.qr_next_poll = Instant::now() + Duration::from_millis(1500);
                        }
                        Err(_) => {
                            ui.global::<AuthModel>().set_phase("error".into());
                            app.toast("common_storage_error");
                        }
                    }
                }
                Err(error) => {
                    if error.kind != ErrorKind::Cancelled {
                        ui.global::<AuthModel>().set_phase(if matches!(error.kind, ErrorKind::Network | ErrorKind::Http) { "offline" } else { "error" }.into());
                        app.source_failure(&error);
                    }
                }
            }
        });
        Ok(())
    }

    fn finish_auth_return(&mut self) {
        let target = self.auth_return.clone();
        if target == "detail" {
            match self.finish_catalog_auth_return() {
                Ok(true) => return,
                Ok(false) => {}
                Err(_) => {
                    self.toast("common_storage_error");
                    self.navigate(if self.catalog.detail.is_some() { "detail" } else { "shelf" });
                    return;
                }
            }
            if self.catalog.detail.is_none() {
                self.navigate("shelf");
                return;
            }
        }
        self.navigate(&target);
    }

    pub(super) fn network_check_approved(&self, phase: &str) -> bool {
        self.demo.is_none() && phase == "connected" && self.network_verified_epoch == Some(self.epoch)
    }

    fn check_auth_network(&mut self, qr_source: bool) {
        self.invalidate("network-check");
        let Some(ui) = self.ui.upgrade() else { return; };
        let auth = ui.global::<AuthModel>();
        if qr_source {
            self.client.cancel_login();
            self.qr_key = None;
            self.qr_polling = false;
            self.invalidate("qr-generate");
            self.invalidate("qr-poll");
            auth.set_net_visible(false);
            auth.set_phase("connecting".into());
            auth.set_status_message(tr("auth_network_check_note"));
        } else {
            auth.set_net_visible(true);
        }
        auth.set_net_phase("connecting".into());
        auth.set_net_message(tr("auth_network_check_note"));
        if self.demo.is_some() {
            if qr_source {
                self.options.offline = false;
                self.online = true;
                self.start_login();
                auth.set_status_message(tr("auth_network_preview_note"));
                auth.set_expires_label(tr("auth_network_preview_title"));
                self.toast("auth_network_preview_note");
            } else {
                auth.set_net_phase("preview".into());
                auth.set_net_message(tr("auth_network_preview_note"));
            }
            return;
        }
        let client = self.client.clone();
        self.source_job("network-check", async move { client.categories().await }, move |app, result| {
            let Some(ui) = app.ui.upgrade() else { return; };
            let auth = ui.global::<AuthModel>();
            match result {
                Ok(_) => {
                    app.network_verified_epoch = Some(app.epoch);
                    auth.set_net_phase("connected".into());
                    auth.set_net_message(tr("auth_network_connected_note"));
                    if qr_source && app.pending_network.is_none()
                        && ["welcome", "login"].contains(&app.route.as_str()) {
                        app.options.offline = false;
                        app.online = true;
                        app.start_login();
                    } else {
                        auth.set_net_visible(true);
                    }
                }
                Err(_) => {
                    app.online = false;
                    auth.set_net_phase("failed".into());
                    auth.set_net_message(tr("auth_network_check_failed"));
                    if qr_source {
                        auth.set_phase("error".into());
                        auth.set_status_message(tr("auth_network_check_failed"));
                    } else {
                        auth.set_net_visible(true);
                    }
                }
            }
        });
    }

    pub(super) fn auth_action(&mut self, action: &str, value: &str) -> Result<()> {
        match action {
            "skip" => {
                self.client.cancel_login(); self.qr_key = None;
                self.invalidate("qr-generate"); self.invalidate("qr-poll");
                self.invalidate("network-check"); self.qr_polling = false;
                if let Some(ui) = self.ui.upgrade() {
                    let auth = ui.global::<AuthModel>();
                    auth.set_step(2); auth.set_status_message("".into());
                    auth.set_net_visible(false); auth.set_net_phase("idle".into());
                }
            }
            "back" => {
                self.invalidate("network-check");
                if let Some(ui) = self.ui.upgrade() {
                    let auth = ui.global::<AuthModel>();
                    auth.set_net_visible(false); auth.set_net_phase("idle".into());
                    auth.set_status_message("".into());
                }
                if self.route == "welcome" { self.start_login(); }
                else {
                    self.client.cancel_login(); self.qr_key = None; self.qr_polling = false;
                    self.invalidate("qr-generate"); self.invalidate("qr-poll");
                    self.finish_auth_return();
                }
            }
            "refresh" => {
                let retry_network = self.ui.upgrade().is_some_and(|ui| {
                    let auth = ui.global::<AuthModel>();
                    auth.get_phase() == "error" && auth.get_net_phase() == "failed"
                });
                self.invalidate("network-check");
                if retry_network { self.check_auth_network(true); } else { self.start_login(); }
            }
            "direction" => { if ["ltr", "rtl"].contains(&value) { self.preferences.direction = value.into(); } }
            "mode" => { if ["page", "panel", "strip"].contains(&value) { self.preferences.mode = value.into(); } }
            "landscape" => { if ["spread", "single"].contains(&value) { self.preferences.landscape = value.into(); } }
            "finish" => {
                self.preferences.onboarded = true; self.save_preferences()?; self.navigate("shelf");
            }
            "login-return" => self.finish_auth_return(),
            "connect" => self.check_auth_network(true),
            "net-connect" => self.check_auth_network(false),
            "net-continue" => {
                let Some(ui) = self.ui.upgrade() else { return Ok(()); };
                let auth = ui.global::<AuthModel>();
                let phase = auth.get_net_phase();
                let checked = self.network_check_approved(&phase);
                let preview = self.demo.is_some() && phase == "preview";
                if !checked && !preview { return Ok(()); }
                self.invalidate("network-check");
                self.options.offline = false;
                self.online = true;
                auth.set_net_visible(false);
                auth.set_net_phase("idle".into());
                auth.set_net_message("".into());
                if self.demo.is_none() { self.recover_downloads(); }
                if let Some((section, action, value)) = self.pending_network.take() {
                    self.dispatch(&section, &action, &value)?;
                } else if ["welcome", "login"].contains(&self.route.as_str()) {
                    self.start_login();
                    if preview {
                        auth.set_status_message(tr("auth_network_preview_note"));
                        auth.set_expires_label(tr("auth_network_preview_title"));
                    }
                }
            }
            "net-close" => {
                self.invalidate("network-check");
                self.pending_network = None;
                if let Some(ui) = self.ui.upgrade() {
                    let auth = ui.global::<AuthModel>();
                    auth.set_net_visible(false); auth.set_net_phase("idle".into()); auth.set_net_message("".into());
                }
            }
            "net-downloads" => {
                self.invalidate("network-check");
                self.pending_network = None;
                if let Some(ui) = self.ui.upgrade() {
                    let auth = ui.global::<AuthModel>();
                    auth.set_net_visible(false); auth.set_net_phase("idle".into()); auth.set_net_message("".into());
                }
                self.navigate("downloads");
            }
            "net-settings" => {
                if let Some(ui) = self.ui.upgrade() { ui.global::<AuthModel>().set_net_message(tr("auth_network_manual_help")); }
                self.toast("auth_network_manual_help");
            }
            _ => {}
        }
        if let Some(ui) = self.ui.upgrade() {
            let model = ui.global::<AuthModel>();
            model.set_direction(self.preferences.direction.clone().into());
            model.set_mode(self.preferences.mode.clone().into());
            model.set_landscape(self.preferences.landscape.clone().into());
        }
        Ok(())
    }

    pub(super) fn switch_session(&mut self, session: AuthSession) -> Result<()> {
        let account = session.account_key().context("A verified account identity is required")?.to_owned();
        let catalog_return = if self.route == "login" && self.auth_return == "detail" {
            self.capture_catalog_auth_return()
        } else { None };
        self.save_reading_position()?;
        self.save_preferences()?;
        self.pause_download_metadata()?;
        let next_epoch = self.epoch.wrapping_add(1);
        // Cancel old work while retaining this account epoch if durable activation fails.
        if let Some(manager) = &mut self.downloads { manager.invalidate_account(self.epoch)?; }
        let previous_credential_epoch = self.client.credential_epoch();
        let activation_error = match self.client.replace_session(Some(session.clone())) {
            Ok(()) => None,
            Err(error) if self.client.credential_epoch() != previous_credential_epoch
                && self.client.session().as_ref() == Some(&session) => Some(error),
            Err(error) => return Err(error.into()),
        };
        // Session publication is the account boundary. Everything below this
        // point must remain empty or belong exclusively to the new account.
        self.epoch = next_epoch;
        self.account = account;
        self.clear_activated_account_context();
        self.stage_catalog_auth_return(catalog_return.clone());
        if let Some(error) = activation_error { return Err(error.into()); }
        let prepared = (|| -> Result<()> {
            self.store.set_preference(&self.account, "renewal_attempt", &"new_login")?;
            self.preferences = self.store.preference(&self.account, "preferences")?.unwrap_or_default();
            self.store.set_preference("device", "last_account", &self.account)?;
            self.downloads = Some(DownloadManager::new(&self.data_dir, self.account.clone(), self.epoch,
                Arc::new(self.download_source()?), self.handle.clone())?);
            self.recover_downloads();
            self.initialize_catalog()?;
            self.stage_catalog_auth_return(catalog_return.clone());
            self.initialize_account()?;
            self.initialize_downloads()?;
            Ok(())
        })();
        if prepared.is_err() {
            // Metadata preparation may have populated only part of the new
            // context. Keep that failure explicit rather than reviving old RAM.
            let new_account_preferences = self.preferences.clone();
            self.clear_activated_account_context();
            self.preferences = new_account_preferences;
            self.stage_catalog_auth_return(catalog_return);
        } else if let Some(ui) = self.ui.upgrade() {
            ui.global::<AuthModel>().set_status_message("".into());
        }
        if let Some(ui) = self.ui.upgrade() {
            let auth = ui.global::<AuthModel>();
            auth.set_direction(self.preferences.direction.clone().into());
            auth.set_mode(self.preferences.mode.clone().into());
            auth.set_landscape(self.preferences.landscape.clone().into());
        }
        prepared
    }

    fn clear_activated_account_context(&mut self) {
        self.retire_reader_checkpoints();
        self.jobs.clear();
        self.pending.clear();
        self.network_verified_epoch = None;
        self.pending_network = None;
        self.qr_key = None;
        self.qr_polling = false;
        self.clear_catalog_auth_context();
        self.reading = None;
        self.downloads = None;
        self.download_ui = DownloadState::default();
        self.account_ui = AccountState::failed();
        self.preferences = Preferences::default();
        self.cover_files.clear();
        self.cover_images.clear();
        self.cover_pending.clear();
        self.cover_failed.clear();
        self.cover_state.clear();
        self.clear_sleep_screen();
        self.toast_until = None;
        self.dirty = true;
        let Some(ui) = self.ui.upgrade() else { return; };
        let auth = ui.global::<AuthModel>();
        let session = self.client.session();
        auth.set_authenticated(session.as_ref().and_then(AuthSession::account_key).is_some());
        auth.set_account_name(session.as_ref().map(|session| session.account_name().to_owned()).unwrap_or_default().into());
        auth.set_direction(self.preferences.direction.clone().into());
        auth.set_mode(self.preferences.mode.clone().into());
        auth.set_landscape(self.preferences.landscape.clone().into());
        auth.set_phase("error".into());
        auth.set_qr(slint::Image::default());
        auth.set_expires_label("".into());
        auth.set_status_message(tr("common_storage_error"));
        auth.set_net_visible(false);
        auth.set_net_phase("idle".into());
        auth.set_net_message("".into());
        ui.global::<ShellModel>().set_toast("".into());
        ui.global::<ShellModel>().set_pending(0);

        // A failed refresh must not leave the previous account's Slint models
        // alive behind an otherwise empty application controller.
        let catalog = ui.global::<CatalogModel>();
        catalog.set_detail(ComicItem::default());
        catalog.set_resume(ComicItem::default());
        catalog.set_has_resume(false);
        catalog.set_shelf_items(model(Vec::new()));
        catalog.set_store_items(model(Vec::new()));
        catalog.set_search_items(model(Vec::new()));
        catalog.set_suggestions(model(Vec::new()));
        catalog.set_episodes(model(Vec::new()));
        catalog.set_history(model(Vec::new()));
        catalog.set_query("".into());
        catalog.set_composition("".into());
        catalog.set_candidates(model(Vec::new()));
        catalog.set_search_result_label("".into());
        catalog.set_shelf_count_label("".into());
        catalog.set_sync_label("".into());
        catalog.set_filter_counts(model(Vec::new()));
        catalog.set_local_download_count(0);
        catalog.set_detail_id_label("".into());
        catalog.set_detail_total(0);
        catalog.set_detail_genres(model(Vec::new()));
        catalog.set_download_selecting(false);
        catalog.set_selection_count(0);
        catalog.set_selection_size("".into());
        catalog.set_status_message(tr("common_storage_error"));

        let reader = ui.global::<ReaderModel>();
        reader.set_page(slint::Image::default());
        reader.set_second_page(slint::Image::default());
        reader.set_overview_image(slint::Image::default());
        reader.set_thumbnails(model(Vec::new()));
        reader.set_thumbnails_loading(false);
        reader.set_chapters(model(Vec::new()));
        reader.set_chapter_count(0);
        reader.set_comic_title("".into());
        reader.set_episode_title("".into());
        reader.set_transition_episode_title("".into());
        reader.set_progress_label("".into());
        reader.set_cache_label("".into());
        reader.set_next_title("".into());
        reader.set_next_price("".into());
        reader.set_next_note("".into());
        reader.set_wallet_label("".into());
        reader.set_coupon_label("".into());
        reader.set_page_count(0);
        reader.set_page_index(1);
        reader.set_seek_index(1);
        reader.set_seek_count(0);
        reader.set_menu(false);
        reader.set_guidance(false);
        reader.set_transition(false);
        reader.set_sheet("".into());
        reader.set_next_available(false);
        reader.set_next_locked(true);
        reader.set_zoom_available(false);
        reader.set_loading(false);
        reader.set_failed(true);
        reader.set_error_message(tr("common_storage_error"));

        let downloads = ui.global::<DownloadModel>();
        downloads.set_items(model(Vec::new()));
        downloads.set_chapters(model(Vec::new()));
        downloads.set_selecting(false);
        downloads.set_selected_count(0);
        downloads.set_sheet_open(false);
        downloads.set_sheet_id("".into());
        downloads.set_sheet_title("".into());
        downloads.set_sheet_summary("".into());
        downloads.set_sheet_cover(slint::Image::default());
        downloads.set_task_visible(false);
        downloads.set_can_read(false);
        downloads.set_summary("".into());

        let purchase = ui.global::<PurchaseModel>();
        purchase.set_phase("error".into());
        purchase.set_comic_title("".into());
        purchase.set_episode_title("".into());
        purchase.set_cover(slint::Image::default());
        purchase.set_ranges(model(Vec::new()));
        purchase.set_result_rows(model(Vec::new()));
        purchase.set_selected_range("".into());
        purchase.set_scope_label("".into());
        purchase.set_quote_ready(false);
        purchase.set_quote_loading(false);
        purchase.set_result_can_read(false);
        purchase.set_total(tr("common_unknown"));
        purchase.set_after_text("".into());
        purchase.set_wallet_label(tr("common_unknown"));
        purchase.set_coupon_label(tr("common_unknown"));
        purchase.set_error_text(tr("common_storage_error"));
        purchase.set_pay_label(tr("buy_confirm"));
        purchase.set_hint(tr("common_storage_error"));

        let account = ui.global::<AccountModel>();
        account.set_signed_in(session.as_ref().and_then(AuthSession::account_key).is_some());
        account.set_username(auth.get_account_name());
        account.set_initial(session.as_ref().and_then(|session| session.account_name().chars().next()).map(|value| value.to_string()).unwrap_or_default().into());
        account.set_session_status(tr("common_storage_error"));
        account.set_balance(tr("common_unknown"));
        account.set_coupons(tr("common_unknown"));
        account.set_balance_time("".into());
        account.set_last_purchase("".into());
        account.set_storage_used(tr("common_unknown"));
        account.set_storage_free(tr("common_unknown"));
        account.set_manual_used(tr("common_unknown"));
        account.set_cache_used(tr("common_unknown"));
        account.set_cache_fill(0.0);
        account.set_storage_manual_fill(0.0);
        account.set_storage_cache_fill(0.0);
        account.set_message(tr("common_storage_error"));
        account.set_recharge_visible(false);
        account.set_recharge_phase("error".into());
        account.set_recharge_total(tr("common_unknown"));
        account.set_recharge_message(tr("common_storage_error"));
        account.set_recharge_can_check(false);
        account.set_recharge_qr(slint::Image::default());
        account.set_recharge_rows(model(Vec::new()));
        account.set_recharge_options(model(Vec::new()));
    }

    pub(super) fn logout(&mut self) -> Result<()> {
        if self.demo.is_some() { self.toast("common_demo_login"); return Ok(()); }
        self.close_reader()?;
        self.flush_reader_checkpoints()?;
        self.pause_download_metadata()?;
        let next_epoch = self.epoch.wrapping_add(1);
        if let Some(manager) = &mut self.downloads { manager.invalidate_account(self.epoch)?; }
        self.client.cancel_login();
        let previous_credential_epoch = self.client.credential_epoch();
        let activation_error = match self.client.replace_session(None) {
            Ok(()) => None,
            Err(error) if self.client.credential_epoch() != previous_credential_epoch
                && self.client.session().is_none() => Some(error),
            Err(error) => return Err(error.into()),
        };
        self.epoch = next_epoch;
        self.retire_reader_checkpoints();
        self.downloads = None;
        self.jobs.clear();
        self.pending.clear(); self.pending_network = None;
        self.network_verified_epoch = None;
        self.cover_state.clear();
        self.cover_files.clear(); self.cover_images.clear();
        self.cover_pending.clear(); self.cover_failed.clear();
        self.clear_sleep_screen();
        self.qr_key = None; self.qr_polling = false; self.reading = None;
        self.initialize_account()?;
        if let Some(error) = activation_error { return Err(error.into()); }
        self.toast("common_signed_out");
        Ok(())
    }

    pub(super) fn check_session(&mut self) {
        if self.demo.is_some() || self.options.offline { return; }
        let client = match self.client.bound_to_account(&self.account) {
            Ok(client) => client,
            Err(error) => { self.source_failure(&error); return; }
        };
        self.source_job("session-check", async move { client.check_session_and_invalidate().await }, |app, result| match result {
            Ok(info) => app.apply_renewal_info(info),
            Err(error) => app.source_failure(&error),
        });
    }

    fn apply_renewal_info(&mut self, info: RenewalInfo) {
        let previous = self.store.preference::<String>(&self.account, "renewal_attempt").ok().flatten().unwrap_or_default();
        let Some(session) = self.client.session() else { return; };
        if session.renewal_is_uncertain() || previous == "confirm_sending" || previous == "uncertain" {
            self.toast("auth_confirmation_required");
            return;
        }
        if session.needs_refresh_confirmation() {
            self.confirm_saved_renewal();
            return;
        }
        if previous == "sending" {
            self.toast("auth_confirmation_required");
            return;
        }
        if !info.refresh { return; }
        if self.store.set_preference(&self.account, "renewal_attempt", &"sending").is_err() {
            self.toast("common_storage_error"); return;
        }
        let client = self.client.clone();
        let Some((account, epoch)) = client.credential_context() else { self.toast("common_session_expired"); return; };
        self.source_job("session-renewal", async move { client.refresh_and_activate_for_session(&account, epoch).await }, |app, result| {
            match result {
                Ok(_) => {
                    let Some(session) = app.client.session() else { return; };
                    if session.renewal_is_uncertain() {
                        let _ = app.store.set_preference(&app.account, "renewal_attempt", &"uncertain");
                        app.toast("auth_confirmation_required");
                        return;
                    }
                    let needs_confirmation = session.needs_refresh_confirmation();
                    if app.store.set_preference(&app.account, "renewal_attempt", &if needs_confirmation { "replacement_saved" } else { "confirmed" }).is_err() {
                        app.toast("common_storage_error"); return;
                    }
                    if needs_confirmation { app.confirm_saved_renewal(); }
                }
                Err(error) => {
                    let state = if error.transmitted == Some(false) || error.definitive { "rejected" } else { "uncertain" };
                    let _ = app.store.set_preference(&app.account, "renewal_attempt", &state);
                    app.source_failure(&error);
                }
            }
        });
    }

    fn confirm_saved_renewal(&mut self) {
        let Some(session) = self.client.session() else { return; };
        if !session.needs_refresh_confirmation() { return; }
        let client = self.client.clone();
        let Some((account, epoch)) = client.credential_context() else { return; };
        if self.store.set_preference(&self.account, "renewal_attempt", &"confirm_sending").is_err() {
            self.toast("common_storage_error"); return;
        }
        self.source_job("session-confirm", async move { client.confirm_and_activate_for_session(&account, epoch).await }, |app, result| {
            match result {
                Ok(_) => {
                    let Some(session) = app.client.session() else { return; };
                    if session.needs_refresh_confirmation() || session.renewal_is_uncertain() {
                        app.toast("auth_confirmation_required");
                        return;
                    }
                    if app.store.set_preference(&app.account, "renewal_attempt", &"confirmed").is_err() {
                        app.toast("common_storage_error");
                    }
                }
                Err(error) => {
                    let state = if error.transmitted == Some(false) && !app.client.session().is_some_and(|session| session.renewal_is_uncertain()) {
                        "replacement_saved"
                    } else { "uncertain" };
                    let _ = app.store.set_preference(&app.account, "renewal_attempt", &state);
                    app.source_failure(&error);
                }
            }
        });
    }
}

pub(super) fn qr_image(payload: &str) -> Result<slint::Image> {
    use qrcode::{Color, QrCode};
    let code = QrCode::new(payload.as_bytes()).context("Encode the login QR")?;
    let modules = code.width();
    let quiet = 4usize;
    let factor = 4usize;
    let side = (modules + quiet * 2) * factor;
    let mut rgba = vec![255u8; side * side * 4];
    for row in 0..modules {
        for column in 0..modules {
            if code[(column, row)] == Color::Dark {
                for y in 0..factor { for x in 0..factor {
                    let offset = (((row + quiet) * factor + y) * side + (column + quiet) * factor + x) * 4;
                    rgba[offset..offset + 3].fill(0);
                }}
            }
        }
    }
    let pixels = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&rgba, side as u32, side as u32);
    Ok(slint::Image::from_rgba8(pixels))
}

#[cfg(test)]
#[path = "auth_resource_tests.rs"]
mod resource_tests;
