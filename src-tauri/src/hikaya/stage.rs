//! The deterministic text stage: runs on every dictation, after Handy's own
//! cleanup and before any language model.

use super::store::LearningStore;
use hikaya_core::{
    commands::apply_commands,
    fillers::strip_fillers,
    rules::{apply_rules, Rule},
    token::{join, tokenize},
};
use std::sync::Arc;
use tauri::{AppHandle, Manager};

/// What the stage needs to know, gathered from settings and the store.
pub struct StageInput<'a> {
    /// Languages the user speaks, such as `["en", "fr"]`.
    pub languages: &'a [String],
    /// Whether the user wants hesitation words removed.
    pub remove_fillers: bool,
    /// Extra filler words the user added.
    pub extra_fillers: &'a [String],
    /// The user's replacement rules.
    pub rules: &'a [Rule],
    /// Whether "new line", "scratch that" and their French forms act.
    pub voice_commands: bool,
}

/// Removes fillers for all of the user's languages, applies their rules,
/// then spoken commands. Commands come last because the steps before them
/// rebuild the text with single spaces, which would undo a line break.
/// Blank input is returned unchanged so "nothing was said" stays detectable.
pub fn apply_stage(text: &str, input: &StageInput) -> String {
    if text.trim().is_empty() {
        return text.to_string();
    }
    let languages: Vec<&str> = input.languages.iter().map(String::as_str).collect();
    let mut text = text.to_string();
    if input.remove_fillers {
        text = keep_original_unless_changed(
            &text,
            strip_fillers(&text, &languages, input.extra_fillers),
        );
    }
    if !input.rules.is_empty() {
        text = keep_original_unless_changed(&text, apply_rules(&text, input.rules, None));
    }
    if input.voice_commands {
        // "fr-CA" speaks French commands too.
        let bases: Vec<String> = languages
            .iter()
            .map(|l| l.split(['-', '_']).next().unwrap_or(l).to_lowercase())
            .collect();
        let bases: Vec<&str> = bases.iter().map(String::as_str).collect();
        text = apply_commands(&text, &bases);
    }
    text
}

/// The core functions rebuild the text with single spaces. When they changed
/// no word, return the original so its line breaks and spacing survive.
fn keep_original_unless_changed(original: &str, candidate: String) -> String {
    if join(&tokenize(original)) == candidate {
        original.to_string()
    } else {
        candidate
    }
}

/// Runs the stage for the running app. If the store is unavailable the text
/// is returned unchanged.
pub fn apply(app: &AppHandle, text: String) -> String {
    let settings = crate::settings::get_settings(app);
    let rules = app
        .try_state::<Arc<LearningStore>>()
        .map(|store| store.active_rules())
        .unwrap_or_default();
    let extra_fillers = settings.custom_filler_words.clone().unwrap_or_default();
    let voice_commands = app
        .try_state::<Arc<LearningStore>>()
        .is_none_or(|store| store.prefs().voice_commands);
    apply_stage(
        &text,
        &StageInput {
            languages: &settings.spoken_languages,
            remove_fillers: settings.filler_word_removal_enabled,
            extra_fillers: &extra_fillers,
            rules: &rules,
            voice_commands,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input<'a>(
        languages: &'a [String],
        remove_fillers: bool,
        rules: &'a [Rule],
    ) -> StageInput<'a> {
        StageInput {
            languages,
            remove_fillers,
            extra_fillers: &[],
            rules,
            voice_commands: true,
        }
    }

    #[test]
    fn commands_run_after_fillers_and_rules_and_keep_their_line_break() {
        let languages = vec!["en".to_string(), "fr-FR".to_string()];
        let rules = vec![Rule::new("get hub", "GitHub")];
        assert_eq!(
            apply_stage(
                "Euh j'ai poussé sur get hub. À la ligne. Um we ship Friday.",
                &input(&languages, true, &rules)
            ),
            "J'ai poussé sur GitHub.\nWe ship Friday."
        );
        let mut off = input(&languages, true, &rules);
        off.voice_commands = false;
        assert_eq!(
            apply_stage("Bonjour. À la ligne. Merci.", &off),
            "Bonjour. À la ligne. Merci."
        );
    }

    #[test]
    fn removes_mixed_language_fillers_then_applies_rules() {
        let languages = vec!["en".to_string(), "fr".to_string()];
        let rules = vec![Rule::new("get hub", "GitHub")];
        assert_eq!(
            apply_stage(
                "Euh on pousse sur get hub, and then hum we ship.",
                &input(&languages, true, &rules)
            ),
            "On pousse sur GitHub, and then we ship."
        );
    }

    #[test]
    fn keeps_fillers_when_the_setting_is_off() {
        let languages = vec!["en".to_string()];
        assert_eq!(
            apply_stage("um hello", &input(&languages, false, &[])),
            "um hello"
        );
    }

    #[test]
    fn blank_text_is_untouched() {
        let languages = vec!["en".to_string()];
        assert_eq!(apply_stage("  ", &input(&languages, true, &[])), "  ");
        assert_eq!(apply_stage("", &input(&languages, true, &[])), "");
    }

    #[test]
    fn text_with_nothing_to_fix_keeps_its_spacing_and_line_breaks() {
        let languages = vec!["en".to_string()];
        let rules = vec![Rule::new("get hub", "GitHub")];
        assert_eq!(
            apply_stage(
                "First line.\nSecond  line.",
                &input(&languages, true, &rules)
            ),
            "First line.\nSecond  line."
        );
    }
}
