//! Command mode: select text, hold the command shortcut, say what to do with
//! it ("make it shorter", "traduis en anglais"), and the selection is
//! replaced by the result.
//!
//! It is an ordinary post-processed dictation (recording, overlay, speech to
//! text, paste) with one step swapped: instead of cleaning the words up,
//! they become an instruction for the selected text, which is copied just
//! before the model runs. By then the shortcut's keys are up, so the copy
//! shortcut reaches the app alone, and the text is still selected, so the
//! paste replaces it.

use super::problems::{report, Problem};
use crate::actions::{ProcessedTranscription, ShortcutAction};
use crate::settings::get_settings;
use hikaya_core::command;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};
use tauri_plugin_clipboard_manager::ClipboardExt;

/// Id of the command shortcut in the settings' bindings.
pub const BINDING: &str = "hikaya_command";

/// How long the model may take. A local model on a laptop GPU edits a page
/// in a few seconds; on a CPU it can take much longer.
const MODEL_TIMEOUT: Duration = Duration::from_secs(90);

/// How long to wait for the app to put the selection on the clipboard.
const COPY_TIMEOUT: Duration = Duration::from_millis(600);

/// The command shortcut: a post-processed dictation marked as a command.
pub struct CommandAction(pub Arc<dyn ShortcutAction>);

impl ShortcutAction for CommandAction {
    fn start(&self, app: &AppHandle, binding_id: &str, shortcut_str: &str) {
        super::target::mark_command(binding_id);
        self.0.start(app, binding_id, shortcut_str);
    }

    fn stop(&self, app: &AppHandle, binding_id: &str, shortcut_str: &str) {
        self.0.stop(app, binding_id, shortcut_str);
    }
}

/// Nothing to paste: the dictation ends quietly, and `report` has said why.
fn nothing() -> ProcessedTranscription {
    ProcessedTranscription {
        final_text: String::new(),
        post_processed_text: None,
        post_process_prompt: None,
        post_process_failed: false,
    }
}

/// Applies the spoken instruction to the selected text.
pub async fn process(app: &AppHandle, instruction: &str) -> ProcessedTranscription {
    let instruction = instruction.trim();
    if instruction.is_empty() {
        return nothing();
    }
    let copier = app.clone();
    let selection = tauri::async_runtime::spawn_blocking(move || copy_selection(&copier))
        .await
        .ok()
        .flatten();
    let Some(selection) = selection.filter(|s| command::selection_fits(s)) else {
        report(app, Problem::CommandNoSelection, None, "");
        return nothing();
    };

    let settings = get_settings(app);
    let Some(provider) = settings.active_post_process_provider().cloned() else {
        report(app, Problem::CommandNoModel, None, "");
        return nothing();
    };
    let model = settings
        .post_process_models
        .get(&provider.id)
        .cloned()
        .unwrap_or_default();
    if model.trim().is_empty() {
        report(app, Problem::CommandNoModel, None, "");
        return nothing();
    }
    let api_key = settings
        .post_process_api_keys
        .get(&provider.id)
        .cloned()
        .unwrap_or_default();
    let context = super::target::current()
        .map(|target| hikaya_core::context::prompt_line(&target))
        .unwrap_or_default();
    let prompt = command::prompt(&selection, instruction, &context);
    let disable_reasoning = matches!(provider.id.as_str(), "custom" | "openrouter");

    log::info!(
        "Command mode: {} chars selected, instruction of {} chars, model {model}",
        selection.chars().count(),
        instruction.chars().count()
    );
    let reply = tokio::time::timeout(
        MODEL_TIMEOUT,
        crate::llm_client::send_chat_completion(
            &provider,
            api_key,
            &model,
            prompt,
            disable_reasoning,
        ),
    )
    .await;
    let reply = match reply {
        Ok(Ok(Some(reply))) => reply,
        Ok(Ok(None)) => String::new(),
        Ok(Err(e)) => {
            report(app, Problem::CommandFailed, None, e);
            return nothing();
        }
        Err(_) => {
            report(
                app,
                Problem::CommandFailed,
                None,
                "The model took too long.",
            );
            return nothing();
        }
    };
    let Some(result) = command::clean_reply(&reply, &selection) else {
        report(
            app,
            Problem::CommandFailed,
            None,
            "The model sent back nothing.",
        );
        return nothing();
    };
    if !command::asks_to_translate(instruction) && command::switched_language(&selection, &result) {
        // A small model sometimes translates while told to shorten: keep
        // the user's text rather than replace it with another language.
        report(app, Problem::CommandTranslated, None, "");
        return nothing();
    }
    ProcessedTranscription {
        final_text: result.clone(),
        post_processed_text: Some(result),
        post_process_prompt: Some(format!("Command: {instruction}")),
        post_process_failed: false,
    }
}

