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
const COPY_TIMEOUT: Duration = Duration::from_millis(1000);

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

    let copier = app.clone();
    let copied = tauri::async_runtime::spawn_blocking(move || copy_selection(&copier))
        .await
        .unwrap_or(Copy::Failed("the copy step stopped".into()));
    let selection = match copied {
        Copy::Selected(text) if command::selection_fits(&text) => text,
        Copy::Selected(_) | Copy::Nothing => {
            report(app, Problem::CommandNoSelection, None, "");
            return nothing();
        }
        Copy::Unsupported => {
            report(app, Problem::CommandUnsupported, None, "");
            return nothing();
        }
        Copy::Failed(why) => {
            report(app, Problem::CommandFailed, None, why);
            return nothing();
        }
    };
    // The app the selection came from; the result must go back to it.
    let source = hikaya_foreground::foreground().map(|f| f.process);

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
    if hikaya_foreground::foreground().map(|f| f.process) != source {
        // The user switched apps while the model worked: pasting now would
        // put the result somewhere else.
        report(app, Problem::CommandFocusMoved, None, "");
        return nothing();
    }
    ProcessedTranscription {
        final_text: result.clone(),
        post_processed_text: Some(result),
        post_process_prompt: Some(format!("Command: {instruction}")),
        post_process_failed: false,
    }
}

/// What reading the selection gave.
enum Copy {
    /// The selected text.
    Selected(String),
    /// Nothing was selected (the copy left the clipboard alone).
    Nothing,
    /// This system gives Hikaya no way to send the copy shortcut (Wayland).
    Unsupported,
    /// The clipboard could not be used, or put back.
    Failed(String),
}

/// Copies the focused app's selection and gives it back, leaving the
/// clipboard as it was. Holds the clipboard against Handy's paste
/// meanwhile, so a paste from an earlier dictation is never read as the
/// selection.
fn copy_selection(app: &AppHandle) -> Copy {
    #[cfg(target_os = "linux")]
    if crate::utils::is_wayland() {
        return Copy::Unsupported;
    }
    let _clipboard_held = crate::utils::lock_clipboard();
    let clipboard = app.clipboard();
    // Only text or an image can be put back; for anything else (files,
    // formatted content alone) the copy would lose what the user had.
    enum Saved {
        Text(String),
        Image(tauri::image::Image<'static>),
        Empty,
    }
    let saved = match clipboard.read_text() {
        Ok(text) if !text.is_empty() => Saved::Text(text),
        _ => match clipboard.read_image() {
            Ok(image) => Saved::Image(image.to_owned()),
            Err(_) if clipboard.read_text().is_ok() => Saved::Empty,
            Err(_) => {
                return Copy::Failed(
                    "The clipboard holds something Hikaya cannot put back (files or formatted text). Copy some plain text, then try again."
                        .into(),
                )
            }
        },
    };
    // Unique per run, so a clipboard manager or a second run never mistakes
    // one marker for another.
    static RUN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let marker = format!(
        "hikaya-command-{}-{}",
        std::process::id(),
        RUN.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    if let Err(e) = clipboard.write_text(marker.clone()) {
        return Copy::Failed(format!("The clipboard could not be used: {e}"));
    }
    let sent = send_copy(app);
    let mut copied = None;
    if sent.is_ok() {
        let started = Instant::now();
        while started.elapsed() < COPY_TIMEOUT {
            std::thread::sleep(Duration::from_millis(25));
            if let Ok(text) = clipboard.read_text() {
                if text != marker {
                    copied = Some(text);
                    break;
                }
            }
        }
    }
    let restored = match saved {
        Saved::Text(text) => clipboard.write_text(text),
        Saved::Image(image) => clipboard.write_image(&image),
        Saved::Empty => clipboard.clear(),
    };
    if let Err(e) = restored {
        log::warn!("Command mode could not restore the clipboard: {e}");
    }
    match (sent, copied) {
        (Err(e), _) => Copy::Failed(format!("The copy shortcut could not be sent: {e}")),
        (Ok(()), Some(text)) => Copy::Selected(text),
        (Ok(()), None) => Copy::Nothing,
    }
}

/// Sends Ctrl+C (Cmd+C on macOS) to the focused app, on the main thread as
/// Handy's paste does. Gives up after a second if the main thread is busy.
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
    rx.recv_timeout(Duration::from_secs(1))
        .map_err(|_| "the app was busy".to_string())?
}
