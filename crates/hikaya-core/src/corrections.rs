//! Learns from the user's edits.
//!
//! When the user corrects a dictation, [`extract_substitutions`] compares the
//! text before and after and returns the word-level replacements. Once the
//! same replacement has been seen often enough, [`propose_rules`] suggests it
//! as a permanent [`Rule`].

use crate::rules::Rule;
use crate::token::{tokenize, Token};
use serde::{Deserialize, Serialize};
use similar::{capture_diff_slices, Algorithm, DiffOp};
use std::collections::HashMap;

/// Replacements longer than this on either side are a rewrite of the
/// sentence, not a fix of a misheard word, and are ignored.
const MAX_WORDS: usize = 3;

/// One word-level replacement the user made.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Substitution {
    /// What the transcription said.
    pub from: String,
    /// What the user changed it to.
    pub to: String,
}

/// A rule suggested from repeated corrections.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Proposal {
    /// The rule to add if the user accepts.
    pub rule: Rule,
    /// How many times the user made this correction.
    pub count: usize,
}

/// Returns the short word replacements that turn `before` into `after`.
///
/// Words that were only added or only removed are ignored, and so are changes
/// to punctuation alone. A change of capitalization ("github" → "GitHub")
/// counts as a replacement.
pub fn extract_substitutions(before: &str, after: &str) -> Vec<Substitution> {
    let words = |text: &str| -> Vec<String> {
        tokenize(text)
            .into_iter()
            .map(|token: Token| token.word)
            .filter(|word| !word.is_empty())
            .collect()
    };
    let old = words(before);
    let new = words(after);

    capture_diff_slices(Algorithm::Myers, &old, &new)
        .into_iter()
        .filter_map(|op| match op {
            DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } if old_len <= MAX_WORDS && new_len <= MAX_WORDS => Some(Substitution {
                from: old[old_index..old_index + old_len].join(" "),
                to: new[new_index..new_index + new_len].join(" "),
            }),
            _ => None,
        })
        .collect()
}

