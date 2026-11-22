use crate::performance::Span;
use serde::Serialize;

#[derive(Debug, Default, Serialize)]
pub(crate) struct CatalogMaintenanceTrace {
    pub(crate) outcome: &'static str,
    pub(crate) committed: bool,
    pub(crate) recency_present: Option<bool>,
    pub(crate) timeout_restored: Option<bool>,
    pub(crate) phases: Vec<CatalogMaintenancePhase>,
}

#[derive(Debug, Serialize)]
pub(crate) struct CatalogMaintenancePhase {
    pub(crate) stage: &'static str,
    pub(crate) ok: bool,
    pub(crate) wall_us: u64,
    pub(crate) cpu_us: Option<u64>,
    pub(crate) rows: Option<usize>,
}

impl CatalogMaintenanceTrace {
    pub(super) fn reset(&mut self) {
        self.outcome = "error";
        self.committed = false;
        self.recency_present = None;
        self.timeout_restored = None;
        self.phases.clear();
        self.phases.reserve(12);
    }
}

pub(super) fn observe<T, E>(trace: &mut Option<&mut CatalogMaintenanceTrace>, stage: &'static str,
    work: impl FnOnce() -> Result<T, E>, rows: impl FnOnce(&T) -> Option<usize>) -> Result<T, E> {
    let Some(trace) = trace.as_mut() else { return work(); };
    let span = Span::start();
    let result = work();
    let elapsed = span.finish();
    trace.phases.push(CatalogMaintenancePhase {
        stage, ok: result.is_ok(), wall_us: elapsed.wall_us, cpu_us: elapsed.cpu_us,
        rows: result.as_ref().ok().and_then(rows),
    });
    result
}
