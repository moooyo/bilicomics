pub mod app;
pub mod cache;
pub mod domain;
pub mod demo;
pub mod downloads;
pub mod fonts;
pub mod i18n;
pub mod input;
pub mod native_presentation;
pub mod native_media;
pub(crate) mod native_media_ui;
pub(crate) mod image_integrity;
pub mod performance;
pub mod platform;
pub mod power_events;
pub mod reader;
pub mod source;
pub mod store;

pub mod ui {
    slint::include_modules!();
}
