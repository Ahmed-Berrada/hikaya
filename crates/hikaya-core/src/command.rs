//! Command mode: the user selects text, holds a shortcut and says what to do
//! with it ("make it shorter", "traduis en anglais"). These build the request
//! for the language model and clean its reply; the app copies the selection
//! and pastes the result over it.

use crate::meeting::strip_think;

const PROMPT: &str = include_str!("prompts/command.txt");

/// Longest selection sent to the model, in bytes. A local model handles a
/// few pages; beyond that the request would be slow and the reply cut.
pub const MAX_SELECTION_BYTES: usize = 20_000;

/// The request for the model. `context` is the line about the target app
/// from `context::prompt_line`, or empty.
pub fn prompt(selection: &str, instruction: &str, context: &str) -> String {
    let context = if context.is_empty() {
        String::new()
    } else {
        format!("- {context}\n")
    };
    PROMPT
        .replace("${context}", &context)
        .replace("${instruction}", instruction.trim())
        .replace("${selection}", selection)
}

/// The edited text from the model's reply, or `None` when it is empty. A
/// reasoning block, a code fence or quotes the model wrapped around the
/// text are removed, unless the selection had them too.
pub fn clean_reply(reply: &str, selection: &str) -> Option<String> {
    let mut text = strip_think(reply).trim();
    let selection = selection.trim();
    if text.starts_with("```") && !selection.starts_with("```") {
        let inner = text.trim_start_matches('`');
        // Drop a language tag on the fence's first line ("```markdown").
        let inner = inner.split_once('\n').map_or(inner, |(first, rest)| {
            if first.trim().contains(' ') {
                inner
            } else {
                rest
            }
        });
        text = inner.trim_end().trim_end_matches('`').trim();
    }
    for (open, close) in [('"', '"'), ('“', '”'), ('«', '»')] {
        if text.starts_with(open)
            && text.ends_with(close)
            && text.chars().count() >= 2
            && !(selection.starts_with(open) && selection.ends_with(close))
        {
            let inner = &text[open.len_utf8()..text.len() - close.len_utf8()];
            // Only one pair: "a" and "b" is not quoted as a whole.
            if !inner.contains(close) {
                text = inner.trim();
            }
        }
    }
    (!text.is_empty()).then(|| text.to_string())
}

/// Whether the spoken instruction asks for another language.
pub fn asks_to_translate(instruction: &str) -> bool {
    let folded: String = instruction
        .to_lowercase()
        .chars()
        .map(|c| match c {
            'ç' => 'c',
            'é' | 'è' | 'ê' => 'e',
            'à' | 'â' => 'a',
            other => other,
        })
        .collect();
    [
        "translat", "tradui", "tradu", "anglais", "francais", "english", "french", "espagnol",
        "spanish", "allemand", "german",
    ]
    .iter()
    .any(|word| folded.contains(word))
}

/// Whether the model switched the text's language: a small model sometimes
/// translates while told only to shorten or fix it. Mixed or short texts
/// whose language is unclear never count as switched.
pub fn switched_language(selection: &str, result: &str) -> bool {
    use crate::language::guess;
    matches!((guess(selection), guess(result)), (Some(before), Some(after)) if before != after)
}

/// Whether a selection can go to the model as is.
pub fn selection_fits(selection: &str) -> bool {
    !selection.trim().is_empty() && selection.len() <= MAX_SELECTION_BYTES
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_prompt_carries_the_text_the_instruction_and_the_place() {
        let p = prompt(
            "Hello team",
            " make it formal ",
            "The text is a chat message in Slack.",
        );
        assert!(p.contains("<text>\nHello team\n</text>"));
        assert!(p.contains("<instruction>\nmake it formal\n</instruction>"));
        assert!(p.contains("- The text is a chat message in Slack.\n"));
        assert!(!p.contains("${"));
        let plain = prompt("a", "b", "");
        assert!(!plain.contains("${"));
        assert!(!plain.contains("Slack"));
    }

    #[test]
    fn a_selection_with_placeholders_is_not_expanded_twice() {
        // The instruction is filled before the selection, so text that
        // happens to contain "${instruction}" stays as written.
        let p = prompt("costs ${instruction}", "shorter", "");
        assert!(p.contains("<text>\ncosts ${instruction}\n</text>"));
    }

    #[test]
    fn replies_lose_wrappers_the_selection_did_not_have() {
        assert_eq!(
            clean_reply("<think>hmm</think>\n Done text ", "x").as_deref(),
            Some("Done text")
        );
        assert_eq!(
            clean_reply("```markdown\n- one\n- two\n```", "one two").as_deref(),
            Some("- one\n- two")
        );
        assert_eq!(
            clean_reply("\"Short.\"", "A long sentence.").as_deref(),
            Some("Short.")
        );
        assert_eq!(
            clean_reply("« Bonjour »", "salut").as_deref(),
            Some("Bonjour")
        );
        // Kept when the selection was itself quoted, or for two quoted parts.
        assert_eq!(
            clean_reply("\"Hi\"", "\"hello\"").as_deref(),
            Some("\"Hi\"")
        );
        assert_eq!(
            clean_reply("\"a\" and \"b\"", "x").as_deref(),
            Some("\"a\" and \"b\"")
        );
        assert_eq!(clean_reply("   ", "x"), None);
        assert_eq!(clean_reply("<think>only thinking</think>", "x"), None);
    }

    #[test]
    fn a_translation_is_noticed_unless_it_was_asked_for() {
        let fr = "On se voit demain à dix heures pour la démo avec toute l'équipe.";
        let en = "We will meet tomorrow at ten for the demo with the whole team.";
        assert!(switched_language(fr, en));
        assert!(!switched_language(
            fr,
            "On se voit demain à dix heures pour la démo."
        ));
        assert!(!switched_language("ok", "d'accord"));
        for asked in [
            "traduis en anglais",
            "Translate to French",
            "mets-le en français",
            "in English please",
        ] {
            assert!(asks_to_translate(asked), "{asked}");
        }
        for not_asked in ["make it shorter", "rends-le plus formel", "fix the grammar"] {
            assert!(!asks_to_translate(not_asked), "{not_asked}");
        }
    }

    #[test]
    fn selections_must_hold_something_and_not_be_huge() {
        assert!(selection_fits("a word"));
        assert!(!selection_fits("  \n "));
        assert!(!selection_fits(&"x".repeat(MAX_SELECTION_BYTES + 1)));
    }
}
