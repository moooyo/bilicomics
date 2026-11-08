//! Explicit payment reads and submissions traced to the pinned source adapter.
//! The caller owns quote confirmation, durable transaction intent, and reconciliation.

use super::client::{PostOptions, now_seconds, numeric_id};
use super::normalize::safe_extra;
use super::{AuthSession, BiliClient, ErrorKind, PurchaseMethod, PurchaseRequest, PurchaseScope, QuoteScope,
    RechargeConfig, RechargeOption, RechargeOrder, Result, SourceError};
use chrono::Datelike;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashSet};

const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
const MAX_AMOUNT_CENTS: u64 = 2_147_483_647;
const MAX_RECHARGE_RESPONSE: usize = 256 * 1024;
const MAX_PAY_PARAMETERS: usize = 32 * 1024;
const MAX_OPTION_COUNT: usize = 128;
const MAX_BATCH_LIMIT: u32 = 2_147_483_647;

impl BiliClient {
    pub async fn wallet(&self) -> Result<Value> {
        let value = self.post("user.v1.User", "GetWallet", json!({}), authenticated()).await?;
        require_object(&value, "wallet_response_invalid")?;
        Ok(safe_extra(&value))
    }

    pub async fn purchase_info(&self, episode_id: &str, scope: QuoteScope) -> Result<Value> {
        let (body, options) = quote_request(episode_id, scope)?;
        let value = self.post("comic.v1.Comic", "GetEpisodeBuyInfo", body, options).await?;
        normalize_quote(value, episode_id)
    }

    pub async fn discount_list(&self, comic_id: &str, order: u8, original_values: Value) -> Result<Value> {
        validate_discount_order(order)?;
        validate_original_values(&original_values)?;
        let value = self.post("comic.v1.Comic", "GetDiscountList", json!({
            "comic_id": numeric_id(comic_id)?, "order": order, "original_values": original_values,
        }), authenticated()).await?;
        Ok(safe_extra(&value))
    }

    pub async fn discount_price(&self, discount_id: &str, original_values: Value) -> Result<Value> {
        validate_original_values(&original_values)?;
        let value = self.post("comic.v1.Comic", "CalDiscountPrice", json!({
            "id": numeric_id(discount_id)?, "original_values": original_values,
        }), authenticated()).await?;
        Ok(safe_extra(&value))
    }

    /// batch_limit is the explicitly selected card-query amount, not an inferred range count.
    pub async fn free_gold_card_info(&self, comic_id: &str, episode_id: &str,
        buy_type: u8, batch_limit: u32) -> Result<Value> {
        if !(1..=3).contains(&buy_type) || batch_limit > MAX_BATCH_LIMIT {
            return Err(SourceError::invalid("free_gold_card_scope_invalid"));
        }
        let value = self.post("comic.v1.Comic", "GetComicFreeGoldCard", json!({
            "comic_id": numeric_id(comic_id)?, "ep_id": numeric_id(episode_id)?,
            "buy_type": buy_type, "batch_limit": batch_limit,
        }), authenticated()).await?;
        Ok(safe_extra(&value))
    }

    /// Sends once. Neither an omitted amount nor this typed request establishes a valid quote.
    pub async fn buy_episode(&self, request: &PurchaseRequest) -> Result<Value> {
        self.submit_purchase(request, self.session_generation()).await
    }

    /// Binds an application journal's account identity before the transport snapshot is taken.
    pub async fn buy_episode_for_account(&self, request: &PurchaseRequest, expected_account: &str) -> Result<Value> {
        let (epoch, session) = self.session_snapshot();
        validate_expected_account(session.as_ref(), expected_account)?;
        self.submit_purchase(request, epoch).await
    }

    async fn submit_purchase(&self, request: &PurchaseRequest, expected_epoch: u64) -> Result<Value> {
        let body = purchase_payload(request)?;
        let options = PostOptions { expected_epoch: Some(expected_epoch), ..authenticated() };
        let value = self.post("comic.v1.Comic", "BuyEpisode", body, options).await
            .map_err(definitive_not_transmitted)?;
        Ok(safe_extra(&value))
    }

    pub async fn recharge_config(&self) -> Result<RechargeConfig> {
        let value = self.post("pay.v1.Pay", "GetPayConfig", json!({}), authenticated()).await?;
        normalize_config(&value, now_seconds())
    }

    /// Creates one explicit order after rereading the official denomination and benefit terms.
    /// Receiving a QR order is not evidence that the order was paid or credited.
    pub async fn create_recharge_order(&self, amount_cents: u64,
        expected_fingerprint: Option<&str>) -> Result<RechargeOrder> {
        self.create_recharge_order_bound(amount_cents, expected_fingerprint, self.session_generation()).await
    }

    pub async fn create_recharge_order_for_account(&self, amount_cents: u64,
        expected_fingerprint: Option<&str>, expected_account: &str) -> Result<RechargeOrder> {
        let (epoch, session) = self.session_snapshot();
        validate_expected_account(session.as_ref(), expected_account)?;
        self.create_recharge_order_bound(amount_cents, expected_fingerprint, epoch).await
    }

