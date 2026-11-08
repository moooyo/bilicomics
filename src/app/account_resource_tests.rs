//! Account-state checks through headless application resource admission.

use super::*;
use crate::source::{AccountIdentity, RechargeOption};

fn current_quote(account: &str) -> Result<OfferSnapshot> {
    let value = json!({"ep_id":"7","comic_id":"9","is_locked":true,
        "ep_original_gold":32,"pay_gold":32,"remain_gold":1280,
        "remain_coupon":2,"allow_coupon":true,"ep_pay_coupons":1,
        "recommend_coupon_ids":["resource-coupon"],"eligible_coupon_ids":["resource-coupon"]});
    let read = QuoteRead {
        basic: value.clone(), scoped: value, wallet: json!({"remain_gold":1280,"remain_coupon":2}),
        detail: ComicDetail { comic: Comic { id: "9".into(), ..Default::default() },
            episodes: vec![Episode { id: "7".into(), comic_id: "9".into(), order: 1.0,
                access: EpisodeAccess::Locked, pay_gold: Some(32.0), ..Default::default() }], extra: Value::Null },
        observed_at: now(),
    };
    Ok(normalize_offer(account, "7", "single", "coin", &read)?)
}

#[tokio::test]
async fn full_durable_admission_leaves_purchase_and_recharge_unsubmitted_without_new_journals() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let client = BiliClient::offline(directory.path())?;
    let session = AuthSession::from_cookie_text("SESSDATA=resource-fixture; bili_jct=resource-csrf; DedeUserID=11")?
        .with_identity(AccountIdentity { id: "11".into(), name: "Resource Fixture".into(), avatar_url: String::new() }, now())?;
    client.replace_session(Some(session))?;
    let mut app = super::super::test_support::app(directory.path(), DeviceProfile::Paperwhite5,
        client, tokio::runtime::Handle::current())?;
    assert!(!app.account_receipts_pending(), "Another test left an observed financial receipt in memory");
    let held = (0..16).map(|index| app.begin_job(&format!("resource-durable-{index}"), true))
        .collect::<Result<Vec<_>>>()?;
    let sequence_before = app.sequence;
    app.account_ui.phase = "preflight".into();
    app.dispatch_purchase(current_quote(&app.account)?)?;
    assert_eq!(app.account_ui.phase, "review");
    assert!(app.account_ui.current_journal.is_none());
    assert!(app.account_ui.journals.is_empty());
    assert!(app.store.preference::<Vec<PurchaseIntent>>(&app.account, JOURNAL_KEY)?.is_none());

    app.account_ui.recharge.phase = "select".into();
    app.account_ui.recharge.config = Some(RechargeConfig {
        options: vec![RechargeOption { amount_cents: 500, coin_amount: 500,
            fingerprint: "a".repeat(64), extra: Value::Null }],
        observed_at: now(), extra: Value::Null,
    });
    app.create_recharge_order()?;
    assert_eq!(app.account_ui.recharge.phase, "select");
    assert!(!app.account_ui.recharge.busy);
    assert!(app.account_ui.recharge.journal.is_none());
    assert!(app.store.preference::<RechargeIntent>(&app.account, RECHARGE_KEY)?.is_none());
    assert_eq!(app.sequence, sequence_before);
    assert!(!app.pending.keys().any(|key| key.starts_with("purchase-submit:") || key.starts_with("recharge-create:")));
    assert!(!app.account_receipts_pending());
    drop(held);
    Ok(())
}

#[tokio::test]
async fn headless_wallet_labels_follow_current_account_state_without_a_slint_projection() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let client = BiliClient::offline(directory.path())?;
    let mut app = super::super::test_support::app(directory.path(), DeviceProfile::Paperwhite5,
        client, tokio::runtime::Handle::current())?;
    assert!(app.ui.upgrade().is_none());
    app.account_ui.wallet = Some(json!({"remain_gold":17,"remain_coupon":2}));
    assert_eq!(app.account_wallet_labels(), ("17".into(), "2".into()));
    app.account = "bili_22".into();
    app.account_ui = AccountState::default();
    let unknown = i18n::text("common_unknown");
    assert_eq!(app.account_wallet_labels(), (unknown.clone(), unknown));
    app.account_ui.wallet = Some(json!({"remain_gold":23,"remain_coupon":4}));
    assert_eq!(app.account_wallet_labels(), ("23".into(), "4".into()));
    Ok(())
}
