use super::{Result, SourceError};
use crate::domain::{Comic, Episode, EpisodeAccess};
use serde_json::{Map, Value};

pub(super) fn safe_extra(value: &Value) -> Value { sanitize(value, 0) }

fn sanitize(value: &Value, depth: usize) -> Value {
    if depth > 64 { return Value::Null; }
    match value {
        Value::Array(values) => Value::Array(values.iter().map(|value| sanitize(value, depth + 1)).collect()),
        Value::Object(values) => Value::Object(values.iter().filter_map(|(name, value)| {
            let private = matches!(name.to_ascii_lowercase().as_str(), "cookie" | "cookies" | "sessdata" | "bili_jct" |
                "token" | "private_key" | "privatekey" | "complete_url" | "ultra_sign" | "bytesdata" | "m1" | "m2" |
                "refresh_token" | "pending_refresh_token" | "refresh_csrf" | "qrcode_key" | "longtoken" | "code_url");
            if private || value.as_str().is_some_and(|text| text.contains("?token=") || text.contains("&token=")) { None }
            else { Some((name.clone(), sanitize(value, depth + 1))) }
        }).collect::<Map<_, _>>()),
        _ => value.clone(),
    }
}

pub(super) fn text(value: &Value, keys: &[&str]) -> String {
    for key in keys {
        match value.get(*key) {
            Some(Value::String(value)) if !value.is_empty() => return value.clone(),
            Some(Value::Number(value)) => return value.to_string(),
            _ => {},
        }
    }
    String::new()
}

pub(super) fn number(value: Option<&Value>) -> Option<f64> {
    let value = value?;
    value.as_f64().or_else(|| value.as_str().and_then(|value| value.parse::<f64>().ok())).filter(|value| value.is_finite())
}

pub(super) fn boolean(value: Option<&Value>) -> Option<bool> {
    match value? {
        Value::Bool(value) => Some(*value),
        Value::Number(value) => Some(value.as_i64() == Some(1)),
        Value::String(value) => Some(value == "1"),
        _ => None,
    }
}

fn names(value: &Value, keys: &[&str]) -> String {
    for key in keys {
        match value.get(*key) {
            Some(Value::String(name)) if !name.is_empty() => return name.clone(),
            Some(Value::Array(items)) => {
                let values: Vec<String> = items.iter().filter_map(|item| match item {
                    Value::String(name) => Some(name.clone()),
                    Value::Object(_) => { let name = text(item, &["name", "title"]); (!name.is_empty()).then_some(name) },
                    _ => None,
                }).collect();
                if !values.is_empty() { return values.join(" / "); }
            }
            _ => {},
        }
    }
    String::new()
}

fn timestamp(value: Option<&Value>) -> i64 {
    number(value).filter(|number| *number >= 0.0 && *number <= i64::MAX as f64).map(|number| number as i64).unwrap_or_default()
}

pub(super) fn comic(raw: &Value) -> Comic {
    let cover_url = text(raw, &["vertical_cover", "vcover", "hcover", "cover", "cover_url"]);
    let cover_url = if let Some(value) = cover_url.strip_prefix("http://") { format!("https://{value}") }
        else if cover_url.starts_with("//") { format!("https:{cover_url}") } else { cover_url };
    let last_episode_id = text(raw, &["last_ep_id", "last_episode_id", "ep_id"]);
    let latest_episode_id = text(raw, &["latest_ep_id", "latest_episode_id"]);
    Comic {
        id: text(raw, &["comic_id", "id", "season_id"]), title: text(raw, &["title", "comic_title"]),
        author: names(raw, &["author_name", "authors", "author_names"]), cover_url,
        description: text(raw, &["evaluate", "description", "comic_introduction", "introduction", "intro"]),
        genre: names(raw, &["styles", "style", "tags", "genre"]),
        latest_episode_id: (!latest_episode_id.is_empty()).then_some(latest_episode_id),
        latest_episode_title: text(raw, &["latest_ep_title", "latest_short_title", "last_short_title", "latest_episode_title"]),
        latest_order: number(raw.get("latest_ord").or_else(|| raw.get("latest_order"))).unwrap_or_default(),
        chapter_count: number(raw.get("total").or_else(|| raw.get("chapter_count"))).unwrap_or_default().clamp(0.0, u32::MAX as f64) as u32,
        progress: number(raw.get("progress").or_else(|| raw.get("read_progress"))).unwrap_or_default().clamp(0.0, 100.0) as u32,
        favorite: boolean(raw.get("is_fav").or_else(|| raw.get("is_favorite"))).unwrap_or_default(),
        finished: boolean(raw.get("is_finish").or_else(|| raw.get("finished"))).unwrap_or_default(),
        updated_at: timestamp(raw.get("last_update_time").or_else(|| raw.get("update_time"))),
        last_episode_id: (!last_episode_id.is_empty()).then_some(last_episode_id),
        last_read_at: timestamp(raw.get("last_read_time").or_else(|| raw.get("read_at"))), extra: safe_extra(raw),
    }
}

