//! Removes hesitation words ("um", "euh") from dictated text.
//!
//! Handy removes fillers for one detected language per dictation. A speaker
//! who mixes English and French in one sentence keeps the other language's
//! fillers. This module takes the set of languages the speaker uses and
//! removes the fillers of all of them in one pass.

use crate::token::{capitalize, join, tokenize, Token};

/// Fillers that are never a real word in English or French.
const EN: &[&str] = &[
    "um", "umm", "uh", "uhh", "uhm", "er", "erm", "hm", "hmm", "mm", "mmm",
];
const FR: &[&str] = &[
    "euh", "euhh", "euhm", "heu", "hum", "hm", "hmm", "mm", "mmm",
];

/// French fillers that are also English words. They are kept when the word
/// before them makes the English reading likely ("I hum", "to hum").
const AMBIGUOUS_IN_ENGLISH: &[&str] = &["hum"];
const ENGLISH_VERB_CONTEXT: &[&str] = &[
    "i", "you", "we", "they", "to", "a", "the", "can", "could", "will", "would", "and",
];

/// The built-in filler list for a language code such as `en` or `fr-CA`.
/// Unknown languages have no built-in fillers.
pub fn fillers_for(language: &str) -> &'static [&'static str] {
    let base = language
        .split(['-', '_'])
        .next()
        .unwrap_or(language)
        .to_lowercase();
    match base.as_str() {
        "en" => EN,
        "fr" => FR,
        _ => &[],
    }
}

/// Removes the fillers of every language in `languages`, plus `extra` words
/// the user added.
///
/// Punctuation is repaired around each removal: a comma that belonged to the
/// filler is dropped, sentence-ending punctuation moves to the previous word,
/// and the word after a sentence-opening filler is capitalized.
pub fn strip_fillers(text: &str, languages: &[&str], extra: &[String]) -> String {
    let speaks_english = languages.iter().any(|language| fillers_for(language) == EN);
    let is_filler = |tokens: &[Token], index: usize| -> bool {
        let key = tokens[index].key();
        if key.is_empty() {
            return false;
        }
        if extra.iter().any(|word| word.to_lowercase() == key) {
            return true;
        }
        let built_in = languages
            .iter()
            .any(|language| fillers_for(language).contains(&key.as_str()));
        if !built_in {
            return false;
        }
        if speaks_english && AMBIGUOUS_IN_ENGLISH.contains(&key.as_str()) {
            let previous = index.checked_sub(1).map(|i| tokens[i].key());
            if previous.is_some_and(|p| ENGLISH_VERB_CONTEXT.contains(&p.as_str())) {
                return false;
            }
        }
        true
    };

    let tokens = tokenize(text);
    let mut kept: Vec<Token> = Vec::with_capacity(tokens.len());
    // True when the next kept word opens a sentence whose first word was removed.
    let mut capital_owed = false;

    for index in 0..tokens.len() {
        let token = &tokens[index];
        if is_filler(&tokens, index) {
            let opened_sentence = kept.last().is_none_or(Token::ends_sentence);
            if token.ends_sentence() {
                // "Well, um. Next" -> "Well. Next"
                if let Some(previous) = kept.last_mut() {
                    if !previous.ends_sentence() {
                        previous.trail =
                            previous.trail.trim_end_matches([',', ';', ':']).to_string()
                                + &token.trail;
                    }
                }
                capital_owed = false;
            } else if opened_sentence && starts_uppercase(&token.word) {
                capital_owed = true;
            }
            continue;
        }

        let mut token = token.clone();
        if capital_owed && !token.word.is_empty() {
            token.word = capitalize(&token.word);
            capital_owed = false;
        }
        kept.push(token);
    }
    join(&kept)
}

fn starts_uppercase(word: &str) -> bool {
    word.chars().next().is_some_and(char::is_uppercase)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip(text: &str) -> String {
        strip_fillers(text, &["en", "fr"], &[])
    }

    #[test]
    fn removes_fillers_of_both_languages_in_one_sentence() {
        assert_eq!(
            strip("Euh donc on se voit demain, and then um we ship the release."),
            "Donc on se voit demain, and then we ship the release."
        );
    }

    #[test]
    fn regression_issue_6_sentence() {
        assert_eq!(
            strip("euh donc on se voit demain à 10h pour la démo, and then hum we ship the release friday"),
            "donc on se voit demain à 10h pour la démo, and then we ship the release friday"
        );
    }

    #[test]
    fn drops_the_fillers_own_comma() {
        assert_eq!(strip("So, um, we start now."), "So, we start now.");
        assert_eq!(strip("Donc, euh, on commence."), "Donc, on commence.");
    }

    #[test]
    fn moves_sentence_end_to_the_previous_word() {
        assert_eq!(
            strip("We could try, um. Next point."),
            "We could try. Next point."
        );
        assert_eq!(strip("Is it ready, euh?"), "Is it ready?");
    }

    #[test]
    fn capitalizes_the_word_after_a_sentence_opening_filler() {
        assert_eq!(strip("Um, the build passed."), "The build passed.");
        assert_eq!(strip("Done. Euh, on continue."), "Done. On continue.");
    }

    #[test]
    fn lowercase_opening_filler_does_not_force_a_capital() {
        assert_eq!(strip("euh on continue"), "on continue");
    }

    #[test]
    fn keeps_hum_when_it_is_an_english_verb() {
        assert_eq!(
            strip("I hum a song and they hum along."),
            "I hum a song and they hum along."
        );
        assert_eq!(strip("Alors hum on verra."), "Alors on verra.");
    }

    #[test]
    fn french_only_speaker_always_loses_hum() {
        assert_eq!(strip_fillers("I hum", &["fr"], &[]), "I");
    }

    #[test]
    fn respects_the_language_list() {
        assert_eq!(strip_fillers("euh hello um", &["en"], &[]), "euh hello");
        assert_eq!(strip_fillers("euh hello um", &["de"], &[]), "euh hello um");
        assert_eq!(strip_fillers("um ok", &["en-US"], &[]), "ok");
    }

    #[test]
    fn removes_user_added_fillers() {
        let extra = vec!["genre".to_string()];
        assert_eq!(
            strip_fillers("C'est genre, compliqué.", &["fr"], &extra),
            "C'est compliqué."
        );
    }

    #[test]
    fn does_not_touch_words_that_contain_a_filler() {
        assert_eq!(
            strip("The umbrella and the hummus."),
            "The umbrella and the hummus."
        );
    }

    #[test]
    fn text_of_only_fillers_becomes_empty() {
        assert_eq!(strip("Um, euh."), "");
    }

    proptest::proptest! {
        #[test]
        fn never_panics_and_never_adds_words(text in "\\PC{0,200}") {
            let out = strip(&text);
            proptest::prop_assert!(tokenize(&out).len() <= tokenize(&text).len());
        }

        #[test]
        fn is_idempotent(text in "[a-zA-Zéè ,.?!']{0,120}") {
            let once = strip(&text);
            proptest::prop_assert_eq!(strip(&once), once.clone());
        }
    }
}
