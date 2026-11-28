//! Where a dictation goes: the app that had the keyboard when it started.
//!
//! It is noted when the shortcut is pressed, before Hikaya's overlay shows,
//! kept under the shortcut until it is released, then travels with that
//! dictation's processing task. A second dictation started while the first
//! is still being cleaned up has its own, so neither sees the other's app.
//! Code outside a dictation (a transcribed file, a retry) sees none.

use hikaya_core::context::{understand, Target};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::Mutex;

/// What one dictation carries from the key press to its processing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Session {
    /// The app it goes into, if known.
    pub target: Option<Target>,
    /// Command mode: the words are an instruction for the selected text.
    pub command: bool,
}

/// Sessions of dictations started and not yet stopped, by shortcut.
static STARTED: Mutex<Option<HashMap<String, Session>>> = Mutex::new(None);
/// Shortcuts whose next dictation is a command (see `command_mode`).
static COMMANDS: Mutex<Option<HashSet<String>>> = Mutex::new(None);

tokio::task_local! {
    static DICTATION: Session;
}

/// Marks the dictation `binding` is about to start as a command.
pub fn mark_command(binding: &str) {
    let mut guard = COMMANDS.lock().unwrap_or_else(|e| e.into_inner());
    guard
        .get_or_insert_with(HashSet::new)
        .insert(binding.to_string());
}

fn take_command(binding: &str) -> bool {
    let mut guard = COMMANDS.lock().unwrap_or_else(|e| e.into_inner());
    guard.get_or_insert_with(HashSet::new).remove(binding)
}

fn started<R>(f: impl FnOnce(&mut HashMap<String, Session>) -> R) -> R {
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
    let session = Session {
        target,
        command: take_command(binding),
    };
    started(|map| map.insert(binding.to_string(), session));
}

/// Hands over the session noted when `binding`'s dictation started.
pub fn take(binding: &str) -> Session {
    started(|map| map.remove(binding)).unwrap_or_default()
}

/// Runs a dictation's processing with its session, for `current` and
/// `is_command`.
pub fn scoped<F: Future>(session: Session, work: F) -> impl Future<Output = F::Output> {
    DICTATION.scope(session, work)
}

/// The app the dictation being processed goes to, if known.
pub fn current() -> Option<Target> {
    DICTATION.try_with(|s| s.target.clone()).ok().flatten()
}

/// Whether the dictation being processed is a command for the selection.
pub fn is_command() -> bool {
    DICTATION.try_with(|s| s.command).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hikaya_core::context::Foreground;

    fn session(process: &str) -> Session {
        Session {
            target: target(process),
            command: false,
        }
    }

    fn target(process: &str) -> Option<Target> {
        Some(understand(&Foreground {
            process: process.into(),
            title: String::new(),
        }))
    }

    #[tokio::test]
    async fn each_dictation_keeps_its_own_target() {
        assert_eq!(current(), None);
        let slack = scoped(session("slack"), async {
            tokio::task::yield_now().await;
            current().map(|t| t.app)
        });
        let terminal = scoped(
            Session {
                command: true,
                ..session("pwsh")
            },
            async { (current().map(|t| t.app), is_command()) },
        );
        let (slack, terminal) = tokio::join!(slack, terminal);
        assert_eq!(slack.as_deref(), Some("slack"));
        assert_eq!(terminal, (Some("pwsh".to_string()), true));
        assert_eq!(current(), None);
        assert!(!is_command());
    }

    #[test]
    fn a_target_is_handed_over_once() {
        started(|map| map.insert("t".into(), session("slack")));
        assert_eq!(take("t").target.map(|t| t.app).as_deref(), Some("slack"));
        assert_eq!(take("t"), Session::default());
    }
}
