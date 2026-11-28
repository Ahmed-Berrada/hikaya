//! Where the dictated text goes: the app that had the keyboard when the
//! dictation started, and what kind of place that is.
//!
//! The kind tunes the cleanup (a chat message is short and casual, a
//! terminal command keeps its lowercase) and the app's name scopes
//! replacement rules. Only formatting changes, never the words, so the
//! cleanup's faithfulness check still applies.

/// The focused app, as the operating system reports it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Foreground {
    /// The executable's name without its folder or `.exe`, lowercase, such
    /// as `slack`, `chrome` or `code`.
    pub process: String,
    /// The window title. Read only to recognize web apps (Gmail in Chrome);
    /// never stored, synced or logged.
    pub title: String,
}

/// What kind of place the text goes into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppKind {
    /// A chat or messaging app.
    Chat,
    /// An email client or webmail.
    Email,
    /// A code editor or IDE.
    Code,
    /// A command line.
    Terminal,
    /// A word processor or notes app.
    Document,
    /// Anything else, or unknown.
    Other,
}

/// Desktop apps by executable name.
const PROCESSES: &[(&str, AppKind)] = &[
    ("slack", AppKind::Chat),
    ("discord", AppKind::Chat),
    ("teams", AppKind::Chat),
    ("ms-teams", AppKind::Chat),
    ("whatsapp", AppKind::Chat),
    ("telegram", AppKind::Chat),
    ("signal", AppKind::Chat),
    ("messenger", AppKind::Chat),
    ("outlook", AppKind::Email),
    ("olk", AppKind::Email),
    ("thunderbird", AppKind::Email),
    ("mailspring", AppKind::Email),
    ("code", AppKind::Code),
    ("cursor", AppKind::Code),
    ("windsurf", AppKind::Code),
    ("zed", AppKind::Code),
    ("devenv", AppKind::Code),
    ("sublime_text", AppKind::Code),
    ("idea64", AppKind::Code),
    ("pycharm64", AppKind::Code),
    ("webstorm64", AppKind::Code),
    ("rider64", AppKind::Code),
    ("clion64", AppKind::Code),
    ("rustrover64", AppKind::Code),
    ("windowsterminal", AppKind::Terminal),
    ("wt", AppKind::Terminal),
    ("cmd", AppKind::Terminal),
    ("powershell", AppKind::Terminal),
    ("pwsh", AppKind::Terminal),
    ("wezterm-gui", AppKind::Terminal),
    ("alacritty", AppKind::Terminal),
    ("gnome-terminal-server", AppKind::Terminal),
    ("konsole", AppKind::Terminal),
    ("kitty", AppKind::Terminal),
    ("winword", AppKind::Document),
    ("notion", AppKind::Document),
    ("obsidian", AppKind::Document),
    ("onenote", AppKind::Document),
    ("soffice.bin", AppKind::Document),
    ("soffice", AppKind::Document),
];

/// Web apps, by the end of the browser's window title (" - Gmail").
const TITLES: &[(&str, &str, AppKind)] = &[
    ("gmail", "Gmail", AppKind::Email),
    ("outlook", "Outlook", AppKind::Email),
    ("proton mail", "Proton Mail", AppKind::Email),
    ("slack", "Slack", AppKind::Chat),
    ("discord", "Discord", AppKind::Chat),
    ("whatsapp", "WhatsApp", AppKind::Chat),
    ("messenger", "Messenger", AppKind::Chat),
    ("microsoft teams", "Microsoft Teams", AppKind::Chat),
    ("google docs", "Google Docs", AppKind::Document),
    ("notion", "Notion", AppKind::Document),
];

const BROWSERS: &[&str] = &[
    "chrome", "msedge", "firefox", "brave", "opera", "vivaldi", "arc", "zen",
];

/// The focused app, understood: a display name and its kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    /// The name rules are scoped by and the cleanup is told: the process,
    /// or the web app's name in a browser (`gmail`).
    pub app: String,
    /// The app's name as people write it, for the cleanup prompt.
    pub display: String,
    /// What kind of place it is.
    pub kind: AppKind,
}

/// Recognizes the focused app.
pub fn understand(foreground: &Foreground) -> Target {
    let process = foreground.process.trim().to_lowercase();
    let process = process.strip_suffix(".exe").unwrap_or(&process).to_string();
    if BROWSERS.contains(&process.as_str()) {
        // Browsers end the title with their own name: "Inbox - Gmail - Google Chrome".
        let title = foreground.title.to_lowercase();
        let parts: Vec<&str> = segments(&title).into_iter().map(strip_counter).collect();
        for (key, display, kind) in TITLES {
            if parts.contains(key) {
                return Target {
                    app: key.replace(' ', "-"),
                    display: (*display).to_string(),
                    kind: *kind,
                };
            }
        }
    }
    let kind = PROCESSES
        .iter()
        .find(|(name, _)| *name == process)
        .map_or(AppKind::Other, |(_, kind)| *kind);
    Target {
        display: display_name(&process),
        app: process,
        kind,
    }
}

