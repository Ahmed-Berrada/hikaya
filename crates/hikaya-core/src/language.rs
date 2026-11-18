//! Guessing whether a text is mostly English or French, from its most
//! common words. Small local models answer in English unless the language
//! is named, so prompts say it outright.

/// A language Hikaya writes summaries in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
    /// English.
    English,
    /// French.
    French,
}

impl Language {
    /// The language's name, in English, for a prompt.
    pub fn name(self) -> &'static str {
        match self {
            Language::English => "English",
            Language::French => "French",
        }
    }
}

/// Words that are frequent in one language and rare in the other.
const FRENCH: &[&str] = &[
    "le", "la", "les", "des", "du", "une", "est", "et", "que", "qui", "pour", "pas", "dans", "sur",
    "avec", "on", "je", "tu", "nous", "vous", "il", "elle", "ce", "ça", "mais", "ou", "donc",
    "aussi", "très", "c'est", "j'ai", "qu'on", "au", "aux", "en", "se", "ne", "mon", "ton",
];
const ENGLISH: &[&str] = &[
    "the", "and", "is", "are", "to", "of", "that", "you", "it", "we", "for", "not", "with", "this",
    "have", "be", "was", "they", "what", "so", "but", "on", "in", "i", "my", "your", "can", "will",
    "do", "it's", "i'm", "there", "about", "just", "if",
];

/// The language most of `text` is in, or `None` when there is too little
/// to tell. Words like "on" and "en", common to both, count for each.
pub fn guess(text: &str) -> Option<Language> {
    let (mut french, mut english) = (0usize, 0usize);
    let words = text
        .split(|c: char| !(c.is_alphanumeric() || c == '\'' || c == '’'))
        .filter(|w| !w.is_empty());
    for word in words {
        let word = word.to_lowercase().replace('’', "'");
        french += usize::from(FRENCH.contains(&word.as_str()));
        english += usize::from(ENGLISH.contains(&word.as_str()));
    }
    if french + english < 3 {
        return None;
    }
    Some(if french >= english {
        Language::French
    } else {
        Language::English
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn french_and_english_are_told_apart() {
        assert_eq!(
            guess("Bon, on décale la démo client à jeudi parce que la version n'est pas prête."),
            Some(Language::French)
        );
        assert_eq!(
            guess("Let's move the client demo to Thursday because the build is not ready."),
            Some(Language::English)
        );
    }

    #[test]
    fn a_mostly_french_meeting_with_english_jargon_is_french() {
        assert_eq!(
            guess("On merge la pull request et on fait le deploy demain, c'est bon pour vous ?"),
            Some(Language::French)
        );
    }

    #[test]
    fn too_little_text_is_not_guessed() {
        assert_eq!(guess("OK"), None);
        assert_eq!(guess(""), None);
    }
}
