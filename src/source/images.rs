use super::{AcquisitionBatch, BiliClient, ErrorKind, ImageToken, Result, SourceError, VerifiedImage};
use super::client::{MANGA_ORIGIN, PostOptions, USER_AGENT, numeric_id};
use super::normalize;
use super::transport::{HttpRequest, check_status};
use crate::domain::{ImageIndex, PageDescriptor};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{path::Path, sync::{Arc, atomic::Ordering}};
use url::Url;

const MAX_IMAGE_BYTES: usize = crate::image_integrity::MAX_ENCODED_BYTES as usize;
const MAX_ENCRYPTED_BYTES: usize = 16 * 1024 * 1024;
const MAX_COVER_BYTES: usize = 2 * 1024 * 1024;

impl BiliClient {
    pub async fn image_index(&self, episode_id: &str) -> Result<ImageIndex> {
        let data = self.post("comic.v1.Comic", "GetImageIndex", json!({"ep_id":numeric_id(episode_id)?,"m2":self.m2()}),
            PostOptions { require_session: true, ..Default::default() }).await?;
        let raw_images = data.get("images").and_then(Value::as_array).filter(|images| !images.is_empty() && images.len() <= 10000)
            .ok_or_else(|| SourceError::protocol("ordered_image_index_missing"))?;
        let mut images = Vec::with_capacity(raw_images.len()); let mut paths = Vec::with_capacity(raw_images.len());
        let mut revision = Vec::with_capacity(raw_images.len());
        for (index, raw) in raw_images.iter().enumerate() {
            let path = raw.get("path").and_then(Value::as_str).filter(|path| !path.is_empty() && path.len() <= 8192
                && !path.chars().any(char::is_control)).ok_or_else(|| SourceError::protocol("image_index_path_invalid"))?;
            let width = geometry(raw.get("x").or_else(|| raw.get("width")))?;
            let height = geometry(raw.get("y").or_else(|| raw.get("height")))?;
            let id = format!("{:x}", Sha256::digest(path.as_bytes()));
            revision.push(format!("{id}:{width}:{height}"));
            images.push(PageDescriptor { id, index, width, height }); paths.push(path.to_owned());
        }
        let mut extra = normalize::safe_extra(&data);
        if let Some(extra) = extra.as_object_mut() { extra.remove("images"); extra.remove("pages"); }
        Ok(ImageIndex { episode_id: episode_id.into(), revision: format!("{:x}",Sha256::digest(revision.join("\n").as_bytes())),
            images, paths, extra })
    }

    pub async fn prepare_image_batch(&self, paths: &[String]) -> Result<AcquisitionBatch> {
        if paths.is_empty() || paths.len() > 20 || paths.iter().any(|path| path.is_empty() || path.len() > 8192 || path.chars().any(char::is_control)) {
            return Err(SourceError::invalid("invalid_image_path_batch"));
        }
        self.require_bound_account(self.session_snapshot().1.as_ref())?;
        let account_epoch = self.account_epoch.load(Ordering::SeqCst);
        let crypto = self.crypto.clone();
        let account_state = self.account_epoch.clone();
        let key = super::work::run_blocking(super::work::METADATA_WORK_BYTES, move |cancel| {
            if cancel.is_cancelled() || account_state.load(Ordering::SeqCst) != account_epoch { return Err(account_changed()); }
            crypto.new_image_key()
        }).await.map_err(|error| super::work::source_error(error, "image_key_worker_failed"))??;
        let encoded_paths = serde_json::to_string(paths).map_err(|_| SourceError::invalid("invalid_image_path_batch"))?;
        let data = self.post("comic.v1.Comic", "ImageToken", json!({"urls":encoded_paths,"m1":key.m1()}),
            PostOptions { require_session: true, ..Default::default() }).await?;
        if self.account_epoch.load(Ordering::SeqCst) != account_epoch { return Err(account_changed()); }
        let raw = data.get("tokens").unwrap_or(&data).as_array().filter(|tokens| tokens.len() == paths.len())
            .ok_or_else(|| SourceError::protocol("image_token_count_mismatch"))?;
        let mut tokens = Vec::with_capacity(raw.len());
        for (index, token) in raw.iter().enumerate() {
            if !token.is_object() { return Err(SourceError::protocol("image_token_invalid")); }
            let _ = image_url(token)?;
            tokens.push(ImageToken { raw: token.clone(), index });
        }
        Ok(AcquisitionBatch { tokens, key: Arc::new(key), account_epoch })
    }

