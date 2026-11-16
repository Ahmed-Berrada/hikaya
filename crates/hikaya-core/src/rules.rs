//! Whole-word replacement rules, such as "get hub" → "GitHub".
//!
//! Rules are deterministic and run before any language model, so a known
//! mistake is fixed the same way every time.

use crate::token::{join, tokenize, Token};
use serde::{Deserialize, Serialize};

/// One replacement rule.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    /// One or more words to look for. Matching ignores case and surrounding
    /// punctuation, and only matches whole words.
    pub pattern: String,
    /// The text that replaces a match, written exactly as it should appear.
    pub replacement: String,
    /// If set, the rule only applies when dictating into this application.
    #[serde(default)]
    pub app: Option<String>,
    /// Disabled rules are kept but never applied.
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
}

fn enabled_by_default() -> bool {
    true
}

impl Rule {
    /// A rule that applies in every application.
    pub fn new(pattern: &str, replacement: &str) -> Self {
        Self {
            pattern: pattern.to_string(),
            replacement: replacement.to_string(),
            app: None,
            enabled: true,
        }
    }

    fn applies_to(&self, app: Option<&str>) -> bool {
        self.enabled
            && match (&self.app, app) {
                (None, _) => true,
                (Some(scope), Some(app)) => scope.eq_ignore_ascii_case(app),
                (Some(_), None) => false,
            }
    }
}

/// Applies `rules` to `text`.
///
/// `app` is the application being dictated into, if known. When several rules
/// could match at the same position, the one with the longest pattern wins,
/// and an application-specific rule beats a general one of the same length.
/// Replaced text is never matched again, so rules cannot chain or loop.
pub fn apply_rules(text: &str, rules: &[Rule], app: Option<&str>) -> String {
    let mut compiled: Vec<(Vec<String>, &Rule)> = rules
        .iter()
        .filter(|rule| rule.applies_to(app))
        .map(|rule| {
            let keys: Vec<String> = tokenize(&rule.pattern)
                .iter()
                .map(Token::key)
                .filter(|key| !key.is_empty())
                .collect();
            (keys, rule)
        })
        .filter(|(keys, _)| !keys.is_empty())
        .collect();
    if compiled.is_empty() {
        return join(&tokenize(text));
    }
    compiled.sort_by_key(|(keys, rule)| (std::cmp::Reverse(keys.len()), rule.app.is_none()));

    let tokens = tokenize(text);
    let keys: Vec<String> = tokens.iter().map(Token::key).collect();
    let mut out: Vec<Token> = Vec::with_capacity(tokens.len());
    let mut index = 0;
    while index < tokens.len() {
        let hit = compiled.iter().find(|(pattern, _)| {
            let end = index + pattern.len();
            end <= tokens.len()
                && keys[index..end] == pattern[..]
                // Punctuation between the words means it is not one phrase.
                && tokens[index..end - 1].iter().all(|t| t.trail.is_empty())
                && tokens[index + 1..end].iter().all(|t| t.lead.is_empty())
        });
        match hit {
            Some((pattern, rule)) => {
                let last = index + pattern.len() - 1;
                out.push(Token {
                    lead: tokens[index].lead.clone(),
                    word: rule.replacement.clone(),
                    trail: tokens[last].trail.clone(),
                });
                index = last + 1;
            }
            None => {
                out.push(tokens[index].clone());
                index += 1;
            }
        }
    }
    join(&out)
}

/// Most words handed to a language model as the speaker's vocabulary.
pub const MAX_VOCABULARY: usize = 60;

