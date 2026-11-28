//! Spoken commands: "new line", "new paragraph" and "scratch that", in
//! English and French, turned into what they mean.
//!
//! A command counts only when it stands apart from the sentence around it:
//! at the start or end of the text, or set off by punctuation, which is how
//! speech-to-text writes a pause. "Nouvelle ligne." is a command; "I added a
//! new line of code" is not. Matching ignores case and accents.

/// What a spoken command does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// Start a new line.
    NewLine,
    /// Start a new paragraph (a blank line).
    NewParagraph,
    /// Delete the sentence said just before.
    ScratchThat,
}

/// The phrases for each language, written folded (lowercase, no accents).
/// Longer phrases come first so "retour a la ligne" wins over "a la ligne".
fn phrases_for(language: &str) -> &'static [(&'static str, Command)] {
    match language {
        "en" => &[
            ("new paragraph", Command::NewParagraph),
            ("new line", Command::NewLine),
            // "delete that" is heard right most often; "scratch that" comes
            // out as "crutch that" on some models.
            ("delete that", Command::ScratchThat),
            ("scratch that", Command::ScratchThat),
        ],
        "fr" => &[
            ("nouveau paragraphe", Command::NewParagraph),
            ("retour a la ligne", Command::NewLine),
            ("nouvelle ligne", Command::NewLine),
            ("a la ligne", Command::NewLine),
            // "supprime ça" is heard right most often; Whisper also writes it
            // "suprime ça", and "annule ça" as "annulça". "Efface ça" is
            // often heard as "et face à ça", so it is kept but not shown.
            ("supprime ca", Command::ScratchThat),
            ("suprime ca", Command::ScratchThat),
            ("annule ca", Command::ScratchThat),
            ("annulca", Command::ScratchThat),
            ("efface ca", Command::ScratchThat),
        ],
        _ => &[],
    }
}

/// Lowercase, without the accents French uses, one char for one char so
/// positions in the folded text are positions in the original.
fn fold(c: char) -> char {
    let c = c.to_lowercase().next().unwrap_or(c);
    match c {
        'à' | 'â' | 'ä' | 'á' => 'a',
        'é' | 'è' | 'ê' | 'ë' => 'e',
        'î' | 'ï' => 'i',
        'ô' | 'ö' => 'o',
        'ù' | 'û' | 'ü' => 'u',
        'ç' => 'c',
        'ÿ' => 'y',
        '’' => '\'',
        other => other,
    }
}

/// Punctuation that speech-to-text writes for a pause around a command.
fn is_pause(c: char) -> bool {
    matches!(c, '.' | ',' | ';' | ':' | '!' | '?' | '…')
}

fn ends_sentence(c: char) -> bool {
    matches!(c, '.' | '!' | '?' | '…' | '\n')
}

/// Whether text written so far leaves room for a command: nothing yet, or a
/// pause or line break just before (spaces aside).
fn open_before(out: &str) -> bool {
    match out.trim_end_matches([' ', '\t']).chars().last() {
        None => true,
        Some(c) => is_pause(c) || c == '\n',
    }
}

/// Removes the sentence said before "scratch that". The sentence ends at
/// the last `.`, `!`, `?` or line break followed by a space (so "3.5" is not
/// a sentence end), or the text is emptied.
fn scratch(out: &str) -> String {
    let body = out.trim_end().trim_end_matches([',', ';', ':']).trim_end();
    let body = body.strip_suffix(['.', '!', '?', '…']).unwrap_or(body);
    let chars: Vec<(usize, char)> = body.char_indices().collect();
    for k in (0..chars.len()).rev() {
        let (at, c) = chars[k];
        let next_is_space = chars.get(k + 1).is_none_or(|(_, n)| n.is_whitespace());
        if ends_sentence(c) && (c == '\n' || next_is_space) {
            let end = at + c.len_utf8();
            let mut kept = body[..end].to_string();
            if c != '\n' {
                kept.push(' ');
            }
            return kept;
        }
    }
    String::new()
}