    async fn create_recharge_order_bound(&self, amount_cents: u64,
        expected_fingerprint: Option<&str>, epoch: u64) -> Result<RechargeOrder> {
        if !(1..=MAX_AMOUNT_CENTS).contains(&amount_cents) {
            return Err(SourceError::invalid("invalid_recharge_amount"));
        }
        if expected_fingerprint.is_some_and(|value| value.len() != 64
            || !value.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))) {
            return Err(SourceError::invalid("recharge_fingerprint_invalid"));
        }
        let configuration = self.recharge_config().await.map_err(|mut error| {
            // A configuration read never submits the requested order.
            error.transmitted = Some(false);
            error.definitive = true;
            error.retryable = false;
            error
        })?;
        let option = configuration.options.iter().find(|option| option.amount_cents == amount_cents)
            .ok_or_else(|| SourceError::invalid("recharge_config_changed"))?;
        if expected_fingerprint.is_some_and(|expected| expected != option.fingerprint) {
            return Err(SourceError::invalid("recharge_config_changed"));
        }
        if self.session_generation() != epoch {
            return Err(SourceError { transmitted: Some(false), definitive: true,
                ..SourceError::new(ErrorKind::Cancelled, "account_changed") });
        }
        let options = PostOptions { expected_epoch: Some(epoch), ..authenticated() };
        let value = self.post("pay.v1.Pay", "CreateOrder", json!({
            "pay_type": "qr", "pay_amount": amount_cents,
        }), options).await.map_err(|error| {
            if error.transmitted == Some(false) { definitive_not_transmitted(error) }
            else { unknown_order(None) }
        })?;
        normalize_order(&value, amount_cents, now_seconds())
    }

    pub async fn recharge_history(&self, page: u32, page_size: u32) -> Result<Value> {
        self.recharge_history_for(page, page_size, chrono::Utc::now().year(), 0).await
    }

    /// The observed service uses a year and month filter; month zero means all months.
    pub async fn recharge_history_for(&self, page: u32, page_size: u32,
        order_year: i32, order_month: u8) -> Result<Value> {
        if !(1..=100_000).contains(&page) || !(1..=100).contains(&page_size)
            || !(1970..=9999).contains(&order_year) || order_month > 12 {
            return Err(SourceError::invalid("recharge_history_options_invalid"));
        }
        let value = self.post("user.v1.User", "GetPayOrders", json!({
            "page_num": page, "page_size": page_size,
            "order_year": order_year, "order_month": order_month,
        }), authenticated()).await?;
        normalize_history(&value, page, page_size, order_year, order_month)
    }
}

fn authenticated() -> PostOptions { PostOptions { require_session: true, ..Default::default() } }

fn validate_expected_account(session: Option<&AuthSession>, expected: &str) -> Result<()> {
    if expected.is_empty() { return Err(SourceError::invalid("payment_account_required")); }
    if session.and_then(AuthSession::account_key) != Some(expected) {
        return Err(SourceError { transmitted: Some(false), definitive: true,
            ..SourceError::new(ErrorKind::AccountMismatch, "payment_account_changed") });
    }
    Ok(())
}

fn definitive_not_transmitted(mut error: SourceError) -> SourceError {
    if error.transmitted == Some(false) { error.definitive = true; error.retryable = false; }
    error
}

fn quote_request(episode_id: &str, scope: QuoteScope) -> Result<(Value, PostOptions)> {
    let mut body = json!({"ep_id": numeric_id(episode_id)?});
    let mut options = authenticated();
    match scope {
        QuoteScope::Basic => {},
        QuoteScope::Single { order } => {
            validate_discount_order(order)?;
            body["buy_type"] = json!(1);
            body["order"] = json!(order);
            options.flag = Some("getEpisodeDiscounts");
        }
        QuoteScope::Batch { limit, order } => {
            validate_discount_order(order)?;
            if limit > MAX_BATCH_LIMIT { return Err(SourceError::invalid("quote_batch_limit_invalid")); }
            body["buy_type"] = json!(2);
            body["batch_limit"] = json!(limit);
            body["order"] = json!(order);
            options.flag = Some("getEpisodeDiscounts");
        }
    }
    Ok((body, options))
}

fn normalize_quote(value: Value, episode_id: &str) -> Result<Value> {
    require_object(&value, "quote_response_invalid")?;
    if let Some(reported) = value.get("ep_id") {
        if response_id(reported).as_deref() != Some(episode_id) {
            return Err(SourceError::protocol("quote_episode_mismatch"));
        }
    }
    let comic_id = value.get("comic_id").map(|value| response_id(value)
        .ok_or_else(|| SourceError::protocol("quote_comic_identity_invalid"))).transpose()?;
    let mut sanitized = safe_extra(&value);
    sanitized["ep_id"] = json!(episode_id);
    if let Some(comic_id) = comic_id { sanitized["comic_id"] = json!(comic_id); }
    Ok(sanitized)
}

fn validate_discount_order(order: u8) -> Result<()> {
    if order != 1 && order != 2 { return Err(SourceError::invalid("discount_order_invalid")); }
    Ok(())
}

fn validate_original_values(value: &Value) -> Result<()> {
    let values = value.as_array().filter(|values| !values.is_empty() && values.len() <= 514)
        .ok_or_else(|| SourceError::invalid("discount_original_values_invalid"))?;
    if values.iter().any(|value| !valid_amount(value.as_f64())) {
        return Err(SourceError::invalid("discount_original_values_invalid"));
    }
    Ok(())
}