/// The words the speaker cares about, for a language model to spell right:
/// their custom words, then the replacements of their enabled rules. Case
/// duplicates are dropped (the first spelling wins) and the list is capped
/// at [`MAX_VOCABULARY`].
pub fn vocabulary(custom_words: &[String], rules: &[Rule]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    custom_words
        .iter()
        .map(String::as_str)
        .chain(
            rules
                .iter()
                .filter(|r| r.enabled)
                .map(|r| r.replacement.as_str()),
        )
        .map(str::trim)
        .filter(|w| w.chars().any(char::is_alphanumeric))
        .filter(|w| seen.insert(w.to_lowercase()))
        .take(MAX_VOCABULARY)
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_a_phrase_ignoring_case_and_keeping_punctuation() {
        let rules = [Rule::new("get hub", "GitHub")];
        assert_eq!(
            apply_rules("Push it to Get Hub, then (get hub) again.", &rules, None),
            "Push it to GitHub, then (GitHub) again."
        );
    }

    #[test]
    fn only_matches_whole_words() {
        let rules = [Rule::new("cat", "dog")];
        assert_eq!(
            apply_rules("concatenate the cat", &rules, None),
            "concatenate the dog"
        );
    }

    #[test]
    fn does_not_match_across_punctuation() {
        let rules = [Rule::new("get hub", "GitHub")];
        assert_eq!(
            apply_rules("I get, hub is down", &rules, None),
            "I get, hub is down"
        );
    }

    #[test]
    fn longest_pattern_wins() {
        let rules = [Rule::new("new", "old"), Rule::new("new york", "NYC")];
        assert_eq!(
            apply_rules("a new york new idea", &rules, None),
            "a NYC old idea"
        );
    }

    #[test]
    fn replacements_are_not_matched_again() {
        let rules = [Rule::new("a", "b"), Rule::new("b", "a")];
        assert_eq!(apply_rules("a b", &rules, None), "b a");
    }

    #[test]
    fn app_scoped_rule_only_applies_in_that_app() {
        let mut rule = Rule::new("pr", "pull request");
        rule.app = Some("Slack".to_string());
        let rules = [rule];
        assert_eq!(
            apply_rules("open a pr", &rules, Some("slack")),
            "open a pull request"
        );
        assert_eq!(apply_rules("open a pr", &rules, Some("Code")), "open a pr");
        assert_eq!(apply_rules("open a pr", &rules, None), "open a pr");
    }

    #[test]
    fn app_scoped_rule_beats_a_general_one() {
        let mut scoped = Rule::new("pr", "PR");
        scoped.app = Some("Code".to_string());
        let rules = [Rule::new("pr", "pull request"), scoped];
        assert_eq!(apply_rules("a pr", &rules, Some("Code")), "a PR");
    }

    #[test]
    fn disabled_and_empty_rules_are_ignored() {
        let mut off = Rule::new("cat", "dog");
        off.enabled = false;
        let rules = [off, Rule::new("  ", "x"), Rule::new("...", "y")];
        assert_eq!(apply_rules("the cat ...", &rules, None), "the cat ...");
    }

    #[test]
    fn handles_accents_and_apostrophes() {
        let rules = [
            Rule::new("aujourd'hui", "aujourd’hui"),
            Rule::new("ECOLE", "École"),
        ];
        assert_eq!(
            apply_rules("L'ecole ferme aujourd'hui. Ecole!", &rules, None),
            "L'ecole ferme aujourd’hui. École!"
        );
    }

    #[test]
    fn vocabulary_merges_custom_words_and_rule_replacements() {
        let mut off = Rule::new("x", "Disabled");
        off.enabled = false;
        let rules = [
            Rule::new("get hub", "GitHub"),
            Rule::new("clod", "claude"),
            off,
            Rule::new("a", " ... "),
        ];
        let words = vocabulary(&["Claude".to_string(), " Hikaya ".to_string()], &rules);
        assert_eq!(words, ["Claude", "Hikaya", "GitHub"]);
    }

    #[test]
    fn vocabulary_is_capped() {
        let many: Vec<String> = (0..100).map(|i| format!("word{i}")).collect();
        assert_eq!(vocabulary(&many, &[]).len(), MAX_VOCABULARY);
    }

    #[test]
    fn rule_round_trips_through_json_with_defaults() {
        let rule: Rule =
            serde_json::from_str(r#"{"pattern":"a","replacement":"b"}"#).expect("valid rule JSON");
        assert_eq!(rule, Rule::new("a", "b"));
    }

    proptest::proptest! {
        #[test]
        fn no_rules_only_normalizes_whitespace(text in "\\PC{0,200}") {
            proptest::prop_assert_eq!(apply_rules(&text, &[], None), join(&tokenize(&text)));
        }

        #[test]
        fn never_panics(text in "\\PC{0,120}", pattern in "\\PC{0,20}", replacement in "\\PC{0,20}") {
            let _ = apply_rules(&text, &[Rule::new(&pattern, &replacement)], None);
        }
    }
}