/// Suggests rules for corrections made at least `min_count` times.
///
/// `history` is every substitution recorded so far. A correction is skipped
/// when a rule for the same words already exists, or when the user has
/// corrected the same words to different things about equally often (the
/// right replacement is then unclear). The most frequent proposals come first.
pub fn propose_rules(
    history: &[Substitution],
    existing: &[Rule],
    min_count: usize,
) -> Vec<Proposal> {
    // from (lowercase) -> to -> count
    let mut counts: HashMap<String, HashMap<&str, usize>> = HashMap::new();
    for substitution in history {
        *counts
            .entry(substitution.from.to_lowercase())
            .or_default()
            .entry(substitution.to.as_str())
            .or_default() += 1;
    }

    let has_rule = |from: &str| {
        existing
            .iter()
            .any(|rule| rule.app.is_none() && rule.pattern.to_lowercase() == from)
    };

    let mut proposals: Vec<Proposal> = counts
        .into_iter()
        .filter(|(from, _)| !has_rule(from))
        .filter_map(|(from, targets)| {
            let total: usize = targets.values().sum();
            let (to, count) = targets
                .into_iter()
                .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(a.0)))?;
            // The winner must be a clear majority of the corrections for these words.
            (count >= min_count.max(1) && count * 2 > total).then(|| Proposal {
                rule: Rule::new(&from, to),
                count,
            })
        })
        .collect();
    proposals.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| a.rule.pattern.cmp(&b.rule.pattern))
    });
    proposals
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::apply_rules;

    fn sub(from: &str, to: &str) -> Substitution {
        Substitution {
            from: from.to_string(),
            to: to.to_string(),
        }
    }

    #[test]
    fn finds_a_misheard_name() {
        assert_eq!(
            extract_substitutions("Send it to a med today.", "Send it to Ahmed today."),
            vec![sub("a med", "Ahmed")]
        );
    }

    #[test]
    fn finds_several_fixes_in_one_edit() {
        assert_eq!(
            extract_substitutions(
                "push to get hub and ping clod",
                "push to GitHub and ping Claude"
            ),
            vec![sub("get hub", "GitHub"), sub("clod", "Claude")]
        );
    }

    #[test]
    fn capitalization_change_is_a_substitution() {
        assert_eq!(
            extract_substitutions("open github now", "open GitHub now"),
            vec![sub("github", "GitHub")]
        );
    }

    #[test]
    fn ignores_punctuation_only_edits_and_identical_text() {
        assert!(extract_substitutions("hello world", "Hello, world.").len() == 1);
        assert!(extract_substitutions("hello, world", "hello world.").is_empty());
        assert!(extract_substitutions("same text", "same text").is_empty());
    }

    #[test]
    fn ignores_added_and_removed_words() {
        assert!(extract_substitutions("we ship friday", "we ship on friday").is_empty());
        assert!(extract_substitutions("we really ship", "we ship").is_empty());
    }

    #[test]
    fn ignores_rewrites() {
        assert!(extract_substitutions(
            "the quick brown fox jumps over",
            "a slow red dog walks under"
        )
        .is_empty());
    }

    #[test]
    fn works_on_french() {
        assert_eq!(
            extract_substitutions("on déploie sur verse cell", "on déploie sur Vercel"),
            vec![sub("verse cell", "Vercel")]
        );
    }

    #[test]
    fn proposes_after_the_threshold() {
        let history = vec![sub("a med", "Ahmed")];
        assert!(propose_rules(&history, &[], 2).is_empty());

        let history = vec![
            sub("a med", "Ahmed"),
            sub("A med", "Ahmed"),
            sub("clod", "Claude"),
        ];
        let proposals = propose_rules(&history, &[], 2);
        assert_eq!(proposals.len(), 1);
        assert_eq!(proposals[0].rule, Rule::new("a med", "Ahmed"));
        assert_eq!(proposals[0].count, 2);
    }

    #[test]
    fn skips_words_that_already_have_a_rule() {
        let history = vec![sub("clod", "Claude"), sub("clod", "Claude")];
        assert!(propose_rules(&history, &[Rule::new("Clod", "Claude")], 2).is_empty());
    }

    #[test]
    fn skips_ambiguous_corrections() {
        let history = vec![
            sub("there", "their"),
            sub("there", "their"),
            sub("there", "they're"),
            sub("there", "they're"),
        ];
        assert!(propose_rules(&history, &[], 2).is_empty());
    }

    #[test]
    fn clear_majority_wins_over_a_rare_alternative() {
        let history = vec![
            sub("clod", "Claude"),
            sub("clod", "Claude"),
            sub("clod", "Claude"),
            sub("clod", "cloud"),
        ];
        let proposals = propose_rules(&history, &[], 2);
        assert_eq!(proposals[0].rule.replacement, "Claude");
        assert_eq!(proposals[0].count, 3);
    }

    #[test]
    fn most_frequent_proposals_come_first() {
        let history = vec![
            sub("b", "B"),
            sub("b", "B"),
            sub("a", "A"),
            sub("a", "A"),
            sub("a", "A"),
        ];
        let patterns: Vec<String> = propose_rules(&history, &[], 2)
            .into_iter()
            .map(|p| p.rule.pattern)
            .collect();
        assert_eq!(patterns, ["a", "b"]);
    }

    #[test]
    fn the_whole_loop_fixes_the_next_dictation() {
        let mut history = Vec::new();
        for _ in 0..2 {
            history.extend(extract_substitutions(
                "merci a med pour la demo",
                "merci Ahmed pour la demo",
            ));
        }
        let rules: Vec<Rule> = propose_rules(&history, &[], 2)
            .into_iter()
            .map(|p| p.rule)
            .collect();
        assert_eq!(
            apply_rules("Demande à a med, please.", &rules, None),
            "Demande à Ahmed, please."
        );
    }

    proptest::proptest! {
        #[test]
        fn never_panics(before in "\\PC{0,150}", after in "\\PC{0,150}") {
            let _ = extract_substitutions(&before, &after);
        }

        #[test]
        fn identical_text_has_no_substitutions(text in "\\PC{0,150}") {
            proptest::prop_assert!(extract_substitutions(&text, &text).is_empty());
        }
    }
}
