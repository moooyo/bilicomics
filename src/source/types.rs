use super::crypto::ImageKey;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CategoryOption { pub id: String, pub name: String }

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CategoryOrder { pub id: i32, pub name: String }

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CategoryMetadata {
    pub styles: Vec<CategoryOption>,
    pub orders: Vec<CategoryOrder>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ComicPage {
    pub comics: Vec<crate::domain::Comic>,
    pub page: u32,
    pub has_more: bool,
}

/// A token is temporary acquisition material; Debug omits URLs and opaque fields.
#[derive(Clone)]
pub struct ImageToken {
    pub(crate) raw: Value,
    pub index: usize,
}

impl std::fmt::Debug for ImageToken {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("ImageToken").field("index", &self.index).finish_non_exhaustive()
    }
}

#[derive(Clone)]
pub struct AcquisitionBatch {
    pub tokens: Vec<ImageToken>,
    pub(crate) key: Arc<ImageKey>,
    pub(crate) account_epoch: u64,
}

impl std::fmt::Debug for AcquisitionBatch {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("AcquisitionBatch").field("token_count", &self.tokens.len()).finish_non_exhaustive()
    }
}

#[derive(Clone)]
pub struct VerifiedImage {
    pub bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub format: String,
    pub sha256: String,
    pub verification: &'static str,
}

impl std::fmt::Debug for VerifiedImage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("VerifiedImage").field("width", &self.width).field("height", &self.height)
            .field("format", &self.format).field("bytes", &self.bytes.len())
            .field("verification", &self.verification).finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum QuoteScope {
    Basic,
    Single { order: u8 },
    Batch { limit: u32, order: u8 },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PurchaseScope {
    Single { episode_id: String, comic_id: Option<String> },
    OrdinalRange { comic_id: String, start_order: f64, limit: u32 },
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PurchaseMethod { Currency, Coupon }

/// Submission is explicit. The application owns quote confirmation and the durable intent journal.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PurchaseRequest {
    pub scope: PurchaseScope,
    pub method: PurchaseMethod,
    pub pay_amount: Option<f64>,
    pub coupon_id: Option<String>,
    pub coupon_ids: Vec<String>,
    pub free_gold_card_id: Option<String>,
    pub free_gold_amount: Option<f64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RechargeOption {
    pub amount_cents: u64,
    pub coin_amount: u64,
    pub fingerprint: String,
    pub extra: Value,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RechargeConfig {
    pub options: Vec<RechargeOption>,
    pub observed_at: i64,
    pub extra: Value,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RechargeOrder {
    pub order_id: String,
    pub code_url: String,
    pub amount_cents: u64,
    pub observed_at: i64,
}

impl Default for RechargeOrder {
    fn default() -> Self { Self { order_id: String::new(), code_url: String::new(), amount_cents: 0, observed_at: 0 } }
}

impl std::fmt::Debug for RechargeOrder {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("RechargeOrder").field("amount_cents", &self.amount_cents)
            .field("observed_at", &self.observed_at).finish_non_exhaustive()
    }
}
