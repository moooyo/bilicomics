//! Software-renderer fixtures for states that need upstream observations or hardware.
//!
//! The normal demo controller seeds each group. Exceptional observations then go
//! directly to its presentation model; they are not login/payment/driver tests.

use std::{fs, path::{Path, PathBuf}, rc::Rc, sync::Arc, time::Instant};

use anyhow::{Context, Result, ensure};
use bilicomics::{
    app::{App, AppOptions}, fonts::Fonts, i18n,
    platform::{DeviceProfile, Platform},
    ui::{AccountInfoItem, AccountModel, AccountRecordItem, AppWindow, AuthModel, CatalogModel,
        EpisodeItem, PurchaseModel, ReaderModel, ShellModel, SleepScreenModel, Theme},
};
use serde_json::json;
use slint::{ComponentHandle, Image, LogicalPosition, Model, ModelRc, SharedString, VecModel,
    platform::{PointerEventButton, WindowAdapter, WindowEvent,
        software_renderer::{MinimalSoftwareWindow, RepaintBufferType}}};

struct SoftwarePlatform(Rc<MinimalSoftwareWindow>, Instant);

impl slint::platform::Platform for SoftwarePlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }

    fn duration_since_start(&self) -> std::time::Duration { self.1.elapsed() }
}

fn model<T: Clone + 'static>(items: Vec<T>) -> ModelRc<T> { Rc::new(VecModel::from(items)).into() }
fn text(key: &str) -> SharedString { i18n::text(key).into() }

fn snapshot(ui: &AppWindow, root: &Path, profile: DeviceProfile, name: &str) -> Result<()> {
    slint::platform::update_timers_and_animations();
    ui.window().request_redraw();
    let img = ui.window().take_snapshot().map_err(|error| anyhow::anyhow!("Render {name}: {error}"))?;
    let (width, height) = profile.logical_size();
    ensure!((img.width(), img.height()) == (2 * width, 2 * height), "Expected the physical profile dimensions for {name}");
    let path = root.join(format!("{name}.png"));
    ensure!(!path.exists(), "Refusing to replace {name}");
    image::save_buffer(&path, img.as_bytes(), img.width(), img.height(), image::ColorType::Rgba8)?;
    fs::write(path.with_extension("json"), serde_json::to_vec_pretty(&json!({
        "profile": profile.id(), "image": name, "scope": "synthetic presentation fixture; no upstream observation",
        "size": [img.width(), img.height()], "scale": 2, "live_account": false, "physical_device": false,
    }))?)?;
    Ok(())
}

fn tap(ui: &AppWindow, x: f32, y: f32) {
    let position = LogicalPosition::new(x, y);
    ui.window().dispatch_event(WindowEvent::PointerPressed { position, button: PointerEventButton::Left });
    ui.window().dispatch_event(WindowEvent::PointerReleased { position, button: PointerEventButton::Left });
}