fn purchase_payload(request: &PurchaseRequest) -> Result<Value> {
    let mut body = Map::new();
    match &request.scope {
        PurchaseScope::Single { episode_id, comic_id } => {
            body.insert("ep_id".into(), json!(numeric_id(episode_id)?));
            if let Some(comic_id) = comic_id { body.insert("comic_id".into(), json!(numeric_id(comic_id)?)); }
        }
        PurchaseScope::OrdinalRange { comic_id, start_order, limit } => {
            if !start_order.is_finite() || *limit > MAX_BATCH_LIMIT {
                return Err(SourceError::invalid("purchase_ordinal_scope_invalid"));
            }
            if !matches!(request.method, PurchaseMethod::Currency) {
                return Err(SourceError::invalid("batch_coupon_purchase_unsupported"));
            }
            body.insert("comic_id".into(), json!(numeric_id(comic_id)?));
            body.insert("with_ord_scope".into(), json!(true));
            body.insert("start_ord".into(), json!(start_order));
            body.insert("limit".into(), json!(limit));
        }
    }
    body.insert("buy_method".into(), json!(match request.method { PurchaseMethod::Coupon => 2, PurchaseMethod::Currency => 3 }));
    if let Some(amount) = request.pay_amount {
        if !valid_amount(Some(amount)) { return Err(SourceError::invalid("purchase_amount_invalid")); }
        body.insert("pay_amount".into(), json!(amount));
    }
    if let Some(coupon) = &request.coupon_id {
        validate_asset_id(coupon)?;
        body.insert("coupon_id".into(), json!(coupon));
    }
    if !request.coupon_ids.is_empty() {
        if request.coupon_ids.len() > 1024 { return Err(SourceError::invalid("purchase_coupon_list_invalid")); }
        let mut seen = BTreeSet::new();
        for coupon in &request.coupon_ids {
            validate_asset_id(coupon)?;
            if !seen.insert(coupon) { return Err(SourceError::invalid("purchase_coupon_list_invalid")); }
        }
        body.insert("coupon_ids".into(), json!(request.coupon_ids));
    }
    if matches!(request.method, PurchaseMethod::Coupon)
        && request.coupon_id.is_none() && request.coupon_ids.is_empty() {
        return Err(SourceError::invalid("purchase_coupons_required"));
    }
    if matches!(request.method, PurchaseMethod::Currency)
        && (request.coupon_id.is_some() || !request.coupon_ids.is_empty()) {
        return Err(SourceError::invalid("currency_purchase_contains_coupons"));
    }
    if let Some(card) = &request.free_gold_card_id {
        validate_asset_id(card)?;
        body.insert("free_gold_card_id".into(), json!(card));
    }
    if let Some(amount) = request.free_gold_amount {
        if !valid_amount(Some(amount)) { return Err(SourceError::invalid("purchase_card_amount_invalid")); }
        body.insert("free_gold_amount".into(), json!(amount));
    }
    if matches!(request.method, PurchaseMethod::Coupon)
        && (request.free_gold_card_id.is_some() || request.free_gold_amount.is_some()) {
        return Err(SourceError::invalid("coupon_purchase_contains_currency_assets"));
    }
    Ok(Value::Object(body))
}

fn validate_asset_id(value: &str) -> Result<()> {
    if value.is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
        return Err(SourceError::invalid("purchase_asset_identifier_invalid"));
    }
    Ok(())
}

fn valid_amount(value: Option<f64>) -> bool {
    value.is_some_and(|value| value.is_finite() && value >= 0.0 && value <= MAX_SAFE_INTEGER as f64)
}

fn response_id(value: &Value) -> Option<String> {
    let text = if let Some(value) = value.as_str() { value.to_owned() }
        else if let Some(value) = safe_integer(value) { value.to_string() }
        else { return None; };
    numeric_id(&text).ok()?;
    Some(text)
}

fn require_object<'a>(value: &'a Value, code: &str) -> Result<&'a Map<String, Value>> {
    value.as_object().ok_or_else(|| SourceError::protocol(code))
}

fn safe_integer(value: &Value) -> Option<u64> {
    value.as_u64().filter(|value| *value <= MAX_SAFE_INTEGER).or_else(|| {
        let value = value.as_f64()?;
        (value.is_finite() && value >= 0.0 && value <= MAX_SAFE_INTEGER as f64 && value.fract() == 0.0)
            .then_some(value as u64)
    })
}

fn cents_from_yuan(value: &Value) -> Option<u64> {
    if let Some(value) = value.as_str() {
        if value.len() > 32 { return None; }
        let value = value.trim();
        let (whole, fraction) = match value.split_once('.') { Some(parts) => parts, None => (value, "") };
        if whole.is_empty() || fraction.len() > 2 || !whole.bytes().all(|byte| byte.is_ascii_digit())
            || !fraction.bytes().all(|byte| byte.is_ascii_digit())
            || (value.contains('.') && fraction.is_empty()) { return None; }
        let whole: u64 = whole.parse().ok()?;
        let fraction: u64 = if fraction.is_empty() { 0 } else { fraction.parse::<u64>().ok()? * if fraction.len() == 1 { 10 } else { 1 } };
        let amount = whole.checked_mul(100)?.checked_add(fraction)?;
        return (1..=MAX_AMOUNT_CENTS).contains(&amount).then_some(amount);
    }
    let value = value.as_f64()?;
    if !value.is_finite() || value <= 0.0 || value > MAX_AMOUNT_CENTS as f64 / 100.0 { return None; }
    let cents = (value * 100.0).round();
    if cents / 100.0 != value || cents < 1.0 || cents > MAX_AMOUNT_CENTS as f64 { return None; }
    Some(cents as u64)
}

fn text(value: &Value, maximum: usize) -> Option<&str> {
    value.as_str().filter(|value| value.len() <= maximum && !value.chars().any(|character|
        character == '\u{7f}' || (character.is_control() && !matches!(character, '\t' | '\r' | '\n'))))
}

