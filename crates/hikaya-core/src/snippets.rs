//! Snippets: a short spoken cue ("my address", "mon lien calendly") that
//! types a saved text, which may run over several lines.
//!
//! Unlike replacement rules, a snippet's text is inserted exactly as saved,
//! line breaks included, so this runs last in the text stage. A cue matches
//! whole words, ignoring case, accents and the punctuation speech-to-text
//! puts around it; a period or comma that ended the cue is dropped with it.

/// Longest cue, in bytes.
pub const MAX_CUE_BYTES: usize = 100;
/// Longest snippet text, in bytes.
pub const MAX_TEXT_BYTES: usize = 4_000;

/// One snippet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snippet {
    /// What the user says.
    pub cue: String,
    /// What gets typed.
    pub text: String,
}

/// The cue as it is compared: lowercase, accents and punctuation removed,
/// single spaces. Empty when nothing is left.
pub fn normalize_cue(cue: &str) -> String {
    let words: Vec<String> = cue
        // The same word characters as `words`, typographic apostrophe
        // included, so "l’adresse" stays one word on both sides.
        .split(|c: char| !(c.is_alphanumeric() || c == '\'' || c == '’'))
        .filter(|w| !w.is_empty())
        .map(|w| w.chars().map(fold).collect())
        .collect();
    words.join(" ")
}

fn fold(c: char) -> char {
    let c = c.to_lowercase().next().unwrap_or(c);
    match c {
        'à' | 'â' | 'ä' | 'á' => 'a',
        'é' | 'è' | 'ê' | 'ë' => 'e',
        'î' | 'ï' => 'i',
        'ô' | 'ö' => 'o',
        'ù' | 'û' | 'ü' => 'u',
        'ç' => 'c',
        '’' => '\'',
        other => other,
    }
}

/// Whether a snippet can be saved as is.
pub fn valid(cue: &str, text: &str) -> bool {
    !normalize_cue(cue).is_empty()
        && cue.len() <= MAX_CUE_BYTES
        && !text.trim().is_empty()
        && text.len() <= MAX_TEXT_BYTES
}

/// A word of the text with where it sits, for matching.
struct Word {
    key: String,
    start: usize,
    end: usize,
}

fn words(text: &str) -> Vec<Word> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, c) in text.char_indices() {
        let inside = c.is_alphanumeric() || c == '\'' || c == '’';
        match (inside, start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                out.push(Word {
                    key: text[s..i].chars().map(fold).collect(),
                    start: s,
                    end: i,
                });
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        out.push(Word {
            key: text[s..].chars().map(fold).collect(),
            start: s,
            end: text.len(),
        });
    }
    out
}

/// Types each snippet whose cue was said. Longer cues win when two could
/// match at the same place; inserted text is never matched again.
pub fn expand(text: &str, snippets: &[Snippet]) -> String {
    let mut cues: Vec<(Vec<String>, &Snippet)> = snippets
        .iter()
        .filter(|s| valid(&s.cue, &s.text))
        .map(|s| {
            let keys = normalize_cue(&s.cue)
                .split(' ')
                .map(str::to_string)
                .collect();
            (keys, s)
        })
        .collect();
    if cues.is_empty() {
        return text.to_string();
    }
    cues.sort_by_key(|(keys, _)| std::cmp::Reverse(keys.len()));
    let found = words(text);
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut i = 0;
    while i < found.len() {
        let hit = cues.iter().find(|(keys, _)| {
            found
                .get(i..i + keys.len())
                .is_some_and(|slice| slice.iter().zip(keys.iter()).all(|(w, k)| &w.key == k))
        });
        let Some((keys, snippet)) = hit else {
            i += 1;
            continue;
        };
        let first = &found[i];
        let last = &found[i + keys.len() - 1];
        out.push_str(&text[copied..first.start]);
        out.push_str(&snippet.text);
        // The pause mark speech-to-text put after the cue goes with it.
        let mut end = last.end;
        if let Some(c) = text[end..].chars().next() {
            if matches!(c, '.' | ',' | ';' | '!' | '?') {
                end += c.len_utf8();
            }
        }
        copied = end;
        i += keys.len();
    }
    out.push_str(&text[copied..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snippet(cue: &str, text: &str) -> Snippet {
        Snippet {
            cue: cue.into(),
            text: text.into(),
        }
    }

    #[test]
    fn a_cue_types_its_text_with_line_breaks() {
        let s = [snippet("my address", "12 rue de la Paix\n75002 Paris")];
        assert_eq!(
            expand("Send it to my address.", &s),
            "Send it to 12 rue de la Paix\n75002 Paris"
        );
        assert_eq!(
            expand("My Address, please", &s),
            "12 rue de la Paix\n75002 Paris please"
        );
    }

    #[test]
    fn cues_match_whole_words_ignoring_accents() {
        let s = [snippet("mon lien calendly", "https://cal.example/ahmed")];
        assert_eq!(
            expand("Voici mon lien Calendly.", &s),
            "Voici https://cal.example/ahmed"
        );
        let s = [snippet("sig", "— Ahmed")];
        assert_eq!(expand("A signal and a design", &s), "A signal and a design");
        let s = [snippet("adresse", "X")];
        assert_eq!(expand("Mon adrèsse", &s), "Mon X");
    }

    #[test]
    fn a_typographic_apostrophe_matches_either_way() {
        let s = [snippet("l’adresse", "X")];
        assert_eq!(expand("Envoie à l'adresse.", &s), "Envoie à X");
        let s = [snippet("l'adresse", "X")];
        assert_eq!(expand("Envoie à l’adresse", &s), "Envoie à X");
    }

    #[test]
    fn the_longest_cue_wins_and_inserted_text_is_not_expanded_again() {
        let s = [
            snippet("address", "SHORT"),
            snippet("work address", "address of the office"),
        ];
        assert_eq!(expand("my work address", &s), "my address of the office");
    }

    #[test]
    fn invalid_snippets_are_ignored() {
        assert!(!valid("  ...  ", "text"));
        assert!(!valid("cue", "   "));
        assert!(!valid("cue", &"x".repeat(MAX_TEXT_BYTES + 1)));
        assert_eq!(expand("hello", &[snippet("", "x")]), "hello");
    }

    #[test]
    fn text_without_a_cue_is_unchanged() {
        let text = "Two  spaces,\nand a break.";
        assert_eq!(expand(text, &[snippet("my address", "x")]), text);
    }

    proptest::proptest! {
        #[test]
        fn never_panics(text in "[a-zA-Zàéç’' ,.!?\\n🙂]{0,80}") {
            let _ = expand(&text, &[snippet("é a", "X\nY"), snippet("a", "Z")]);
        }
    }
}