/// A browser title cut at its separators: "Inbox - Gmail | x" → ["Inbox", "Gmail", "x"].
fn segments(title: &str) -> Vec<&str> {
    let mut parts = vec![title];
    for separator in [" - ", " | ", " — ", " – ", " · "] {
        parts = parts.iter().flat_map(|p| p.split(separator)).collect();
    }
    parts
}

/// "(3) WhatsApp" → "whatsapp": an unread counter is not part of the name.
fn strip_counter(part: &str) -> &str {
    let part = part.trim();
    match part
        .strip_prefix('(')
        .and_then(|rest| rest.split_once(") "))
    {
        Some((count, rest)) if count.chars().all(|c| c.is_ascii_digit() || c == '+') => rest.trim(),
        _ => part,
    }
}

fn display_name(process: &str) -> String {
    match process {
        "code" => "VS Code".into(),
        "olk" | "outlook" => "Outlook".into(),
        "winword" => "Word".into(),
        "windowsterminal" | "wt" => "Windows Terminal".into(),
        "ms-teams" | "teams" => "Microsoft Teams".into(),
        "whatsapp" => "WhatsApp".into(),
        other => {
            let mut chars = other.chars();
            chars
                .next()
                .map(|c| c.to_uppercase().chain(chars).collect())
                .unwrap_or_default()
        }
    }
}

/// One line for the cleanup prompt about where the text goes. Empty when
/// the place is unknown or needs nothing beyond the default cleanup (email,
/// documents), so the prompt then reads as before.
pub fn prompt_line(target: &Target) -> String {
    let name = &target.display;
    match target.kind {
        AppKind::Chat => format!(
            "The text is a chat message in {name}: keep it casual, and a single short sentence needs no final period."
        ),
        AppKind::Code => format!(
            "The text goes into a code editor ({name}): keep code words, file names and symbols exactly as spoken."
        ),
        AppKind::Terminal => format!(
            "The text goes into a terminal ({name}): keep commands, flags and file names in lowercase, exactly as spoken, with no final period."
        ),
        // Email and documents get the default cleanup: a line asking for
        // "complete sentences" made the small model translate and rewrite.
        AppKind::Email | AppKind::Document | AppKind::Other => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fg(process: &str, title: &str) -> Foreground {
        Foreground {
            process: process.into(),
            title: title.into(),
        }
    }

    #[test]
    fn desktop_apps_by_process() {
        let slack = understand(&fg("slack.exe", "general | Acme"));
        assert_eq!((slack.app.as_str(), slack.kind), ("slack", AppKind::Chat));
        assert_eq!(understand(&fg("Code.exe", "main.rs")).kind, AppKind::Code);
        assert_eq!(
            understand(&fg("WindowsTerminal.exe", "")).kind,
            AppKind::Terminal
        );
        assert_eq!(understand(&fg("WINWORD.EXE", "Rapport")).display, "Word");
        let unknown = understand(&fg("notepad.exe", "Untitled"));
        assert_eq!(
            (unknown.app.as_str(), unknown.kind),
            ("notepad", AppKind::Other)
        );
        assert_eq!(unknown.display, "Notepad");
    }

    #[test]
    fn web_apps_by_the_browser_title() {
        let gmail = understand(&fg(
            "chrome.exe",
            "Inbox (3) - you@example.com - Gmail - Google Chrome",
        ));
        assert_eq!((gmail.app.as_str(), gmail.kind), ("gmail", AppKind::Email));
        let docs = understand(&fg(
            "msedge.exe",
            "Notes de réunion - Google Docs - Microsoft Edge",
        ));
        assert_eq!(
            (docs.app.as_str(), docs.kind),
            ("google-docs", AppKind::Document)
        );
        // A page that merely mentions Slack is not Slack.
        let page = understand(&fg(
            "firefox.exe",
            "Why we left Slackware - Mozilla Firefox",
        ));
        assert_eq!(page.kind, AppKind::Other);
        assert_eq!(page.app, "firefox");
        let video = understand(&fg("chrome.exe", "gmail tips - YouTube - Google Chrome"));
        assert_eq!(video.kind, AppKind::Other);
        let whatsapp = understand(&fg("brave.exe", "(3) WhatsApp - Brave"));
        assert_eq!(whatsapp.kind, AppKind::Chat);
        let discord = understand(&fg(
            "firefox.exe",
            "#general | Hikaya - Discord — Mozilla Firefox",
        ));
        assert_eq!(discord.app, "discord");
    }

    #[test]
    fn a_title_outside_a_browser_is_ignored() {
        assert_eq!(
            understand(&fg("notepad.exe", "Gmail draft.txt")).kind,
            AppKind::Other
        );
    }

    #[test]
    fn prompt_lines_name_the_app_and_stay_empty_when_unknown() {
        let slack = understand(&fg("slack.exe", ""));
        assert!(prompt_line(&slack).contains("chat message in Slack"));
        let terminal = understand(&fg("pwsh.exe", ""));
        assert!(prompt_line(&terminal).contains("lowercase"));
        assert_eq!(prompt_line(&understand(&fg("notepad.exe", ""))), "");
        assert_eq!(prompt_line(&understand(&fg("outlook.exe", ""))), "");
        assert_eq!(prompt_line(&understand(&Foreground::default())), "");
    }
}
