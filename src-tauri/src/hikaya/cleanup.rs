//! Hikaya's AI cleanup prompt for mixed English/French dictation, and the
//! check that keeps a bad cleanup from being pasted.

use crate::settings::{AppSettings, LLMPrompt};
use hikaya_core::faithful;

/// Id of Hikaya's built-in prompt in the post-processing prompt list.
pub const PROMPT_ID: &str = "hikaya_cleanup";

/// The prompt text. `eval/cleanup/run.py` tests this same file against a
/// local model.
const PROMPT: &str = include_str!("cleanup_prompt.txt");

/// Earlier versions of the built-in prompt. Settings keep a copy of the
/// prompt from the day the app was installed; a copy that is exactly one of
/// these was never edited, so it is replaced by the current text.
const EARLIER: &[&str] = &[
    include_str!("cleanup_prompt.v1.txt"),
    include_str!("cleanup_prompt.v2.txt"),
];

/// The current built-in prompt when `prompt` is an unedited earlier one.
fn upgraded(prompt: &str) -> &str {
    if EARLIER
        .iter()
        .any(|old| old.trim_end() == prompt.trim_end())
    {
        PROMPT.trim_end()
    } else {
        prompt
    }
}

/// Hikaya's built-in cleanup prompt, selected by default on a new install.
pub fn default_prompt() -> LLMPrompt {
    LLMPrompt {
        id: PROMPT_ID.to_string(),
        name: "Hikaya Cleanup (English + French)".to_string(),
        prompt: PROMPT.trim_end().to_string(),
    }
}

/// Fills Hikaya's placeholders in a cleanup prompt. `${vocabulary}` becomes
/// the speaker's custom words and rule replacements, so the model spells
/// names and terms the way the user does. Prompts without it are unchanged.
pub fn expand(prompt: &str, settings: &AppSettings) -> String {
    let prompt = upgraded(prompt);
    if !prompt.contains("${vocabulary}") {
        return prompt.to_string();
    }
    let rules = super::learning_store()
        .map(|store| store.active_rules())
        .unwrap_or_default();
    let words = hikaya_core::rules::vocabulary(&settings.custom_words, &rules);
    let list = if words.is_empty() {
        "(none)".to_string()
    } else {
        words.join(", ")
    };
    prompt.replace("${vocabulary}", &list)
}

/// Whether a post-processed text may replace the transcription.
///
/// Only Hikaya's own prompt is checked: it asks for a light cleanup, so a
/// reply with many words the speaker never said means the model translated,
/// answered or rewrote. Prompts the user wrote may ask for exactly that
/// (translate, turn into an email), so they are always accepted.
pub fn accepts(settings: &AppSettings, transcription: &str, processed: &str) -> bool {
    if settings.post_process_selected_prompt_id.as_deref() != Some(PROMPT_ID) {
        return true;
    }
    let result = faithful::compare(transcription, processed);
    if result.is_faithful() {
        return true;
    }
    log::warn!(
        "Cleanup rejected: {} new word(s) out of {}, {:.0}% of the original kept. Pasting the transcription as spoken.",
        result.new_words.len(),
        result.cleaned_words,
        result.kept_share * 100.0
    );
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::get_default_settings;

    #[test]
    fn the_hikaya_prompt_is_a_default_and_is_selected() {
        let settings = get_default_settings();
        assert_eq!(
            settings.post_process_selected_prompt_id.as_deref(),
            Some(PROMPT_ID)
        );
        let prompt = settings
            .post_process_prompts
            .iter()
            .find(|prompt| prompt.id == PROMPT_ID)
            .expect("Hikaya prompt is in the defaults");
        assert!(prompt.prompt.contains("${output}"));
        assert!(
            prompt.prompt.contains("Never translate") || prompt.prompt.contains("NEVER translate")
        );
    }

    #[test]
    fn an_unedited_earlier_prompt_is_upgraded_and_an_edited_one_is_kept() {
        for old in EARLIER {
            assert_eq!(upgraded(old), PROMPT.trim_end());
        }
        let edited = format!("{} Always use British spelling.", EARLIER[1]);
        assert_eq!(upgraded(&edited), edited);
        assert!(PROMPT.contains("line break"));
    }

    #[test]
    fn a_translated_cleanup_is_rejected_for_the_hikaya_prompt() {
        let settings = get_default_settings();
        assert!(!accepts(
            &settings,
            "on se voit mardi à 14h, and bring your laptop",
            "On se voit mardi à 14h, et apporte ton ordinateur."
        ));
        assert!(accepts(
            &settings,
            "on se voit mardi à 14h, and bring your laptop",
            "On se voit mardi à 14h, and bring your laptop."
        ));
    }

    #[test]
    fn vocabulary_is_filled_from_custom_words() {
        let mut settings = get_default_settings();
        settings.custom_words = vec!["GitHub".into(), "Hikaya".into()];
        assert_eq!(
            expand("Words: ${vocabulary}.", &settings),
            "Words: GitHub, Hikaya."
        );
        settings.custom_words.clear();
        assert_eq!(expand("Words: ${vocabulary}.", &settings), "Words: (none).");
        assert_eq!(expand("No placeholder", &settings), "No placeholder");
        assert!(default_prompt().prompt.contains("${vocabulary}"));
    }

    #[test]
    fn a_user_prompt_may_translate() {
        let mut settings = get_default_settings();
        settings.post_process_selected_prompt_id = Some("my_translate_prompt".to_string());
        assert!(accepts(&settings, "bonjour à tous", "Hello everyone"));
        settings.post_process_selected_prompt_id = None;
        assert!(accepts(&settings, "bonjour à tous", "Hello everyone"));
    }
}