fn optional_text(source: &Map<String, Value>, raw: &str, maximum: usize) -> Result<Option<Value>> {
    source.get(raw).map(|value| text(value, maximum).map(|value| json!(value))
        .ok_or_else(|| SourceError::protocol("recharge_terms_invalid"))).transpose()
}

fn optional_integer(source: &Map<String, Value>, raw: &str) -> Result<Option<Value>> {
    source.get(raw).map(|value| safe_integer(value).map(|value| json!(value))
        .ok_or_else(|| SourceError::protocol("recharge_benefit_invalid"))).transpose()
}

fn normalize_config(value: &Value, timestamp: i64) -> Result<RechargeConfig> {
    let source = require_object(value, "recharge_config_invalid")?;
    let ranges = source.get("pay_amount_ranges").and_then(Value::as_array)
        .filter(|values| values.len() <= MAX_OPTION_COUNT)
        .ok_or_else(|| SourceError::protocol("recharge_options_invalid"))?;
    let mut metadata = Map::from_iter([
        ("schema_version".into(), json!(1)), ("source".into(), json!("official_pay_config")),
        ("custom_amount".into(), json!({"allowed": false, "reason": "not_advertised"})),
        ("channels".into(), json!(["wechat", "alipay"])), ("channel_selection".into(), json!("scan_app")),
    ]);
    for (raw, normalized, limit) in [("show_text", "notice", 2048), ("bonus_reason", "bonus_reason", 512)] {
        if let Some(value) = optional_text(source, raw, limit)? { metadata.insert(normalized.into(), value); }
    }
    for (raw, normalized) in [("first_pay_send_type", "first_bonus_type"), ("act_send_type", "activity_bonus_type")] {
        if let Some(value) = optional_integer(source, raw)? { metadata.insert(normalized.into(), value); }
    }
    let mut seen = BTreeSet::new();
    let mut options = Vec::new();
    for raw in ranges {
        let raw = require_object(raw, "recharge_option_invalid")?;
        let amount_cents = raw.get("pay_amount").and_then(cents_from_yuan)
            .ok_or_else(|| SourceError::protocol("recharge_denomination_invalid"))?;
        let coin_amount = raw.get("gold_amount").and_then(safe_integer)
            .ok_or_else(|| SourceError::protocol("recharge_coin_amount_invalid"))?;
        if !seen.insert(amount_cents) { return Err(SourceError::protocol("recharge_denomination_ambiguous")); }
        let mut benefits = Map::from_iter([
            ("amount_cents".into(), json!(amount_cents)), ("coin_amount".into(), json!(coin_amount)),
            ("amount_yuan".into(), json!(format!("{}.{:02}", amount_cents / 100, amount_cents % 100))),
        ]);
        for (raw_name, normalized) in [
            ("first_bonus_amount", "first_bonus_gold"), ("first_coupon_amount", "first_bonus_coupons"),
            ("bonus_coupon_amount", "activity_bonus_coupons"), ("bonus_gold_amount", "activity_bonus_gold"),
            ("break_ice_coupon_amount", "break_ice_coupons"), ("gold_set_amount", "coupon_bundle_amount"),
            ("free_gold_set_amount", "deduction_card_amount"),
        ] {
            if let Some(value) = optional_integer(raw, raw_name)? { benefits.insert(normalized.into(), value); }
        }
        for (raw_name, normalized) in [("first_txt", "first_text"), ("activity_txt", "activity_text")] {
            if let Some(value) = optional_text(raw, raw_name, 512)? { benefits.insert(normalized.into(), value); }
        }
        let fingerprint = option_fingerprint(&benefits, &metadata)?;
        options.push(RechargeOption { amount_cents, coin_amount, fingerprint, extra: Value::Object(benefits) });
    }
    let fingerprints: Vec<_> = options.iter().map(|option| option.fingerprint.as_str()).collect();
    metadata.insert("fingerprint".into(), json!(hash_json(&json!(fingerprints))?));
    Ok(RechargeConfig { options, observed_at: timestamp, extra: Value::Object(metadata) })
}

fn option_fingerprint(option: &Map<String, Value>, config: &Map<String, Value>) -> Result<String> {
    let mut basis = vec![json!("official_manga_recharge_option_v1")];
    for (source, names) in [
        (option, &["amount_cents", "coin_amount", "first_bonus_gold", "first_bonus_coupons",
            "activity_bonus_coupons", "activity_bonus_gold", "break_ice_coupons", "coupon_bundle_amount",
            "deduction_card_amount", "first_text", "activity_text"][..]),
        (config, &["notice", "bonus_reason", "first_bonus_type", "activity_bonus_type"][..]),
    ] {
        for name in names {
            let (kind, value) = match source.get(*name) {
                Some(value @ Value::Number(_)) => ("number", value.clone()),
                Some(value @ Value::String(_)) => ("string", value.clone()),
                Some(value @ Value::Bool(_)) => ("boolean", value.clone()),
                Some(_) => return Err(SourceError::protocol("recharge_fingerprint_value_invalid")),
                None => ("nil", json!("")),
            };
            basis.push(json!([name, kind, value]));
        }
    }
    hash_json(&Value::Array(basis))
}

fn hash_json(value: &Value) -> Result<String> {
    let bytes = serde_json::to_vec(value).map_err(|_| SourceError::protocol("recharge_fingerprint_invalid"))?;
    Ok(format!("{:x}", Sha256::digest(&bytes)))
}