/// Copies the focused app's selection and gives it back, leaving the
/// clipboard as it was. `None` when nothing is selected or the system
/// cannot send the copy shortcut (Wayland).
fn copy_selection(app: &AppHandle) -> Option<String> {
    #[cfg(target_os = "linux")]
    if crate::utils::is_wayland() {
        log::warn!("Command mode cannot copy the selection on Wayland");
        return None;
    }
    let clipboard = app.clipboard();
    let saved_text = clipboard.read_text().ok();
    let saved_image = if saved_text.as_deref().is_none_or(str::is_empty) {
        clipboard.read_image().ok().map(|image| image.to_owned())
    } else {
        None
    };
    // A marker no app would copy: if it is still there, nothing was selected.
    let marker = format!("hikaya-command-{}", std::process::id());
    clipboard.write_text(marker.clone()).ok()?;
    if let Err(e) = send_copy(app) {
        log::warn!("Command mode could not send the copy shortcut: {e}");
    }
    let started = Instant::now();
    let mut copied = None;
    while started.elapsed() < COPY_TIMEOUT {
        std::thread::sleep(Duration::from_millis(25));
        match clipboard.read_text() {
            Ok(text) if text != marker => {
                copied = Some(text);
                break;
            }
            _ => {}
        }
    }
    let restored = match (saved_text.filter(|t| !t.is_empty()), saved_image) {
        (Some(text), _) => clipboard.write_text(text),
        (None, Some(image)) => clipboard.write_image(&image),
        (None, None) => clipboard.clear(),
    };
    if let Err(e) = restored {
        log::warn!("Command mode could not restore the clipboard: {e}");
    }
    copied
}

/// Sends Ctrl+C (Cmd+C on macOS) to the focused app, on the main thread as
/// Handy's paste does.
fn send_copy(app: &AppHandle) -> Result<(), String> {
    use enigo::{Direction, Key, Keyboard};
    #[cfg(target_os = "macos")]
    let (modifier, c) = (Key::Meta, Key::Other(8)); // kVK_ANSI_C
    #[cfg(target_os = "windows")]
    let (modifier, c) = (Key::Control, Key::Other(0x43)); // VK_C
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let (modifier, c) = (Key::Control, Key::Unicode('c'));

    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let handle = app.clone();
    app.run_on_main_thread(move || {
        let result = (|| {
            let state = handle
                .try_state::<crate::input::EnigoState>()
                .ok_or("keyboard input is not ready")?;
            let mut enigo = state.0.lock().map_err(|e| e.to_string())?;
            enigo
                .key(modifier, Direction::Press)
                .map_err(|e| e.to_string())?;
            let clicked = enigo.key(c, Direction::Click).map_err(|e| e.to_string());
            std::thread::sleep(Duration::from_millis(30));
            enigo
                .key(modifier, Direction::Release)
                .map_err(|e| e.to_string())?;
            clicked
        })();
        let _ = tx.send(result);
    })
    .map_err(|e| e.to_string())?;
    rx.recv()
        .map_err(|_| "the main thread dropped the copy".to_string())?
}