    pub(crate) fn require_current_image_batch(&self, batch: &AcquisitionBatch) -> Result<()> {
        if self.account_epoch.load(Ordering::SeqCst) != batch.account_epoch { return Err(account_changed()); }
        self.require_bound_account(self.session_snapshot().1.as_ref())
    }

    pub async fn download_image(&self, batch: &AcquisitionBatch, token_index: usize) -> Result<VerifiedImage> {
        self.require_current_image_batch(batch)?;
        let token = batch.tokens.get(token_index).ok_or_else(|| SourceError::invalid("image_token_index_invalid"))?;
        let url = image_url(&token.raw)?;
        let encrypted = normalize::boolean(token.raw.get("hit_encrpyt")) == Some(true);
        let max_bytes = if encrypted { MAX_ENCRYPTED_BYTES } else { MAX_IMAGE_BYTES };
        let headers = if encrypted {
            vec![("referer".into(),format!("{MANGA_ORIGIN}/")), ("accept".into(),"application/octet-stream,image/*".into())]
        } else {
            vec![("referer".into(),format!("{MANGA_ORIGIN}/")), ("user-agent".into(), USER_AGENT.into())]
        };
        let response = self.send(HttpRequest { method: "GET".into(), url: url.clone(), headers, body: Vec::new(), max_bytes }).await?;
        if response.status != 200 {
            return Err(SourceError { status: Some(response.status), retryable: response.status == 429 || response.status >= 500,
                transmitted: Some(true), ..SourceError::new(ErrorKind::Http, "image_http_rejected") });
        }
        self.require_current_image_batch(batch)?;
        let ordinary = response.header("content-type").is_some_and(|value| value.to_ascii_lowercase().starts_with("image/"));
        let bytes = if encrypted && !ordinary {
            let crypto = self.crypto.clone(); let key = batch.key.clone(); let encoded = response.body;
            let account_state = self.account_epoch.clone(); let expected_epoch = batch.account_epoch;
            super::work::run_blocking(64 * 1024 * 1024, move |cancel| {
                let obsolete = || cancel.is_cancelled() || account_state.load(Ordering::SeqCst) != expected_epoch;
                if obsolete() { return Err(account_changed()); }
                crypto.decrypt_image_cancellable(&key, &url, &encoded, &obsolete)
            }).await.map_err(|error| super::work::source_error(error, "image_crypto_worker_failed"))??
        } else { response.body };
        if bytes.is_empty() || bytes.len() > max_bytes { return Err(SourceError::protocol("image_size_limit")); }
        let account_state = self.account_epoch.clone(); let expected_epoch = batch.account_epoch;
        let image = super::work::run_blocking(super::work::HEAVY_WORK_BYTES, move |cancel| {
            let obsolete = || cancel.is_cancelled() || account_state.load(Ordering::SeqCst) != expected_epoch;
            if obsolete() { return Err(account_changed()); }
            inspect_image_cancellable(bytes, None, &obsolete)
        }).await.map_err(|error| super::work::source_error(error, "image_inspection_worker_failed"))??;
        self.require_current_image_batch(batch)?;
        Ok(image)
    }