fn order_id(value: &Value) -> Option<String> {
    let value = if let Some(value) = value.as_str() { value.to_owned() }
        else if let Some(value) = safe_integer(value) { value.to_string() }
        else { return None; };
    canonical_order_id(&value).then_some(value)
}

fn canonical_order_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128 && !value.starts_with('0') && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn trusted_code_url(value: &str) -> bool {
    if value.is_empty() || value.len() > 8192 || value.chars().any(|character| character.is_whitespace()
        || character.is_control() || character == '\\') { return false; }
    let Some(rest) = value.strip_prefix("https://") else { return false; };
    let Some((host, _)) = rest.split_once('/') else { return false; };
    if !host.eq_ignore_ascii_case("pay.bilibili.com") { return false; }
    let Ok(url) = url::Url::parse(value) else { return false; };
    url.scheme() == "https" && url.host_str() == Some("pay.bilibili.com")
        && url.username().is_empty() && url.password().is_none() && url.port().is_none()
}

fn normalize_order(value: &Value, amount_cents: u64, timestamp: i64) -> Result<RechargeOrder> {
    let parameters = value.get("pay_params").and_then(Value::as_str)
        .filter(|value| value.len() <= MAX_PAY_PARAMETERS).ok_or_else(|| unknown_order(None))?;
    let parameters = decode_recharge_envelope(parameters.as_bytes()).map_err(|_| unknown_order(None))?;
    let order_id = parameters.get("orderId").and_then(order_id).ok_or_else(|| unknown_order(None))?;
    let code_url = parameters.get("codeUrl").and_then(Value::as_str)
        .filter(|value| trusted_code_url(value)).ok_or_else(|| unknown_order(Some(order_id.clone())))?;
    Ok(RechargeOrder { order_id, code_url: code_url.into(), amount_cents, observed_at: timestamp })
}

fn unknown_order(order_id: Option<String>) -> SourceError {
    SourceError { transmitted: Some(true), definitive: false, reconciliation_id: order_id,
        ..SourceError::new(ErrorKind::PurchaseUnknown, "recharge_outcome_unknown") }
}

fn normalize_history(value: &Value, page: u32, page_size: u32, year: i32, month: u8) -> Result<Value> {
    let source = value.as_array().filter(|values| values.len() <= 100)
        .ok_or_else(|| SourceError::protocol("recharge_history_invalid"))?;
    let mut records = Vec::new(); let mut seen = BTreeSet::new();
    for raw in source {
        let raw = require_object(raw, "recharge_history_record_invalid")?;
        let identifier = raw.get("id").and_then(order_id)
            .ok_or_else(|| SourceError::protocol("recharge_history_identity_invalid"))?;
        if !seen.insert(identifier.clone()) { return Err(SourceError::protocol("recharge_history_identity_duplicated")); }
        let mut record = Map::from_iter([("order_id".into(), json!(identifier))]);
        for (raw_name, normalized, limit) in [("ctime", "created_at", 128), ("pay_channel", "pay_channel", 64),
            ("pay_channel_name", "pay_channel_name", 128), ("activity", "activity", 512)] {
            if let Some(value) = raw.get(raw_name).and_then(|value| text(value, limit)) {
                record.insert(normalized.into(), json!(value));
            }
        }
        for (raw_name, normalized) in [("product_amount", "product_amount"),
            ("extra_product_amount", "extra_product_amount"), ("free_gold", "deduction_card_amount")] {
            if let Some(value) = optional_integer(raw, raw_name)? { record.insert(normalized.into(), value); }
        }
        if let Some(amount) = raw.get("pay_amount") {
            if !valid_amount(amount.as_f64()) { return Err(SourceError::protocol("recharge_history_amount_invalid")); }
            // The inspected history model does not establish a yuan or cent wire unit.
            record.insert("raw_pay_amount".into(), amount.clone());
        }
        records.push(Value::Object(record));
    }
    Ok(json!({"records": records, "page_num": page, "page_size": page_size,
        "order_year": year, "order_month": month}))
}

