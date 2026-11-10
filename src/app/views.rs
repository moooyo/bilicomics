//! Deferred invalidation for models that are expensive to rebuild.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum View {
    Catalog,
    Account,
    Purchase,
    Downloads,
    Reader,
}

impl View {
    fn bit(self) -> u8 { 1 << (self as u8) }

    pub(super) fn for_route(route: &str) -> Option<Self> {
        match route {
            "shelf" | "store" | "search" | "detail" => Some(Self::Catalog),
            "account" => Some(Self::Account),
            "purchase" => Some(Self::Purchase),
            "downloads" => Some(Self::Downloads),
            "reader" => Some(Self::Reader),
            _ => None,
        }
    }
}

pub(super) struct ViewDirty { pending: u8 }

impl Default for ViewDirty {
    fn default() -> Self { Self { pending: 0b1_1111 } }
}

impl ViewDirty {
    pub(super) fn all(&mut self) { self.pending = 0b1_1111; }
    pub(super) fn mark(&mut self, view: View) { self.pending |= view.bit(); }
    pub(super) fn needs_route(&self, route: &str) -> bool {
        View::for_route(route).is_some_and(|view| self.pending & view.bit() != 0)
    }
    pub(super) fn take(&mut self, route: &str) -> Option<View> {
        let view = View::for_route(route)?;
        if self.pending & view.bit() == 0 { return None; }
        self.pending &= !view.bit();
        Some(view)
    }

    pub(super) fn job(&mut self, key: &str) {
        // Inventory workers often finish a bounded batch without a completed
        // chapter. Their callback marks views only when visible data changed.
        if key == "catalog:inventory-step" { return; }
        if key.starts_with("reader-") {
            self.mark(View::Reader);
        } else if key.starts_with("cover") {
            for view in [View::Catalog, View::Downloads, View::Purchase] { self.mark(view); }
        } else if key.starts_with("catalog:") {
            self.mark(View::Catalog);
        } else if key.starts_with("account-") {
            self.mark(View::Account);
            self.mark(View::Purchase);
            self.mark(View::Reader);
        } else if key.starts_with("purchase-") || key.starts_with("recharge-") {
            for view in [View::Account, View::Purchase, View::Catalog, View::Reader] { self.mark(view); }
        } else if key == "device-status" {
            self.mark(View::Reader);
            self.mark(View::Downloads);
        } else {
            self.all();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_invalidations_wait_for_their_route() {
        let mut dirty = ViewDirty { pending: 0 };
        dirty.job("cover-local:42");
        assert_eq!(dirty.take("reader"), None);
        assert_eq!(dirty.take("detail"), Some(View::Catalog));
        assert_eq!(dirty.take("shelf"), None);
        assert_eq!(dirty.take("downloads"), Some(View::Downloads));
        assert_eq!(dirty.take("purchase"), Some(View::Purchase));
    }

    #[test]
    fn reader_results_do_not_rebuild_hidden_catalogs() {
        let mut dirty = ViewDirty { pending: 0 };
        dirty.job("reader-render");
        assert!(!dirty.needs_route("shelf"));
        assert!(dirty.needs_route("reader"));
        assert_eq!(dirty.take("reader"), Some(View::Reader));
        assert!(!dirty.needs_route("reader"));
        dirty.all();
        assert_eq!(dirty.take("login"), None);
        assert!(dirty.needs_route("store"));
    }

}