    pub async fn download_page(&self, index: &ImageIndex, page_index: usize) -> Result<VerifiedImage> {
        let descriptor = index.images.get(page_index).ok_or_else(|| SourceError::invalid("page_index_invalid"))?;
        let path = index.paths.get(page_index).ok_or_else(|| SourceError::capability("fresh_image_index_required"))?;
        let batch = self.prepare_image_batch(std::slice::from_ref(path)).await?;
        let image = self.download_image(&batch, 0).await?;
        if image.width != descriptor.width || image.height != descriptor.height {
            return Err(SourceError::protocol("image_geometry_changed"));
        }
        Ok(image)
    }

    pub async fn download_cover(&self, url: &str) -> Result<VerifiedImage> {
        let url = normalize_image_url(url)?;
        let response = self.send(HttpRequest { method: "GET".into(), url,
            headers: vec![("referer".into(),format!("{MANGA_ORIGIN}/")), ("user-agent".into(),USER_AGENT.into())],
            body: Vec::new(), max_bytes: MAX_COVER_BYTES }).await?;
        check_status(&response)?;
        super::work::run_blocking(super::work::COVER_WORK_BYTES, move |cancel| {
            inspect_image_cancellable(response.body, Some(4_000_000), &|| cancel.is_cancelled())
        }).await.map_err(|error| super::work::source_error(error, "cover_inspection_worker_failed"))?
    }

    /// Writes only a verified image; durable cache indexing remains the application's responsibility.
    pub async fn download_page_to(&self, index: &ImageIndex, page_index: usize, temporary_path: &Path) -> Result<VerifiedImage> {
        let image = self.download_page(index, page_index).await?;
        let path = temporary_path.to_owned();
        super::work::run_blocking(super::work::HEAVY_WORK_BYTES, move |cancel| -> Result<VerifiedImage> {
            use std::io::Write;
            if cancel.is_cancelled() { return Err(SourceError::new(ErrorKind::Cancelled, "source_work_cancelled")); }
            let mut options = std::fs::OpenOptions::new(); options.write(true).create_new(true);
            #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; options.mode(0o600); }
            let mut file = options.open(&path).map_err(|_| SourceError::storage("page_temporary_open_failed"))?;
            if file.write_all(&image.bytes).and_then(|_| file.sync_all()).is_err() {
                drop(file); let _ = std::fs::remove_file(&path); return Err(SourceError::storage("page_temporary_write_failed"));
            }
            drop(file);
            if cancel.is_cancelled() {
                let _ = std::fs::remove_file(&path);
                return Err(SourceError::new(ErrorKind::Cancelled, "source_work_cancelled"));
            }
            Ok(image)
        }).await.map_err(|error| super::work::source_error(error, "page_write_worker_failed"))?
    }
}

fn geometry(value: Option<&Value>) -> Result<u32> {
    let value = normalize::number(value).filter(|value| *value >= 1.0 && *value <= 1_000_000.0 && value.fract() == 0.0)
        .ok_or_else(|| SourceError::protocol("image_geometry_invalid"))?;
    Ok(value as u32)
}

fn account_changed() -> SourceError { SourceError { transmitted: Some(false), definitive: true,
    ..SourceError::new(ErrorKind::Cancelled, "account_changed") } }

fn image_url(token: &Value) -> Result<String> {
    if let Some(complete) = token.get("complete_url").and_then(Value::as_str).filter(|value| !value.is_empty()) {
        if complete.starts_with("http://") { return normalize_image_url(complete); }
        return normalize_image_url(&format!("{complete}&code=DanmakuInfo"));
    }
    let value = token.get("url").and_then(Value::as_str).filter(|value| !value.is_empty())
        .ok_or_else(|| SourceError::protocol("image_token_url_missing"))?;
    let value = if let Some(token) = token.get("token").and_then(Value::as_str).filter(|value| !value.is_empty()) {
        let encoded: String = url::form_urlencoded::byte_serialize(token.as_bytes()).collect();
        format!("{value}{}token={encoded}",if value.contains('?') { "&" } else { "?" })
    } else { value.into() };
    normalize_image_url(&value)
}

