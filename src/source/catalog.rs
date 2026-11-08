use super::{BiliClient, CategoryMetadata, CategoryOption, CategoryOrder, ComicPage, Result, SourceError};
use super::client::{MANGA_ORIGIN, PostOptions, USER_AGENT, now_seconds, numeric_id};
use super::normalize;
use super::transport::{HttpRequest, check_status};
use crate::domain::{Comic, ComicDetail};
use serde_json::{Value, json};
use std::collections::BTreeSet;

fn pagination(page: u32, size: u32) -> Result<(u32, u32)> {
    if page == 0 || !(1..=100).contains(&size) { return Err(SourceError::invalid("invalid_pagination")); }
    Ok((page, size))
}

impl BiliClient {
    pub async fn list_favorites(&self, page: u32, page_size: u32) -> Result<Vec<Comic>> {
        let (page, size) = pagination(page, page_size)?;
        let data = self.post("bookshelf.v1.Bookshelf", "ListFavorite", json!({"page_num":page,"page_size":size,
            "order":1,"wait_free":0,"time_limit_free":0,"type":0,"from":"web","source":"web"}),
            PostOptions { require_session: true, ..Default::default() }).await?;
        let mut comics = normalize::comic_list(&data)?;
        for comic in &mut comics { comic.favorite = true; }
        Ok(comics)
    }

    pub async fn list_history(&self, page: u32, page_size: u32) -> Result<Vec<Comic>> {
        let (page, size) = pagination(page, page_size)?;
        let data = self.post("bookshelf.v1.Bookshelf", "ListHistory", json!({"page_num":page,"page_size":size,"type":0}),
            PostOptions { require_session: true, ..Default::default() }).await?;
        normalize::comic_list(&data)
    }

    pub async fn search(&self, query: &str, page: u32, page_size: u32) -> Result<Vec<Comic>> {
        let (page, size) = pagination(page, page_size)?;
        if query.trim().is_empty() || query.len() > 300 { return Err(SourceError::invalid("invalid_search_query")); }
        let data = self.post("comic.v1.Comic", "Search", json!({"key_word":query,"page_num":page,"page_size":size}), PostOptions::default()).await?;
        normalize::comic_list(&data)
    }

    pub async fn comic_detail(&self, comic_id: &str) -> Result<ComicDetail> {
        let data = self.post("comic.v1.Comic", "ComicDetail", json!({"comic_id":numeric_id(comic_id)?,"m2":self.m2()}), PostOptions::default()).await?;
        let mut comic = normalize::comic(&data);
        if comic.id.is_empty() { comic.id = comic_id.into(); }
        if comic.id != comic_id { return Err(SourceError::protocol("comic_identity_mismatch")); }
        let rows = data.get("ep_list").or_else(|| data.get("episodes")).and_then(Value::as_array)
            .ok_or_else(|| SourceError::protocol("episode_list_missing"))?;
        // A malformed row must invalidate the response rather than publish a
        // shortened directory as a successful replacement for the cached one.
        let timestamp = now_seconds();
        let mut episodes = Vec::with_capacity(rows.len());
        let mut ids = BTreeSet::new();
        for raw in rows {
            if !raw.is_object() { return Err(SourceError::protocol("episode_list_invalid")); }
            let episode = normalize::episode(raw, comic_id, timestamp);
            numeric_id(&episode.id).map_err(|_| SourceError::protocol("episode_identity_invalid"))?;
            // Omitted parent IDs inherit the requested comic; an explicitly
            // supplied parent identity must be complete and match that comic.
            let supplied_comic = raw.get("comic_id").map(|_| normalize::text(raw, &["comic_id"]));
            if episode.comic_id != comic_id || supplied_comic.as_deref().is_some_and(|id| id != comic_id)
                || !ids.insert(episode.id.clone())
            { return Err(SourceError::protocol("episode_identity_invalid")); }
            episodes.push(episode);
        }
        episodes.sort_by(|left, right| left.order.total_cmp(&right.order).then_with(|| left.id.cmp(&right.id)));
        comic.chapter_count = episodes.len().try_into().unwrap_or(u32::MAX);
        if let Some(latest) = episodes.last() {
            if comic.latest_episode_id.is_none() { comic.latest_episode_id = Some(latest.id.clone()); }
            if comic.latest_episode_title.is_empty() { comic.latest_episode_title = if latest.short_title.is_empty() { latest.title.clone() } else { latest.short_title.clone() }; }
            if comic.latest_order == 0.0 { comic.latest_order = latest.order; }
        }
        Ok(ComicDetail { comic, episodes, extra: normalize::safe_extra(&data) })
    }

    pub async fn set_favorite(&self, comic_id: &str, favorite: bool) -> Result<()> {
        numeric_id(comic_id)?;
        self.post("bookshelf.v1.Bookshelf", if favorite { "AddFavorite" } else { "DeleteFavorite" },
            json!({"comic_ids":comic_id}), PostOptions { require_session: true, ..Default::default() }).await?;
        Ok(())
    }

    pub async fn add_history(&self, comic_id: &str, episode_id: &str) -> Result<()> {
        self.post("bookshelf.v1.Bookshelf", "AddHistory", json!({"comic_id":numeric_id(comic_id)?,"ep_id":numeric_id(episode_id)?}),
            PostOptions { require_session: true, ..Default::default() }).await?;
        Ok(())
    }