#[test]
#[ignore = "Run on test-env with a new evidence directory"]
fn exceptional_handoff_states() -> Result<()> {
    let root = PathBuf::from(std::env::var_os("BILICOMICS_HANDOFF_FIXTURES").context("Supply a new evidence directory")?);
    fs::create_dir(&root)?;
    let data = tempfile::tempdir()?;
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(SoftwarePlatform(window.clone(), Instant::now())))
        .map_err(|error| anyhow::anyhow!("Install software capture platform: {error}"))?;
    let ui = AppWindow::new()?;
    Fonts::load()?.register(&ui)?;
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build()?;
    let _runtime = runtime.enter();
    let qr = Image::load_from_path(Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/demo/qr-login.png").as_path())?;
    let page = Image::load_from_path(Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/demo/pages/page-19.png").as_path())?;
    let fixture: serde_json::Value = serde_json::from_str(include_str!("../assets/demo/catalog.json"))?;
    let title = fixture["comics"].as_array().and_then(|rows| rows.first()).and_then(|row| row["title"].as_str()).context("Demo title")?;
    let sleep_artwork = fixture["comics"][0]["sleep_cover_file"].as_str().context("Demo sleep artwork")?;
    let sleep_cover = Image::load_from_path(&Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/demo").join(sleep_artwork))?;
    for profile in [DeviceProfile::Paperwhite6, DeviceProfile::Scribe] {
        let raw = root.join(profile.id());
        fs::create_dir(&raw)?;
        let (width, height) = profile.logical_size();
        window.dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: 2.0 });
        window.set_size(slint::PhysicalSize::new(width * 2, height * 2));
        ui.global::<Theme>().set_scribe(profile.is_scribe());
        ui.show()?;
        let capture = |name: &str| snapshot(&ui, &raw, profile, name);
        let options = AppOptions { data_dir: data.path().join(profile.id()), profile, demo: true,
            offline: false, initial_screen: "shelf".into(), landscape: false };
        let app = App::attach_prepared(&ui, App::prepare(options)?, Arc::new(Platform::new(profile, false)?), runtime.handle().clone())?;
        while !app.borrow_mut().advance_startup()? {}
        let shell = ui.global::<ShellModel>();
        shell.set_booting(false);
        let catalog = ui.global::<CatalogModel>();
        for state in ["empty", "syncing"] {
            catalog.set_shelf_state(state.into());
            capture(&format!("shelf-{state}"))?;
        }
        catalog.set_shelf_state("signed-out".into());
        catalog.set_local_download_count(8);
        capture("shelf-signedout")?;

        let auth = ui.global::<AuthModel>();
        app.borrow_mut().dispatch("shell", "navigate", "store")?;
        let target = fixture["comics"].as_array().into_iter().flatten().find(|comic| comic["id"] == 28704)
            .and_then(|comic| comic["title"].as_str()).context("Demo network target")?;
        auth.set_net_visible(true);
        auth.set_net_action(i18n::format("network_open_title", &[("title", target.to_owned())]).into());
        for (phase, file) in [("connecting", "checking"), ("failed", "failed"), ("connected", "ok")] {
            auth.set_net_phase(phase.into());
            capture(&format!("network-{file}"))?;
        }
        auth.set_net_visible(false);
        app.borrow_mut().dispatch("shell", "navigate", "welcome")?;
        auth.set_qr(qr.clone());
        for phase in ["scanned", "expired"] {
            auth.set_phase(phase.into());
            capture(&format!("welcome-{phase}"))?;
        }
        auth.set_authenticated(true);
        auth.set_account_name(fixture["account"]["name"].as_str().context("Demo account name")?.into());
        auth.set_phase("success".into());
        app.borrow_mut().tick()?;
        ensure!(auth.get_step() == 1, "Onboarding must wait for acknowledgement after sign-in");
        app.borrow_mut().dispatch("auth", "login-return", "")?;
        ensure!(auth.get_step() == 2, "Acknowledged onboarding did not show reading preferences");
        capture("welcome-prefs")?;

        app.borrow_mut().dispatch("shell", "navigate", "account")?;
        app.borrow_mut().dispatch("account", "login", "")?;
        app.borrow_mut().dispatch("auth", "login-return", "")?;
        ensure!(shell.get_route() == "login", "An unconfirmed login must not complete");
        auth.set_phase("success".into());
        app.borrow_mut().tick()?;
        ensure!(shell.get_route() == "login", "A confirmed sign-in must wait on the result screen");
        capture("login-success")?;
        tap(&ui, width as f32 / 2.0, if profile.is_scribe() { 889.0 } else { 629.0 });
        ensure!(shell.get_route() == "account", "The login success button did not return to Account");

        app.borrow_mut().dispatch("shell", "navigate", "login")?;
        auth.set_return_route("detail".into());
        capture("login-from-detail")?;

        app.borrow_mut().dispatch("catalog", "comic.open", "28601")?;
        let mut guest = catalog.get_detail();
        guest.in_shelf = false;
        catalog.set_detail(guest);
        capture("detail-signedout")?;
        let original = catalog.get_episodes();
        catalog.set_episodes(model(original.iter().map(|row| if row.order.as_str() == "13" {
            EpisodeItem { access: text("cat_owned"), readable: true, locked: false, ..row }
        } else { row }).collect()));
        capture("detail-bought")?;
        catalog.set_episodes(original);

        app.borrow_mut().dispatch("purchase", "open", "2860113")?;
        let purchase = ui.global::<PurchaseModel>();
        app.borrow_mut().dispatch("purchase", "range", "batch:1")?;
        purchase.set_sufficient(false);
        purchase.set_wallet_label(i18n::format("acct_wallet_amount", &[("amount", "20".into())]).into());
        purchase.set_after_text(i18n::format("buy_short_coin", &[("amount", "130".into())]).into());
        capture("purchase-low")?;
        app.borrow_mut().dispatch("purchase", "range", "single")?;
        let amount = purchase.get_total();
        purchase.set_phase("submitting".into());
        capture("purchase-submitting")?;
        purchase.set_pending_summary(format!("{title} · {} · {amount}", purchase.get_scope_label()).into());
        purchase.set_phase("pending".into());
        capture("purchase-pending")?;
        purchase.set_phase("done".into());
        purchase.set_result_can_read(true);
        purchase.set_result_title(text("buy_success_title"));
        purchase.set_result_body("".into());
        capture("purchase-done")?;

        app.borrow_mut().dispatch("shell", "navigate", "account")?;
        let account = ui.global::<AccountModel>();
        app.borrow_mut().dispatch("account", "purchase-history", "")?;
        account.set_purchase_rows(model([("12", "32", "11-22 16:29"), ("17", "150", "11-20 21:04"), ("31", "32", "11-18 08:40")]
            .into_iter().map(|(chapter, amount, time)| AccountRecordItem {
                title: format!("{title} · {}", i18n::format("common_chapter", &[("episode", chapter.into())])).into(),
                amount: format!("{amount} {}", i18n::text("buy_coin")).into(),
                status: text("acct_entitlement"), time: time.into(),
            }).collect()));
        capture("account-purchases")?;
        app.borrow_mut().dispatch("account", "recharge-history", "")?;
        let example_recharges = ["300", "680", "60"].into_iter().zip(["11-20 21:04", "10-24 20:19", "09-14 12:11"])
            .map(|(amount, time)| AccountRecordItem {
                title: format!("{amount} {}", i18n::text("buy_coin")).into(), time: time.into(),
                ..Default::default()
            }).collect::<Vec<_>>();
        account.set_recharge_rows(model(example_recharges.clone()));
        account.set_recharge_history_status("".into());
        capture("account-recharges")?;
        for section in ["display", "home"] {
            app.borrow_mut().dispatch("account", "section", section)?;
            account.set_jpeg_acceleration_available(true);
            if section == "display" { capture("acct-display-jpeg")?; }
        }
        account.set_recharge_options(model([("60", "6"), ("300", "30"), ("680", "68"), ("1,280", "128")]
            .into_iter().map(|(count, price)| AccountInfoItem { label: format!("{count} {}", i18n::text("buy_coin")).into(), value: format!("¥{price}").into() }).collect()));
        account.set_recharge_total("¥30".into());
        let purchased_coins = SharedString::from(format!("300 {}", i18n::text("buy_coin")));
        account.set_recharge_coins(purchased_coins.clone());
        account.set_recharge_index(1);
        account.set_recharge_visible(true);
        account.set_recharge_phase("select".into());
        capture("account-recharge")?;
        account.set_recharge_phase("qr".into());
        account.set_recharge_qr(qr.clone());
        account.set_recharge_can_check(true);
        capture("account-recharge-qr")?;
        account.set_recharge_visible(false);
        app.borrow_mut().dispatch("account", "recharge-history", "")?;
        account.set_recharge_history_status("".into());
        account.set_balance("1,580".into());
        account.set_recharge_rows(model(std::iter::once(AccountRecordItem {
            title: purchased_coins, time: "16:30".into(), ..Default::default()
        }).chain(example_recharges).collect()));
        capture("account-recharged")?;

        app.borrow_mut().dispatch("reader", "open-episode", "28601:2860112")?;
        app.borrow_mut().dispatch("reader", "menu", "open")?;
        let reader = ui.global::<ReaderModel>();
        let normal_title = reader.get_episode_title();
        reader.set_loading(false);
        reader.set_page(page.clone());
        reader.set_seek_index(1);
        reader.set_progress_label("1 / 45".into());
        reader.set_episode_title(i18n::format("cat_chapter_title", &[("episode", "13".into()),
            ("title", fixture["purchase"]["episode_title"].as_str().context("Demo purchase episode")?.into())]).into());
        reader.set_download_state("".into());
        capture("reader-menu-ep13")?;
        reader.set_download_state("queued".into());
        capture("reader-menu-ep13-q")?;
        app.borrow_mut().dispatch("reader", "sheet", "display")?;
        reader.set_loading(false);
        reader.set_page(Image::load_from_path(Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/demo/pages/page-18.png").as_path())?);
        reader.set_episode_title(normal_title);
        reader.set_jpeg_acceleration_available(true);
        if !profile.is_scribe() { tap(&ui, 96.0, height as f32 - 300.0); }
        capture(if profile.is_scribe() { "reader-settings-jpeg" } else { "reader-display-jpeg" })?;
        let sleep = ui.global::<SleepScreenModel>();
        sleep.set_active(true);
        sleep.set_has_cover(true);
        sleep.set_cover(sleep_cover.clone());
        sleep.set_title(title.into());
        capture("sleep")?;
        sleep.set_active(false);
        app.borrow_mut().shutdown()?;
        ui.hide()?;
    }
    Ok(())
}
