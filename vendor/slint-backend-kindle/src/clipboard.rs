//! An application-local text clipboard. No operating-system service is used.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ClipboardKind {
    Default,
    Selection,
}

/// Separate owned text slots, retained only for this platform's lifetime.
#[derive(Default)]
pub(crate) struct ClipboardStore {
    default_text: Option<String>,
    selection_text: Option<String>,
}

impl ClipboardStore {
    pub(crate) fn set(&mut self, kind: ClipboardKind, text: &str) {
        let destination = match kind {
            ClipboardKind::Default => &mut self.default_text,
            ClipboardKind::Selection => &mut self.selection_text,
        };
        *destination = Some(text.to_owned());
    }

    pub(crate) fn text(&self, kind: ClipboardKind) -> Option<String> {
        match kind {
            ClipboardKind::Default => self.default_text.clone(),
            ClipboardKind::Selection => self.selection_text.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_platform_has_no_clipboard_text() {
        let store = ClipboardStore::default();
        assert_eq!(store.text(ClipboardKind::Default), None);
        assert_eq!(store.text(ClipboardKind::Selection), None);
    }

    #[test]
    fn selecting_text_does_not_replace_an_explicit_copy() {
        let mut store = ClipboardStore::default();
        store.set(ClipboardKind::Default, "Explicit copy");
        store.set(ClipboardKind::Selection, "Selected text");
        assert_eq!(store.text(ClipboardKind::Default).as_deref(), Some("Explicit copy"));
        assert_eq!(store.text(ClipboardKind::Selection).as_deref(), Some("Selected text"));
    }

    #[test]
    fn copying_owns_the_text_and_pasting_returns_an_independent_value() {
        let mut store = ClipboardStore::default();
        let mut source = String::from("First line\nSecond line");
        store.set(ClipboardKind::Default, &source);
        source.clear();
        let mut first_paste = store.text(ClipboardKind::Default).unwrap();
        first_paste.clear();
        assert_eq!(store.text(ClipboardKind::Default).as_deref(), Some("First line\nSecond line"));
    }

    #[test]
    fn empty_text_replaces_previous_content_without_resurrecting_it() {
        let mut store = ClipboardStore::default();
        store.set(ClipboardKind::Default, "Previous copy");
        store.set(ClipboardKind::Default, "");
        assert_eq!(store.text(ClipboardKind::Default).as_deref(), Some(""));
        assert_eq!(store.text(ClipboardKind::Selection), None);
    }

    #[test]
    fn copying_preserves_unicode_and_paragraph_boundaries() {
        let mut store = ClipboardStore::default();
        let text = "\u{82b3}\u{8349}\u{9c9c}\u{7f8e}\n\u{843d}\u{82f1}\u{7f24}\u{7eb7}";
        store.set(ClipboardKind::Default, text);
        assert_eq!(store.text(ClipboardKind::Default).as_deref(), Some(text));
    }
}