fn normalize_image_url(value: &str) -> Result<String> {
    let value = if let Some(value) = value.strip_prefix("http://") { format!("https://{value}") }
        else if value.starts_with("//") { format!("https:{value}") } else { value.into() };
    if value.len() > 32768 || value.chars().any(char::is_control) { return Err(SourceError::protocol("image_resource_invalid")); }
    let parsed = Url::parse(&value).map_err(|_| SourceError::protocol("image_resource_invalid"))?;
    let host = parsed.host_str().ok_or_else(|| SourceError::protocol("image_resource_invalid"))?;
    if parsed.scheme() != "https" || parsed.port_or_known_default() != Some(443) || !parsed.username().is_empty()
        || parsed.password().is_some() || ![".hdslb.com", ".biliimg.com", ".bilibili.com"].iter().any(|suffix| host.ends_with(suffix)) {
        return Err(SourceError::protocol("image_resource_invalid"));
    }
    Ok(value)
}

#[cfg(test)]
pub(super) fn inspect_image(bytes: Vec<u8>, max_pixels: Option<u64>) -> Result<VerifiedImage> {
    inspect_image_cancellable(bytes, max_pixels, &|| false)
}

fn inspect_image_cancellable(bytes: Vec<u8>, max_pixels: Option<u64>, is_cancelled: &dyn Fn() -> bool) -> Result<VerifiedImage> {
    let checked = crate::image_integrity::validate_image_cancellable(&bytes, max_pixels, is_cancelled).map_err(|error| {
        match error.kind {
            crate::image_integrity::IntegrityFailureKind::Corrupt => SourceError::protocol(error.code),
            crate::image_integrity::IntegrityFailureKind::ResourceLimit
            | crate::image_integrity::IntegrityFailureKind::Unsupported => SourceError::new(ErrorKind::Capability, error.code),
        }
    })?.ok_or_else(|| SourceError::new(ErrorKind::Cancelled, "source_work_cancelled"))?.into_metadata();
    Ok(VerifiedImage { bytes, width: checked.width, height: checked.height, format: checked.extension.into(),
        sha256: checked.sha256, verification: "decoded_v1" })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::ImageFormat;
    use std::io::Cursor;

    #[test]
    fn signed_image_url_is_not_reserialized_or_sent_to_an_unrelated_origin() {
        let token = json!({"complete_url":"https://i0.hdslb.com/a?token=a%2Bb&cpx=%252F"});
        assert_eq!(image_url(&token).unwrap(), "https://i0.hdslb.com/a?token=a%2Bb&cpx=%252F&code=DanmakuInfo");
        assert_eq!(image_url(&json!({"complete_url":"http://i0.hdslb.com/a?token=x"})).unwrap(), "https://i0.hdslb.com/a?token=x");
        assert!(image_url(&json!({"url":"https://hdslb.com.attacker.invalid/a"})).is_err());
        assert!(image_url(&json!({"url":"https://i0.hdslb.com@attacker.invalid/a"})).is_err());
    }

    #[test]
    fn png_crc_and_complete_container_are_checked_before_cache_publication() {
        let mut output = Cursor::new(Vec::new());
        image::DynamicImage::new_luma8(2, 3).write_to(&mut output, ImageFormat::Png).unwrap();
        let bytes = output.into_inner();
        let image = inspect_image(bytes.clone(), None).unwrap();
        assert_eq!((image.width,image.height,image.verification),(2,3,"decoded_v1"));
        let mut damaged = bytes.clone(); damaged[16] ^= 1;
        assert!(inspect_image(damaged, None).is_err());
        let mut repeated_header = bytes[..33].to_vec();
        repeated_header.extend_from_slice(&bytes[8..]);
        assert!(inspect_image(repeated_header, None).is_err());
        assert!(inspect_image(bytes[..bytes.len()-1].to_vec(), None).is_err());
        assert!(inspect_image(bytes, Some(5)).is_err());
    }
}
