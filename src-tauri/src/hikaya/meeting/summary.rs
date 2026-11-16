//! Summarizing a meeting through the post-processing provider the user set
//! up (Ollama by default). Long transcripts are summarized in parts, then the
//! part summaries together.

use crate::settings::AppSettings;
use hikaya_core::meeting::{plain_transcript, split_for_summary, SpeakerNames, TranscriptLine};

/// Characters of transcript per model call. About 3,000 tokens: small local
/// models (4B, 8K context) keep their instructions in view.
const PART_CHARS: usize = 12_000;

const SUMMARY_PROMPT: &str = include_str!("summary_prompt.txt");
const MERGE_PROMPT: &str = include_str!("merge_prompt.txt");

/// What the summary is made with, from settings.
pub struct SummaryModel {
    provider: crate::settings::PostProcessProvider,
    api_key: String,
    model: String,
}

impl SummaryModel {
    /// The provider and model chosen for post-processing, or why there is none.
    pub fn from_settings(settings: &AppSettings) -> Result<Self, String> {
        let provider = settings
            .active_post_process_provider()
            .cloned()
            .ok_or("choose an AI provider in Post Processing first")?;
        let model = settings
            .post_process_models
            .get(&provider.id)
            .cloned()
            .unwrap_or_default();
        if model.trim().is_empty() {
            return Err(format!(
                "choose a model for {} in Post Processing first",
                provider.label
            ));
        }
        let api_key = settings
            .post_process_api_keys
            .get(&provider.id)
            .cloned()
            .unwrap_or_default();
        Ok(Self {
            provider,
            api_key,
            model,
        })
    }

    /// The model's name, stored with the summary.
    pub fn name(&self) -> &str {
        &self.model
    }

    async fn ask(&self, prompt: String) -> Result<String, String> {
        let disable_reasoning = matches!(self.provider.id.as_str(), "custom" | "openrouter");
        let reply = crate::llm_client::send_chat_completion(
            &self.provider,
            self.api_key.clone(),
            &self.model,
            prompt,
            disable_reasoning,
        )
        .await?
        .ok_or("the AI provider returned nothing")?;
        let reply = strip_think(&reply).trim().to_string();
        if reply.is_empty() {
            return Err("the AI provider returned an empty summary".into());
        }
        Ok(reply)
    }
}

/// Removes a `<think>…</think>` block some models put before their answer.
fn strip_think(text: &str) -> &str {
    match text.find("</think>") {
        Some(end) if text.trim_start().starts_with("<think>") => &text[end + "</think>".len()..],
        _ => text,
    }
}

/// The prompts for one meeting: one per part, and a merge prompt when the
/// transcript has several parts. Separate from the model calls for testing.
pub fn prompts(lines: &[TranscriptLine], names: &SpeakerNames) -> Vec<String> {
    let transcript = plain_transcript(lines, names);
    split_for_summary(&transcript, PART_CHARS)
        .into_iter()
        .map(|part| SUMMARY_PROMPT.replace("${transcript}", &part))
        .collect()
}

/// Summarizes a meeting. Returns Markdown.
pub async fn summarize(
    model: &SummaryModel,
    lines: &[TranscriptLine],
    names: &SpeakerNames,
) -> Result<String, String> {
    if lines.is_empty() {
        return Err("the meeting has no transcript yet".into());
    }
    let mut parts = Vec::new();
    for prompt in prompts(lines, names) {
        parts.push(model.ask(prompt).await?);
    }
    if parts.len() == 1 {
        return Ok(parts.remove(0));
    }
    model
        .ask(MERGE_PROMPT.replace("${summaries}", &parts.join("\n\n---\n\n")))
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use hikaya_core::meeting::Speaker;

    fn names() -> SpeakerNames {
        SpeakerNames {
            you: "You".into(),
            others: "Others".into(),
        }
    }

    #[test]
    fn a_short_meeting_is_one_prompt_with_the_transcript_inside() {
        let lines = [TranscriptLine {
            start_ms: 0,
            end_ms: 3_000,
            speaker: Speaker::Others,
            text: "On décale la démo à jeudi.".into(),
        }];
        let prompts = prompts(&lines, &names());
        assert_eq!(prompts.len(), 1);
        assert!(prompts[0].contains("[00:00] Others: On décale la démo à jeudi."));
        assert!(!prompts[0].contains("${transcript}"));
    }

    #[test]
    fn a_long_meeting_is_split_into_several_prompts() {
        let lines: Vec<TranscriptLine> = (0..400)
            .map(|i| TranscriptLine {
                start_ms: i * 30_000,
                end_ms: i * 30_000 + 30_000,
                speaker: if i % 2 == 0 {
                    Speaker::You
                } else {
                    Speaker::Others
                },
                text: "Une phrase assez longue pour remplir la transcription rapidement.".into(),
            })
            .collect();
        let prompts = prompts(&lines, &names());
        assert!(prompts.len() > 1);
        assert!(prompts
            .iter()
            .all(|p| p.len() < PART_CHARS + SUMMARY_PROMPT.len()));
    }

    #[test]
    fn a_leading_think_block_is_removed() {
        assert_eq!(strip_think("<think>hmm</think>\n## TL;DR"), "\n## TL;DR");
        assert_eq!(strip_think("## TL;DR <think>"), "## TL;DR <think>");
    }

    #[test]
    fn the_prompts_keep_their_placeholders() {
        assert!(SUMMARY_PROMPT.contains("${transcript}"));
        assert!(MERGE_PROMPT.contains("${summaries}"));
    }
}
