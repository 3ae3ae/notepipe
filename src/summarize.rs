//! Markdown summaries returned by the language model, without renderer-added labels.
use crate::{
    llm::{self, LlmSettings},
    source::MediaMeta,
    timeline::TranscriptEvent,
};
use anyhow::{Context, Result};

const CHUNK_CHAR_LIMIT: usize = 25_000;

fn transcript(events: &[TranscriptEvent]) -> String {
    events
        .iter()
        .map(|event| format!("[{}] {}\n", crate::render::fmt_ts(event.start), event.text))
        .collect()
}

fn summarize_text(agent: &ureq::Agent, settings: &LlmSettings, text: &str) -> Result<String> {
    let system = format!(
        "Summarize the supplied transcript in Markdown, using its original language. Use only facts supported by the transcript. Return the summary directly, without a code fence. {}",
        llm::MATH_RULE
    );
    let mut body = llm::chat_body(settings, &system, text, llm::CHAT_MAX_TOKENS);
    body.as_object_mut().unwrap().remove("response_format");
    llm::send_chat(agent, settings, &body, llm::Purpose::Summary, &|content| {
        anyhow::ensure!(!content.trim().is_empty(), "Empty summary response");
        Ok(())
    })
}

pub async fn summarize(
    settings: &LlmSettings,
    events: &[TranscriptEvent],
    meta: &MediaMeta,
) -> Result<String> {
    anyhow::ensure!(!events.is_empty(), "Transcript is empty; cannot summarize");
    llm::validate(settings)?;
    let mut chunks = Vec::new();
    let mut text = String::new();
    for event in events {
        let line = transcript(std::slice::from_ref(event));
        if !text.is_empty() && text.chars().count() + line.chars().count() > CHUNK_CHAR_LIMIT {
            chunks.push(std::mem::take(&mut text));
        }
        text.push_str(&line);
    }
    if !text.is_empty() {
        chunks.push(text);
    }
    let settings = settings.clone();
    let context = format!("Title: {}\nAuthor: {}\n\n", meta.title, meta.author);
    tokio::task::spawn_blocking(move || {
        let agent = llm::chat_agent();
        let partials = crate::runtime::map_pool(chunks, 4, |chunk| {
            summarize_text(&agent, &settings, &format!("{context}{chunk}"))
        })?;
        if partials.len() == 1 {
            return Ok(partials.into_iter().next().unwrap());
        }
        summarize_text(
            &agent,
            &settings,
            &format!(
                "Merge the following summaries into one Markdown summary.\n\n{context}{}",
                partials.join("\n\n")
            ),
        )
    })
    .await
    .context("Summary generation was interrupted")?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn empty_transcript_bails_without_an_http_request() {
        let meta = MediaMeta {
            title: "Title".into(),
            author: String::new(),
            source: String::new(),
            duration: 0.,
        };
        assert!(
            summarize(&LlmSettings::default(), &[], &meta)
                .await
                .unwrap_err()
                .to_string()
                .contains("Transcript is empty")
        );
    }
}
