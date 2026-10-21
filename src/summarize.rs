//! Whole-transcript stages, with bounded chunks and ordered map/reduce.
use crate::{
    llm::{self, LlmSettings, Stage},
    source::MediaMeta,
    timeline::TranscriptEvent,
};
use anyhow::{Context, Result};

pub(crate) const SUMMARY_PROMPT: &str = r#"You summarize recordings from their timestamped transcript. Use only what the transcript says: never invent facts, numbers, names or opinions, and leave out anything uncertain rather than guess. Write a one-sentence overview, a list of 3 to 6 key points, and an outline of 3 to 8 sections in time order that covers the whole recording; start each outline item with the section's start time as [mm:ss] and describe the section in one or two sentences. Write mathematical formulas as LaTeX in $...$ or $$...$$. Use Markdown with headings of level 2 or lower, and do not repeat the title."#;
pub(crate) const SUMMARY_LANGUAGE: &str = r#"Write the summary in the language of the transcript."#;
pub(crate) const SUMMARY_CONTRACT: &str =
    r#"Output only the Markdown, without code fences or any other text."#;
pub(crate) const SUMMARY_USER: &str = r#"Transcript (each line starts with its start time as [mm:ss]):

{}"#;
pub(crate) const SUMMARY_MERGE_USER: &str = r#"Below are summaries of consecutive parts of one recording, in order. Their times are already measured from the start of the recording.

{}

Merge them into one summary of the whole recording, following the same instructions."#;
const CHUNK_CHAR_LIMIT: usize = 25_000;

fn system(stage: &Stage) -> String {
    if stage.name == "summary" {
        format!("{} {SUMMARY_LANGUAGE} {SUMMARY_CONTRACT}", stage.prompt)
    } else {
        format!("{} {SUMMARY_CONTRACT}", stage.prompt)
    }
}
fn send(agent: &ureq::Agent, settings: &LlmSettings, stage: &Stage, user: &str) -> Result<String> {
    let body = llm::chat_body(settings, &system(stage), user, stage.input)?;
    llm::send_chat(agent, settings, &body, &stage.name, &|content| {
        anyhow::ensure!(
            !content.trim().is_empty(),
            "Empty transcript-stage response"
        );
        Ok(())
    })
}
fn context(meta: &MediaMeta) -> String {
    let mut context = String::new();
    if !meta.title.trim().is_empty() {
        context.push_str(&format!("Title: {}\n", meta.title));
    }
    if !meta.author.trim().is_empty() {
        context.push_str(&format!("Author: {}\n", meta.author));
    }
    if !context.is_empty() {
        context.push('\n');
    }
    context
}
// shortcut: a single paragraph can exceed the chunk limit, split paragraphs if model context limits require it.
fn chunks(events: &[TranscriptEvent]) -> Vec<String> {
    let (mut chunks, mut text, mut chars) = (Vec::new(), String::new(), 0);
    for event in events {
        let line = format!("[{}] {}\n", crate::render::fmt_ts(event.start), event.text);
        let count = line.chars().count();
        if !text.is_empty() && chars + count > CHUNK_CHAR_LIMIT {
            chunks.push(std::mem::take(&mut text));
            chars = 0;
        }
        text.push_str(&line);
        chars += count;
    }
    if !text.is_empty() {
        chunks.push(text);
    }
    chunks
}
pub async fn run(
    settings: &LlmSettings,
    stage: &Stage,
    events: &[TranscriptEvent],
    meta: &MediaMeta,
) -> Result<String> {
    anyhow::ensure!(
        !events.is_empty(),
        "Transcript is empty; cannot run a transcript stage"
    );
    let (settings, stage, context, chunks) = (
        settings.clone(),
        stage.clone(),
        context(meta),
        chunks(events),
    );
    tokio::task::spawn_blocking(move || {
        let agent = llm::chat_agent();
        let partials = crate::runtime::map_pool(chunks, settings.concurrency, |chunk| {
            send(
                &agent,
                &settings,
                &stage,
                &format!("{context}{}", SUMMARY_USER.replace("{}", &chunk)),
            )
        })?;
        if partials.len() == 1 {
            return Ok(partials.into_iter().next().unwrap());
        }
        let mut parts = String::new();
        for (index, text) in partials.iter().enumerate() {
            parts.push_str(&format!("== Part {} ==\n{text}\n\n", index + 1));
        }
        send(
            &agent,
            &settings,
            &stage,
            &format!("{context}{}", SUMMARY_MERGE_USER.replace("{}", &parts)),
        )
    })
    .await
    .context("Transcript stage interrupted")?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn built_in_and_custom_transcript_contracts_do_not_override_user_instructions() {
        let mut stage = Stage::builtin("summary").unwrap();
        assert_eq!(
            system(&stage),
            format!("{SUMMARY_PROMPT} {SUMMARY_LANGUAGE} {SUMMARY_CONTRACT}")
        );
        stage.name = "outline".into();
        stage.prompt = "CUSTOM".into();
        assert_eq!(system(&stage), format!("CUSTOM {SUMMARY_CONTRACT}"));
        let events = (0..3)
            .map(|index| TranscriptEvent {
                start: f64::from(index * 60),
                end: f64::from(index * 60 + 1),
                text: "x".repeat(CHUNK_CHAR_LIMIT - 10),
                raw: None,
                below: Vec::new(),
            })
            .collect::<Vec<_>>();
        let chunks = chunks(&events);
        assert_eq!(chunks.len(), 3);
        assert!(chunks[0].starts_with("[00:00] "));
        assert!(chunks[1].starts_with("[01:00] "));
        assert!(chunks[2].starts_with("[02:00] "));
    }

    #[tokio::test]
    async fn empty_transcript_bails_without_an_http_request() {
        let meta = MediaMeta {
            title: "Title".into(),
            author: String::new(),
            source: String::new(),
            duration: 0.,
            seek_url: None,
        };
        let stage = Stage::builtin("summary").unwrap();
        assert!(
            run(&LlmSettings::default(), &stage, &[], &meta)
                .await
                .unwrap_err()
                .to_string()
                .contains("Transcript is empty")
        );
    }
}
