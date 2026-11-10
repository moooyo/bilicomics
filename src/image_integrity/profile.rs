//! Explicit, operation-local preparation timing without shared tracing state.

use crate::performance::{Span, StageMap};

/// Measure exactly one stage only when its caller supplied a destination.
/// The completion predicate distinguishes a completed stage from an error or
/// cancellation. A stage may finish successfully before its parent is cancelled.
pub(crate) fn measure<T>(
    profile: &mut Option<&mut StageMap>,
    stage: &'static str,
    work: impl FnOnce() -> T,
    completed: impl FnOnce(&T) -> bool,
) -> T {
    let span = profile.as_ref().map(|_| Span::start());
    let result = work();
    if let (Some(profile), Some(span)) = (profile.as_deref_mut(), span) {
        profile.entry(stage).or_default().record(span.finish(), completed(&result));
    }
    result
}