pub(super) fn comic_list(data: &Value) -> Result<Vec<Comic>> {
    let items = data.get("list").or_else(|| data.get("comics")).or_else(|| data.get("result")).unwrap_or(data);
    let items = items.as_array().ok_or_else(|| SourceError::protocol("comic_list_invalid"))?;
    Ok(items.iter().filter(|value| value.is_object()).map(comic).filter(|comic| !comic.id.is_empty()).collect())
}

pub(super) fn episode(raw: &Value, comic_id: &str, now: i64) -> Episode {
    let expiry_value = raw.get("unlock_expire_at").or_else(|| raw.get("expires_at"));
    let expiry = if expiry_value.and_then(Value::as_str) == Some("0000-00-00 00:00:00") { Some(0) }
        else { number(expiry_value).filter(|value| *value >= 0.0 && *value <= i64::MAX as f64).map(|value| value as i64) };
    let unknown_expiry = expiry_value.is_some_and(|value| !value.is_null() && value.as_str() != Some("")) && expiry.is_none();
    let unlock = number(raw.get("unlock_type"));
    let expired = expiry.is_some_and(|expiry| expiry > 0 && expiry <= now);
    let access = if boolean(raw.get("unavailable")) == Some(true) || boolean(raw.get("is_available")) == Some(false)
        || number(raw.get("status")) == Some(501.0) { EpisodeAccess::Unavailable }
        else if boolean(raw.get("is_locked")) == Some(true) { EpisodeAccess::Locked }
        else if matches!(unlock, Some(2.0) | Some(3.0)) {
            if unknown_expiry { EpisodeAccess::Unknown } else if expired { EpisodeAccess::Locked } else { EpisodeAccess::Temporary }
        } else if unlock == Some(1.0) || boolean(raw.get("is_purchased")) == Some(true) {
            if unknown_expiry { EpisodeAccess::Unknown } else if expired { EpisodeAccess::Locked }
            else if expiry.is_some_and(|expiry| expiry > 0) { EpisodeAccess::Temporary } else { EpisodeAccess::Owned }
        } else if number(raw.get("pay_mode")) == Some(0.0) { EpisodeAccess::Free }
        else if boolean(raw.get("is_in_free")) == Some(true) {
            // Temporary access cannot discard an explicitly supplied expiry we cannot parse.
            if unknown_expiry { EpisodeAccess::Unknown } else { EpisodeAccess::Temporary }
        } else { EpisodeAccess::Unknown };
    let supplied_comic = text(raw, &["comic_id"]);
    Episode { id: text(raw, &["id", "ep_id", "episode_id"]),
        comic_id: if supplied_comic.is_empty() { comic_id.into() } else { supplied_comic },
        title: text(raw, &["title", "ep_title"]), short_title: text(raw, &["short_title", "ep_short_title"]),
        order: number(raw.get("ord").or_else(|| raw.get("order"))).unwrap_or_default(), access, expires_at: expiry,
        pay_gold: number(raw.get("pay_gold")).filter(|value| *value >= 0.0),
        read: boolean(raw.get("is_read").or_else(|| raw.get("read"))).unwrap_or_default(), extra: safe_extra(raw) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn entitlement_does_not_promote_ambiguous_or_expired_rights() {
        assert_eq!(episode(&json!({"unlock_type":1,"unlock_expire_at":"0000-00-00 00:00:00"}), "1", 100).access, EpisodeAccess::Owned);
        assert_eq!(episode(&json!({"is_locked":1,"pay_mode":0,"unlock_type":1}), "1", 100).access, EpisodeAccess::Locked);
        assert_eq!(episode(&json!({"unlock_type":2,"unlock_expire_at":99}), "1", 100).access, EpisodeAccess::Locked);
        assert_eq!(episode(&json!({"unlock_type":1,"unlock_expire_at":"unknown"}), "1", 100).access, EpisodeAccess::Unknown);
        assert_eq!(episode(&json!({"is_locked":false,"pay_mode":1}), "1", 100).access, EpisodeAccess::Unknown);
    }

    #[test]
    fn temporary_fallback_keeps_explicit_unparsed_expiry_unreadable() {
        for field in ["unlock_expire_at", "expires_at"] {
            for expiry in [json!("unknown"), json!("2026-09-12 08:00:00"), json!(-1), json!(true), json!({}), json!([])] {
                let mut raw = json!({"pay_mode":1,"is_in_free":true});
                raw[field] = expiry.clone();
                let normalized = episode(&raw, "1", 100);
                assert_eq!(normalized.access, EpisodeAccess::Unknown, "{field}={expiry}");
                assert_eq!(normalized.expires_at, None, "{field}={expiry}");
                assert!(!normalized.readable_at(100), "{field}={expiry}");
            }
        }
    }

    #[test]
    fn temporary_fallback_preserves_known_expiry_and_absent_expiry_behavior() {
        let absent = episode(&json!({"pay_mode":1,"is_in_free":true}), "1", 100);
        assert_eq!(absent.access, EpisodeAccess::Temporary);
        assert!(absent.readable_at(100));
        for field in ["unlock_expire_at", "expires_at"] {
            for (expiry, expected, readable) in [
                (Value::Null, None, true), (json!(""), None, true),
                (json!(0), Some(0_i64), true), (json!("0"), Some(0), true),
                (json!("0000-00-00 00:00:00"), Some(0), true),
                (json!(99), Some(99), false), (json!(100), Some(100), false),
                (json!(101), Some(101), true), (json!("200"), Some(200), true),
            ] {
                let mut raw = json!({"pay_mode":1,"is_in_free":true});
                raw[field] = expiry.clone();
                let normalized = episode(&raw, "1", 100);
                assert_eq!(normalized.access, EpisodeAccess::Temporary, "{field}={expiry}");
                assert_eq!(normalized.expires_at, expected, "{field}={expiry}");
                assert_eq!(normalized.readable_at(100), readable, "{field}={expiry}");
            }
        }
    }

    #[test]
    fn explicit_free_pay_mode_keeps_precedence_over_the_temporary_fallback() {
        for field in ["unlock_expire_at", "expires_at"] {
            let mut raw = json!({"pay_mode":0,"is_in_free":true});
            raw[field] = json!("unknown");
            let normalized = episode(&raw, "1", 100);
            assert_eq!(normalized.access, EpisodeAccess::Free);
            assert!(normalized.readable_at(100));
        }
    }

    #[test]
    fn explicit_entitlement_modes_keep_unparsed_expiry_guards_before_fallbacks() {
        for field in ["unlock_expire_at", "expires_at"] {
            for unlock_type in [1, 2, 3] {
                let mut raw = json!({"unlock_type":unlock_type,"pay_mode":0,"is_in_free":true});
                raw[field] = json!("unknown");
                let normalized = episode(&raw, "1", 100);
                assert_eq!(normalized.access, EpisodeAccess::Unknown, "{field}, unlock_type={unlock_type}");
                assert!(!normalized.readable_at(100), "{field}, unlock_type={unlock_type}");
            }
        }
    }

    #[test]
    fn cached_extra_removes_nested_acquisition_and_account_material() {
        let value = safe_extra(&json!({"title":"visible", "nested":{"SESSDATA":"secret", "complete_url":"private"}, "url":"https://x/a?token=private"}));
        assert_eq!(value, json!({"title":"visible", "nested":{}}));
    }
}