/// Turns spoken commands into line breaks and deletions. `languages` are
/// the user's spoken languages, such as `["en", "fr"]`; commands of other
/// languages are left as words.
pub fn apply_commands(text: &str, languages: &[&str]) -> String {
    let phrases: Vec<(Vec<char>, Command)> = languages
        .iter()
        .flat_map(|language| phrases_for(language))
        .map(|(phrase, command)| (phrase.chars().collect(), *command))
        .collect();
    if phrases.is_empty() {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let folded: Vec<char> = chars.iter().map(|&c| fold(c)).collect();
    let mut out = String::with_capacity(text.len());
    let mut capitalize = false;
    let mut changed = false;
    let mut i = 0;
    while i < chars.len() {
        if let Some((command, next)) = command_at(&chars, &folded, i, &phrases, &out) {
            changed = true;
            match command {
                Command::NewLine | Command::NewParagraph => {
                    let kept = out.trim_end_matches([' ', '\t']).len();
                    out.truncate(kept);
                    out.push_str(if command == Command::NewLine {
                        "\n"
                    } else {
                        "\n\n"
                    });
                }
                Command::ScratchThat => out = scratch(&out),
            }
            capitalize = out
                .trim_end_matches(' ')
                .chars()
                .last()
                .is_none_or(ends_sentence);
            i = next;
            continue;
        }
        let c = chars[i];
        if capitalize && c.is_alphabetic() {
            out.extend(c.to_uppercase());
            capitalize = false;
        } else if !(c == ' ' && (out.is_empty() || out.ends_with(['\n', ' ']))) {
            out.push(c);
            if !c.is_whitespace() {
                capitalize = false;
            }
        }
        i += 1;
    }
    if !changed {
        return text.to_string();
    }
    // A line break at the very end would act as Enter when pasted, and
    // send the message in most chat apps.
    out.trim_end().to_string()
}

/// The command starting at `i`, if one stands there, and where the text
/// after it (and its pause) continues.
fn command_at(
    chars: &[char],
    folded: &[char],
    i: usize,
    phrases: &[(Vec<char>, Command)],
    out: &str,
) -> Option<(Command, usize)> {
    if i > 0 && folded[i - 1].is_alphanumeric() {
        return None;
    }
    let (phrase, command) = phrases
        .iter()
        .find(|(phrase, _)| folded.get(i..i + phrase.len()) == Some(phrase.as_slice()))?;
    let mut j = i + phrase.len();
    if chars.get(j).is_some_and(|c| c.is_alphanumeric()) {
        return None;
    }
    if !open_before(out) {
        return None;
    }
    while chars.get(j) == Some(&' ') {
        j += 1;
    }
    let mut paused = false;
    while chars.get(j).copied().is_some_and(is_pause) {
        paused = true;
        j += 1;
    }
    if !paused && j < chars.len() {
        return None;
    }
    while chars.get(j) == Some(&' ') {
        j += 1;
    }
    Some((*command, j))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOTH: &[&str] = &["en", "fr"];

    #[test]
    fn new_line_and_paragraph_in_both_languages() {
        assert_eq!(
            apply_commands("Hi Sarah, new line. Thanks for the deck.", BOTH),
            "Hi Sarah,\nThanks for the deck."
        );
        assert_eq!(
            apply_commands(
                "Bonjour Paul. Nouveau paragraphe. Voici le compte rendu.",
                BOTH
            ),
            "Bonjour Paul.\n\nVoici le compte rendu."
        );
        assert_eq!(
            apply_commands("Premier point. À la ligne. Deuxième point.", BOTH),
            "Premier point.\nDeuxième point."
        );
        assert_eq!(
            apply_commands("Premier point, retour à la ligne, deuxième point.", BOTH),
            "Premier point,\nDeuxième point."
        );
    }

    #[test]
    fn a_command_inside_a_sentence_stays_words() {
        for text in [
            "I added a new line of code.",
            "On passe à la ligne suivante du tableau.",
            "Please delete that file.",
            "A new linebreak.",
        ] {
            assert_eq!(apply_commands(text, BOTH), text);
        }
    }

    #[test]
    fn scratch_that_removes_the_sentence_before() {
        assert_eq!(
            apply_commands("I'll be late. Scratch that. I'm on time.", BOTH),
            "I'm on time."
        );
        assert_eq!(
            apply_commands(
                "The demo is at ten. It costs 3.5 euros, scratch that. See you there.",
                BOTH
            ),
            "The demo is at ten. See you there."
        );
        assert_eq!(
            apply_commands("On se voit lundi. Efface ça. On se voit mardi.", BOTH),
            "On se voit mardi."
        );
        assert_eq!(apply_commands("Scratch that.", BOTH), "");
        // As speech-to-text actually writes them.
        assert_eq!(
            apply_commands("Je t'appelle lundi. Suprime ça. Je t'appelle mardi.", BOTH),
            "Je t'appelle mardi."
        );
        assert_eq!(
            apply_commands("Je t'appelle lundi. Annulça. Je t'appelle mardi.", BOTH),
            "Je t'appelle mardi."
        );
        assert_eq!(
            apply_commands(
                "I'll call you Monday. Delete that. I'll call you Tuesday.",
                BOTH
            ),
            "I'll call you Tuesday."
        );
    }

    #[test]
    fn commands_at_the_ends_need_no_punctuation() {
        assert_eq!(apply_commands("new line thanks", BOTH), "new line thanks");
        assert_eq!(apply_commands("Thanks, new line", BOTH), "Thanks,");
        assert_eq!(apply_commands("New line. Thanks.", BOTH), "\nThanks.");
    }

    #[test]
    fn only_the_users_languages_count() {
        let text = "Bonjour. À la ligne. Merci.";
        assert_eq!(apply_commands(text, &["en"]), text);
        assert_eq!(apply_commands(text, &["fr"]), "Bonjour.\nMerci.");
        assert_eq!(apply_commands(text, &[]), text);
    }

    #[test]
    fn text_without_commands_is_returned_as_is() {
        let text = "Two  spaces,\nand a line break stay.";
        assert_eq!(apply_commands(text, BOTH), text);
    }

    #[test]
    fn mixed_language_dictation() {
        assert_eq!(
            apply_commands(
                "J'ai poussé le fix. New line. And then we ship on Friday.",
                BOTH
            ),
            "J'ai poussé le fix.\nAnd then we ship on Friday."
        );
    }

    proptest::proptest! {
        #[test]
        fn never_panics_and_only_changes_text_with_a_command(
            text in "[a-zA-Zàéç’ ,.!?\n🙂]{0,80}",
            phrase in proptest::sample::select(vec!["", "new line", "À la ligne", "delete that", "supprime ça"]),
        ) {
            let with = format!("{text}. {phrase}. {text}");
            let _ = apply_commands(&with, BOTH);
            let folded: String = text.chars().map(fold).collect();
            let has_phrase = ["new line", "new paragraph", "scratch that", "delete that",
                "a la ligne", "nouvelle ligne", "nouveau paragraphe", "efface ca", "annule ca",
                "supprime ca", "suprime ca", "annulca"]
                .iter()
                .any(|p| folded.contains(p));
            if !has_phrase {
                proptest::prop_assert_eq!(apply_commands(&text, BOTH), text);
            }
        }
    }
}
