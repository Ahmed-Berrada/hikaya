//! Summarizing a meeting through the post-processing provider the user set
//! up (Ollama by default). Long transcripts are summarized in parts, then the
//! part summaries together.

use crate::settings::AppSettings;
use hikaya_core::meeting::{
    merge_prompt, strip_think, summary_prompts, SpeakerNames, TranscriptLine,
};

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
    for prompt in summary_prompts(lines, names) {
        parts.push(model.ask(prompt).await?);
    }
    if parts.len() == 1 {
        return Ok(parts.remove(0));
    }
    model.ask(merge_prompt(lines, &parts)).await
}