    pub async fn recommendations(&self) -> Result<Vec<Comic>> {
        let response = self.send(HttpRequest { method: "GET".into(), url: format!("{MANGA_ORIGIN}/index.pageContext.json"),
            headers: vec![("user-agent".into(), USER_AGENT.into()), ("accept".into(),"application/json".into()),
                ("referer".into(),format!("{MANGA_ORIGIN}/"))], body: Vec::new(), max_bytes: 4 * 1024 * 1024 }).await?;
        check_status(&response)?;
        let context: Value = serde_json::from_slice(&response.body).map_err(|_| SourceError::protocol("recommendations_json_invalid"))?;
        let data = context.get("data").ok_or_else(|| SourceError::protocol("recommendations_context_missing"))?;
        let first = data.pointer("/recommendation/comics").and_then(Value::as_array)
            .ok_or_else(|| SourceError::protocol("recommendations_missing"))?;
        let mut comics: Vec<Comic> = first.iter().map(normalize::comic).collect();
        if context.get("pageId").and_then(Value::as_str) != Some("/pages/index") {
            return Err(SourceError::protocol("recommendations_page_mismatch"));
        }
        for section in ["hotSeller", "internetHot", "completedComic"] {
            if let Some(group) = data.get(section) {
                for name in ["firstGroup", "secondGroup"] {
                    if let Some(items) = group.get(name).and_then(Value::as_array) { comics.extend(items.iter().map(normalize::comic)); }
                }
            }
        }
        let mut seen = BTreeSet::new();
        comics.retain(|comic| !comic.id.is_empty() && !comic.title.is_empty() && seen.insert(comic.id.clone()));
        comics.truncate(96);
        Ok(comics)
    }

    pub async fn categories(&self) -> Result<CategoryMetadata> {
        let data = self.post("comic.v1.Comic", "AllLabel", json!({}), PostOptions { anonymous: true, ..Default::default() }).await?;
        let styles = data.get("styles").and_then(Value::as_array).ok_or_else(|| SourceError::protocol("category_styles_missing"))?;
        let orders = data.get("orders").and_then(Value::as_array).ok_or_else(|| SourceError::protocol("category_orders_missing"))?;
        let mut seen = BTreeSet::new();
        let styles: Vec<_> = styles.iter().filter_map(|raw| {
            let id = normalize::text(raw, &["id"]); let name = normalize::text(raw, &["name"]);
            (numeric_id(&id).is_ok() && !name.is_empty() && seen.insert(id.clone())).then_some(CategoryOption { id, name })
        }).take(128).collect();
        let orders: Vec<_> = orders.iter().filter_map(|raw| {
            let id = raw.get("id").and_then(Value::as_i64)?;
            let name = normalize::text(raw, &["name"]);
            (matches!(id, 0 | 1 | 3) && !name.is_empty()).then_some(CategoryOrder { id: id as i32, name })
        }).collect();
        if styles.is_empty() || !orders.iter().any(|order| order.id == 0) { return Err(SourceError::protocol("category_metadata_invalid")); }
        Ok(CategoryMetadata { styles, orders })
    }

    pub async fn category_page(&self, category_id: &str, order: i32, page: u32) -> Result<ComicPage> {
        let (page, size) = pagination(page, 18)?;
        if page > 5 || !matches!(order, 0 | 1 | 3) { return Err(SourceError::invalid("category_query_invalid")); }
        let buvid = self.guest_device_context().await?;
        let data = self.post("comic.v1.Comic", "ClassPage", json!({"style_id":numeric_id(category_id)?,"area_id":-1,
            "is_finish":-1,"order":order,"special_tag":0,"page_num":page,"page_size":size,"is_free":-1,"m2":self.m2()}),
            PostOptions { guest_device: Some(buvid), ..Default::default() }).await?;
        let comics = normalize::comic_list(&data)?;
        let has_more = comics.len() == size as usize;
        Ok(ComicPage { comics, page, has_more })
    }

    pub(super) fn m2(&self) -> String {
        format!("error:BiliComics local reader has no browser fingerprint environment_{}", super::client::now_millis())
    }

    async fn guest_device_context(&self) -> Result<String> {
        let response = self.send(HttpRequest { method: "GET".into(), url: format!("{MANGA_ORIGIN}/ductape/buvid"),
            headers: vec![("user-agent".into(),USER_AGENT.into()), ("accept".into(),"application/json".into()),
                ("referer".into(),format!("{MANGA_ORIGIN}/"))], body: Vec::new(), max_bytes: 65536 }).await?;
        check_status(&response)?;
        let selected: Vec<_> = super::session::set_cookie_records(&response.headers)?.into_iter().filter(|record|
            record.split(';').next().and_then(|pair| pair.trim().split_once('='))
                .is_some_and(|(name, _)| name.trim() == "buvid3"))
            .map(|record| ("set-cookie".into(), record)).collect();
        let mut scratch = super::AuthSession::default();
        scratch.apply_set_cookie_for_path(&selected, "manga.bilibili.com", "/ductape/buvid", now_seconds())?;
        let device = scratch.cookie_value_for("manga.bilibili.com", "buvid3").filter(|value| value.len() <= 256 && value.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte,b'_'|b'-')))
            .ok_or_else(|| SourceError::protocol("guest_device_context_missing"))?;
        Ok(device.into())
    }
}