/// Protect identifier number tokens before generic JSON parsing can round them.
/// This lexer also rejects duplicate identity keys, including escaped spellings.
pub(super) fn decode_recharge_envelope(source: &[u8]) -> Result<Value> {
    if source.len() > MAX_RECHARGE_RESPONSE { return Err(SourceError::protocol("recharge_response_size_limit")); }
    std::str::from_utf8(source).map_err(|_| SourceError::protocol("recharge_response_json_invalid"))?;
    struct Context { object: bool, identifiers: HashSet<String> }
    let mut stack: Vec<Context> = Vec::new();
    let mut output = Vec::with_capacity(source.len());
    let mut pending: Option<String> = None;
    let mut offset = 0;
    while offset < source.len() {
        match source[offset] {
            b'"' => {
                let start = offset; offset += 1; let mut complete = false;
                while offset < source.len() {
                    match source[offset] {
                        b'\\' => { offset = offset.saturating_add(2); },
                        b'"' => { offset += 1; complete = true; break; },
                        _ => offset += 1,
                    }
                }
                if !complete || offset > source.len() { return Err(SourceError::protocol("recharge_response_json_invalid")); }
                output.extend_from_slice(&source[start..offset]);
                let mut next = offset;
                while next < source.len() && source[next].is_ascii_whitespace() { next += 1; }
                if source.get(next) == Some(&b':') {
                    let name: String = serde_json::from_slice(&source[start..offset])
                        .map_err(|_| SourceError::protocol("recharge_response_json_invalid"))?;
                    if identifier_field(&name) {
                        if let Some(context) = stack.last_mut().filter(|context| context.object) {
                            if !context.identifiers.insert(name.clone()) {
                                return Err(SourceError::protocol("recharge_identifier_key_duplicated"));
                            }
                        }
                    }
                    pending = Some(name);
                } else { pending = None; }
            }
            character @ (b'{' | b'[') => {
                if stack.len() >= 32 { return Err(SourceError::protocol("recharge_response_depth_limit")); }
                stack.push(Context { object: character == b'{', identifiers: HashSet::new() });
                pending = None; output.push(character); offset += 1;
            }
            character @ (b'}' | b']') => {
                stack.pop(); pending = None; output.push(character); offset += 1;
            }
            b'-' | b'0'..=b'9' => {
                let start = offset;
                while offset < source.len() && matches!(source[offset], b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-') { offset += 1; }
                let token = &source[start..offset];
                if pending.as_deref().is_some_and(identifier_field) {
                    let identifier = std::str::from_utf8(token).map_err(|_| SourceError::protocol("recharge_identifier_invalid"))?;
                    if !canonical_order_id(identifier) { return Err(SourceError::protocol("recharge_identifier_invalid")); }
                    output.push(b'"'); output.extend_from_slice(token); output.push(b'"');
                } else { output.extend_from_slice(token); }
                pending = None;
            }
            character => {
                output.push(character); offset += 1;
                if character != b':' && !character.is_ascii_whitespace() { pending = None; }
            }
        }
    }
    serde_json::from_slice(&output).map_err(|_| SourceError::protocol("recharge_response_json_invalid"))
}

fn identifier_field(name: &str) -> bool { matches!(name, "id" | "order_id" | "orderId") }

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::crypto::NativeCrypto;
    use crate::source::transport::{HttpRequest, HttpResponse, ResponseFuture, Transport};
    use crate::source::AuthSession;
    use std::{collections::VecDeque, path::Path, sync::{Arc, Mutex, atomic::{AtomicUsize, Ordering}}};

    struct Step { endpoint: &'static str, body: Value, flag: bool, result: Result<HttpResponse> }
    struct FixtureTransport { steps: Mutex<VecDeque<Step>>, calls: AtomicUsize }

    impl Transport for FixtureTransport {
        fn send(&self, request: HttpRequest) -> ResponseFuture<'_> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let step = self.steps.lock().unwrap().pop_front().expect("Unexpected fixture request");
            Box::pin(async move {
                let url = url::Url::parse(&request.url).unwrap();
                assert_eq!(request.method, "POST");
                assert!(url.path().ends_with(step.endpoint));
                assert_eq!(url.query().unwrap_or_default().contains("getEpisodeDiscounts"), step.flag);
                let body: Value = serde_json::from_slice(&request.body).unwrap();
                assert_eq!(body, step.body);
                step.result
            })
        }
    }

    fn response(value: Value) -> HttpResponse {
        HttpResponse { status: 200, headers: Vec::new(), body: serde_json::to_vec(&value).unwrap() }
    }

    fn step(endpoint: &'static str, body: Value, data: Value) -> Step {
        Step { endpoint, body, flag: false, result: Ok(response(json!({"code": 0, "data": data}))) }
    }

    fn fixture(steps: Vec<Step>) -> (BiliClient, Arc<FixtureTransport>) {
        let transport = Arc::new(FixtureTransport { steps: Mutex::new(steps.into()), calls: AtomicUsize::new(0) });
        let session = AuthSession::from_cookie_text("SESSDATA=fixture-session; bili_jct=fixture-csrf").unwrap();
        let client = BiliClient::with_adapters(Some(session), Path::new("fixture-only-state"), transport.clone(),
            Arc::new(NativeCrypto::new(Path::new("fixture-only-assets"))));
        (client, transport)
    }

    fn purchase(scope: PurchaseScope, method: PurchaseMethod) -> PurchaseRequest {
        PurchaseRequest { scope, method, pay_amount: None, coupon_id: None, coupon_ids: Vec::new(),
            free_gold_card_id: None, free_gold_amount: None }
    }

    fn configuration() -> Value {
        json!({"pay_amount_ranges": [
            {"pay_amount": "5.00", "gold_amount": 500, "first_bonus_amount": 5, "first_txt": "First purchase"},
            {"pay_amount": 10, "gold_amount": 1000},
        ], "show_text": "Observed fixture terms", "first_pay_send_type": 0})
    }

    #[test]
    fn quote_scopes_preserve_wire_meaning_and_omit_chapter_ordinals() {
        let (body, options) = quote_request("7", QuoteScope::Basic).unwrap();
        assert_eq!(body, json!({"ep_id": 7}));
        assert!(options.flag.is_none());
        let (body, options) = quote_request("7", QuoteScope::Single { order: 2 }).unwrap();
        assert_eq!(body, json!({"ep_id": 7, "buy_type": 1, "order": 2}));
        assert_eq!(options.flag, Some("getEpisodeDiscounts"));
        let (body, _) = quote_request("7", QuoteScope::Batch { limit: 0, order: 1 }).unwrap();
        assert_eq!(body, json!({"ep_id": 7, "buy_type": 2, "batch_limit": 0, "order": 1}));
        assert!(body.get("start_ord").is_none());
        assert!(quote_request("7", QuoteScope::Single { order: 0 }).is_err());
        assert!(quote_request("7", QuoteScope::Batch { limit: u32::MAX, order: 1 }).is_err());
    }

    #[tokio::test]
    async fn quote_checks_response_identity_and_preserves_absence_and_zero() {
        let mut expected = step("GetEpisodeBuyInfo", json!({"ep_id": 7, "buy_type": 1, "order": 2}),
            json!({"comic_id": 9, "pay_gold": 0, "batch_buy": [], "cookie": "fixture-private"}));
        expected.flag = true;
        let (client, transport) = fixture(vec![expected]);
        let quote = client.purchase_info("7", QuoteScope::Single { order: 2 }).await.unwrap();
        assert_eq!(quote["ep_id"], "7");
        assert_eq!(quote["comic_id"], "9");
        assert_eq!(quote["pay_gold"], 0);
        assert!(quote.get("ep_original_gold").is_none());
        assert!(quote.get("cookie").is_none());
        assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
        assert!(normalize_quote(json!({"ep_id": 8}), "7").is_err());
        assert!(normalize_quote(json!({"ep_id": null}), "7").is_err());
        assert!(normalize_quote(json!({"comic_id": "invalid"}), "7").is_err());
    }

    #[test]
    fn purchase_payload_retains_fractional_and_explicit_remaining_scopes() {
        let mut request = purchase(PurchaseScope::OrdinalRange { comic_id: "9".into(), start_order: 12.5, limit: 0 }, PurchaseMethod::Currency);
        request.pay_amount = Some(25.0);
        assert_eq!(purchase_payload(&request).unwrap(), json!({"comic_id": 9, "with_ord_scope": true,
            "start_ord": 12.5, "limit": 0, "buy_method": 3, "pay_amount": 25.0}));
        request.method = PurchaseMethod::Coupon;
        request.coupon_ids = vec!["11".into()];
        assert!(purchase_payload(&request).is_err());
    }

    #[test]
    fn purchase_coupon_payload_never_defaults_a_missing_price_to_zero() {
        let mut request = purchase(PurchaseScope::Single { episode_id: "7".into(), comic_id: None }, PurchaseMethod::Coupon);
        assert!(purchase_payload(&request).is_err());
        request.coupon_ids = vec!["11".into(), "12".into()];
        let body = purchase_payload(&request).unwrap();
        assert_eq!(body, json!({"ep_id": 7, "buy_method": 2, "coupon_ids": ["11", "12"]}));
        assert!(body.get("pay_amount").is_none());
        request.coupon_ids.push("11".into());
        assert!(purchase_payload(&request).is_err());
        request.coupon_ids = vec!["11".into()];
        request.method = PurchaseMethod::Currency;
        assert!(purchase_payload(&request).is_err());
    }

    #[tokio::test]
    async fn purchase_unknown_response_is_not_replayed_or_treated_as_success() {
        let mut request = purchase(PurchaseScope::Single { episode_id: "7".into(), comic_id: None }, PurchaseMethod::Currency);
        request.pay_amount = Some(2.0);
        let expected = Step { endpoint: "BuyEpisode", body: purchase_payload(&request).unwrap(), flag: false,
            result: Ok(HttpResponse { status: 200, headers: Vec::new(), body: b"invalid-json".to_vec() }) };
        let (client, transport) = fixture(vec![expected]);
        let error = client.buy_episode(&request).await.unwrap_err();
        assert_eq!(error.kind, ErrorKind::PurchaseUnknown);
        assert!(!error.definitive);
        assert!(!error.retryable);
        assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
        assert!(transport.steps.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn purchase_definitive_rejection_stays_distinct_from_unknown() {
        let request = purchase(PurchaseScope::Single { episode_id: "7".into(), comic_id: None }, PurchaseMethod::Currency);
        let expected = Step { endpoint: "BuyEpisode", body: purchase_payload(&request).unwrap(), flag: false,
            result: Ok(response(json!({"code": 2, "data": {}}))) };
        let (client, transport) = fixture(vec![expected]);
        let error = client.buy_episode(&request).await.unwrap_err();
        assert_eq!(error.kind, ErrorKind::PurchaseRejected);
        assert_eq!(error.business_code, Some(2));
        assert!(error.definitive);
        assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn stale_or_unverified_payment_accounts_cannot_reach_transport() {
        let request = purchase(PurchaseScope::Single { episode_id: "7".into(), comic_id: None }, PurchaseMethod::Currency);
        let (client, transport) = fixture(Vec::new());
        let error = client.buy_episode_for_account(&request, "bili_1").await.unwrap_err();
        assert_eq!(error.kind, ErrorKind::AccountMismatch);
        assert_eq!(error.transmitted, Some(false));
        let expected = client.session_generation();
        client.replace_session(Some(AuthSession::default())).unwrap();
        let error = client.post("comic.v1.Comic", "BuyEpisode", purchase_payload(&request).unwrap(),
            PostOptions { expected_epoch: Some(expected), ..authenticated() }).await.unwrap_err();
        assert_eq!(error.transmitted, Some(false));
        assert!(error.definitive);
        assert_eq!(transport.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn recharge_configuration_binds_terms_without_binding_retrieval_time() {
        let original = configuration();
        let first = normalize_config(&original, 1).unwrap();
        let later = normalize_config(&original, 2).unwrap();
        assert_eq!(first.options[0].amount_cents, 500);
        assert_eq!(first.options[0].fingerprint, later.options[0].fingerprint);
        let mut changed = original.clone(); changed["pay_amount_ranges"][0]["first_bonus_amount"] = json!(6);
        assert_ne!(first.options[0].fingerprint, normalize_config(&changed, 1).unwrap().options[0].fingerprint);
        changed = original.clone(); changed["show_text"] = json!("Changed benefit terms");
        assert_ne!(first.options[0].fingerprint, normalize_config(&changed, 1).unwrap().options[0].fingerprint);
        changed = original.clone(); changed["pay_amount_ranges"][1]["pay_amount"] = json!("5.00");
        assert!(normalize_config(&changed, 1).is_err());
        assert!(normalize_config(&json!({"pay_amount_ranges": []}), 1).unwrap().options.is_empty());
    }

    #[test]
    fn recharge_amounts_use_exact_cents_and_bounded_official_denominations() {
        assert_eq!(cents_from_yuan(&json!(" 5.2 ")), Some(520));
        assert_eq!(cents_from_yuan(&json!(0.29)), Some(29));
        assert_eq!(cents_from_yuan(&json!("21474836.47")), Some(MAX_AMOUNT_CENTS));
        for value in [json!("5."), json!("5.001"), json!(5.001), json!(0), json!("-1"), json!("1e2"), json!("21474836.48")] {
            assert!(cents_from_yuan(&value).is_none());
        }
    }

    #[test]
    fn recharge_lexer_preserves_large_identifiers_and_rejects_duplicate_or_rounded_shapes() {
        let parsed = decode_recharge_envelope(br#"{"data":[{"id":90071992547409931234,"text":"id:123"}]}"#).unwrap();
        assert_eq!(parsed["data"][0]["id"], "90071992547409931234");
        assert_eq!(parsed["data"][0]["text"], "id:123");
        assert!(decode_recharge_envelope(br#"{"orderId":1,"order\u0049d":2}"#).is_err());
        for input in [br#"{"id":1e3}"#.as_slice(), br#"{"id":01}"#, br#"{"id":-1}"#, br#"{"id":1.0}"#] {
            assert!(decode_recharge_envelope(input).is_err());
        }
        assert!(order_id(&json!(9_007_199_254_740_992_u64)).is_none());
    }

    #[tokio::test]
    async fn recharge_creation_rechecks_configuration_and_transmits_integer_cents_once() {
        let config = configuration(); let confirmed = normalize_config(&config, 0).unwrap().options[0].fingerprint.clone();
        let parameters = "{\"orderId\":90071992547409931234,\"codeUrl\":\"https://pay.bilibili.com/fixture?payToken=SYNTHETIC_ONLY\"}";
        let (client, transport) = fixture(vec![
            step("GetPayConfig", json!({}), config),
            step("CreateOrder", json!({"pay_type": "qr", "pay_amount": 500}), json!({"pay_params": parameters})),
        ]);
        let order = client.create_recharge_order(500, Some(&confirmed)).await.unwrap();
        assert_eq!(order.order_id, "90071992547409931234");
        assert_eq!(order.amount_cents, 500);
        assert!(!format!("{order:?}").contains("SYNTHETIC_ONLY"));
        assert_eq!(transport.calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn changed_recharge_terms_prevent_order_creation() {
        let original = configuration(); let confirmed = normalize_config(&original, 0).unwrap().options[0].fingerprint.clone();
        let mut changed = original; changed["pay_amount_ranges"][0]["first_bonus_amount"] = json!(99);
        let (client, transport) = fixture(vec![step("GetPayConfig", json!({}), changed)]);
        let error = client.create_recharge_order(500, Some(&confirmed)).await.unwrap_err();
        assert_eq!(error.code, "recharge_config_changed");
        assert_eq!(error.transmitted, Some(false));
        assert!(error.definitive);
        assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn uncertain_recharge_retains_only_an_exact_known_reconciliation_identity() {
        let invalid = json!({"pay_params": "{\"orderId\":90071992547409931234,\"codeUrl\":\"https://example.invalid/fixture\"}"});
        let error = normalize_order(&invalid, 500, 0).unwrap_err();
        assert_eq!(error.kind, ErrorKind::PurchaseUnknown);
        assert_eq!(error.reconciliation_id.as_deref(), Some("90071992547409931234"));
        assert!(!format!("{error:?}").contains("90071992547409931234"));
        for url in ["http://pay.bilibili.com/fixture", "https://pay.bilibili.com.evil/fixture",
            "https://pay.bilibili.com:443/fixture", "https://user@pay.bilibili.com/fixture",
            "https://pay.bilibili.com/with space", "https://pay.bilibili.com/with\\slash"] {
            assert!(!trusted_code_url(url));
        }
    }

    #[tokio::test]
    async fn recharge_history_preserves_identity_and_does_not_infer_cash_units() {
        let bytes = br#"{"code":0,"data":[{"id":90071992547409931234,"pay_amount":500,"product_amount":500,"free_gold":1}]}"#.to_vec();
        let expected = Step { endpoint: "GetPayOrders", body: json!({"page_num": 2, "page_size": 20,
            "order_year": 2026, "order_month": 0}), flag: false,
            result: Ok(HttpResponse { status: 200, headers: Vec::new(), body: bytes }) };
        let (client, transport) = fixture(vec![expected]);
        let history = client.recharge_history_for(2, 20, 2026, 0).await.unwrap();
        assert_eq!(history["records"][0]["order_id"], "90071992547409931234");
        assert_eq!(history["records"][0]["raw_pay_amount"], 500);
        assert_eq!(history["records"][0]["deduction_card_amount"], 1);
        assert!(history["records"][0].get("amount_cents").is_none());
        assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
        assert!(normalize_history(&json!([{ "id": "1" }, { "id": "1" }]), 1, 20, 2026, 0).is_err());
    }
}
