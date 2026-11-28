//! Where a dictation goes: the app that had the keyboard when it started.
//!
//! It is noted when the shortcut is pressed, before Hikaya's overlay shows,
//! kept under the shortcut until it is released, then travels with that
//! dictation's processing task. A second dictation started while the first
//! is still being cleaned up has its own, so neither sees the other's app.
//! Code outside a dictation (a transcribed file, a retry) sees none.

use hikaya_core::context::{understand, Target};
use std::collections::HashMap;
use std::future::Future;
use std::sync::Mutex;

/// Targets of dictations started and not yet stopped, by shortcut.
static STARTED: Mutex<Option<HashMap<String, Target>>> = Mutex::new(None);

tokio::task_local! {
    static DICTATION: Option<Target>;
}

fn started<R>(f: impl FnOnce(&mut HashMap<String, Target>) -> R) -> R {
    let mut guard = match STARTED.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    f(guard.get_or_insert_with(HashMap::new))
}

/// Notes the focused app for the dictation `binding` starts. Nothing is
/// noted when `adapt` is off, on systems that do not say, or when Hikaya
/// itself is focused, as when testing from the settings window.
pub fn remember(binding: &str, adapt: bool) {
    let target = adapt
        .then(hikaya_foreground::foreground)
        .flatten()
        .map(|foreground| understand(&foreground))
        .filter(|target| !matches!(target.app.as_str(), "hikaya" | "handy"));
    if let Some(target) = &target {
        // The app's name only: a window title can hold an email subject.
        log::debug!("Dictating into {} ({:?})", target.app, target.kind);
    }
    started(|map| match target {
        Some(target) => map.insert(binding.to_string(), target),
        None => map.remove(binding),
    });
}

/// Hands over the target noted when `binding`'s dictation started.
pub fn take(binding: &str) -> Option<Target> {
    started(|map| map.remove(binding))
}

/// Runs a dictation's processing with its target, for `current`.
pub fn scoped<F: Future>(target: Option<Target>, work: F) -> impl Future<Output = F::Output> {
    DICTATION.scope(target, work)
}

/// The app the dictation being processed goes to, if known.
pub fn current() -> Option<Target> {
    DICTATION.try_with(Clone::clone).ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use hikaya_core::context::Foreground;

    fn target(process: &str) -> Option<Target> {
        Some(understand(&Foreground {
            process: process.into(),
            title: String::new(),
        }))
    }

    #[tokio::test]
    async fn each_dictation_keeps_its_own_target() {
        assert_eq!(current(), None);
        let slack = scoped(target("slack"), async {
            tokio::task::yield_now().await;
            current().map(|t| t.app)
        });
        let terminal = scoped(target("pwsh"), async { current().map(|t| t.app) });
        let (slack, terminal) = tokio::join!(slack, terminal);
        assert_eq!(slack.as_deref(), Some("slack"));
        assert_eq!(terminal.as_deref(), Some("pwsh"));
        assert_eq!(current(), None);
    }

    #[test]
    fn a_target_is_handed_over_once() {
        started(|map| map.insert("t".into(), target("slack").expect("target")));
        assert_eq!(take("t").map(|t| t.app).as_deref(), Some("slack"));
        assert_eq!(take("t"), None);
    }
}
