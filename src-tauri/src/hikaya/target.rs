//! Where the current dictation goes: the app that had the keyboard when it
//! started. Read once at the start, before Hikaya's overlay appears, and
//! used by the text stage (rules scoped to an app) and the cleanup prompt.

use hikaya_core::context::{understand, Target};
use std::sync::Mutex;

static CURRENT: Mutex<Option<Target>> = Mutex::new(None);

/// Notes the focused app for the dictation that is starting. Unknown (and
/// so ignored) when `adapt` is off, on systems that do not say, and when
/// Hikaya itself is focused, as when testing from the settings window.
pub fn remember(adapt: bool) {
    let target = adapt
        .then(hikaya_foreground::foreground)
        .flatten()
        .map(|foreground| understand(&foreground))
        .filter(|target| !matches!(target.app.as_str(), "hikaya" | "handy"));
    if let Some(target) = &target {
        // The app's name only: a window title can hold an email subject.
        log::debug!("Dictating into {} ({:?})", target.app, target.kind);
    }
    match CURRENT.lock() {
        Ok(mut current) => *current = target,
        Err(poisoned) => *poisoned.into_inner() = target,
    }
}

/// The app the current dictation goes to, if known.
pub fn current() -> Option<Target> {
    match CURRENT.lock() {
        Ok(current) => current.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    }
}
