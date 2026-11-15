//! Whitespace tokenizer that keeps each word's surrounding punctuation.

/// One whitespace-separated piece of text, split into leading punctuation,
/// the word itself and trailing punctuation.
///
/// `"(Hello,"` becomes `lead = "("`, `word = "Hello"`, `trail = ","`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    /// Punctuation before the word, such as an opening quote or bracket.
    pub lead: String,
    /// The word with no surrounding punctuation. Empty for a token that is
    /// only punctuation.
    pub word: String,
    /// Punctuation after the word, such as a comma or full stop.
    pub trail: String,
}

impl Token {
    /// Splits one whitespace-free piece of text into a token.
    pub fn parse(piece: &str) -> Self {
        let start = piece
            .char_indices()
            .find(|(_, c)| c.is_alphanumeric())
            .map(|(i, _)| i);
        let Some(start) = start else {
            return Self {
                lead: String::new(),
                word: String::new(),
                trail: piece.to_string(),
            };
        };
        let end = piece
            .char_indices()
            .rev()
            .find(|(_, c)| c.is_alphanumeric())
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(piece.len());
        Self {
            lead: piece[..start].to_string(),
            word: piece[start..end].to_string(),
            trail: piece[end..].to_string(),
        }
    }

    /// The word in lowercase, for case-insensitive comparison.
    pub fn key(&self) -> String {
        self.word.to_lowercase()
    }

    /// True if the trailing punctuation ends a sentence.
    pub fn ends_sentence(&self) -> bool {
        self.trail.contains(['.', '!', '?', '…'])
    }

    /// The token as it appeared in the text.
    pub fn text(&self) -> String {
        format!("{}{}{}", self.lead, self.word, self.trail)
    }
}

/// Splits text into tokens on whitespace.
pub fn tokenize(text: &str) -> Vec<Token> {
    text.split_whitespace().map(Token::parse).collect()
}

/// Joins tokens back into text with single spaces.
pub fn join(tokens: &[Token]) -> String {
    tokens.iter().map(Token::text).collect::<Vec<_>>().join(" ")
}

/// Uppercases the first letter of a word, leaving the rest untouched.
pub fn capitalize(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_punctuation_from_the_word() {
        let token = Token::parse("(Hello,");
        assert_eq!(
            (
                token.lead.as_str(),
                token.word.as_str(),
                token.trail.as_str()
            ),
            ("(", "Hello", ",")
        );
    }

    #[test]
    fn keeps_inner_punctuation_in_the_word() {
        assert_eq!(Token::parse("l'école.").word, "l'école");
        assert_eq!(Token::parse("10h30").word, "10h30");
        assert_eq!(Token::parse("e-mail").word, "e-mail");
    }

    #[test]
    fn punctuation_only_token_has_no_word() {
        let token = Token::parse("—");
        assert!(token.word.is_empty());
        assert_eq!(token.text(), "—");
    }

    #[test]
    fn tokenize_then_join_normalizes_whitespace_only() {
        assert_eq!(join(&tokenize("  a  b,\n c. ")), "a b, c.");
    }

    #[test]
    fn capitalize_handles_accents_and_empty() {
        assert_eq!(capitalize("école"), "École");
        assert_eq!(capitalize(""), "");
    }
}
