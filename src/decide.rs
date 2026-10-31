//! The slow tier: asking a model what to do next.
//!
//! Everything here exists to be used once. Its output is not the answer to a task, it is
//! a step of a program that will afterwards run without it.

use crate::cdp::Element;
use crate::state::as_table;
use anyhow::{anyhow, Result};
use serde_json::Value;
use std::time::Instant;

/// Where the model is reached. Overridable so the same runtime can be pointed at
/// Anthropic credits, a local model, or anything else that speaks this shape.
fn endpoint() -> String {
    std::env::var("MINT_MODEL_URL")
        .unwrap_or_else(|_| "https://openrouter.ai/api/v1/chat/completions".to_string())
}

fn model_name() -> String {
    std::env::var("MINT_MODEL").unwrap_or_else(|_| "google/gemini-2.5-flash".to_string())
}

/// Read the key from the environment files without ever putting it in a log or an
/// argument list.
fn api_key() -> Result<String> {
    // The environment first, under every name a model key is normally given, so that a checkout on
    // another machine works by setting one variable and nothing else.
    for name in ["MINT_API_KEY", "OPENROUTER_API_KEY", "ANTHROPIC_API_KEY"] {
        if let Ok(k) = std::env::var(name) {
            if !k.trim().is_empty() {
                return Ok(k);
            }
        }
    }
    // Then files, nearest first: beside where mint is run, then the user's own directory, and last
    // the paths this machine happens to keep keys in.
    let mut candidates = vec![".env".to_string()];
    if let Ok(home) = std::env::var("HOME") {
        candidates.push(format!("{home}/.mint/env"));
    }
    candidates.push("/root/.hermes/profiles/work2/.env".to_string());
    candidates.push("/root/.hermes/.env".to_string());
    for p in candidates {
        if let Ok(text) = std::fs::read_to_string(&p) {
            for line in text.lines() {
                for name in ["OPENROUTER_API_KEY=", "MINT_API_KEY=", "ANTHROPIC_API_KEY="] {
                    if let Some(rest) = line.strip_prefix(name) {
                        let v = rest.trim().trim_matches('"').trim_matches('\'');
                        if !v.is_empty() {
                            return Ok(v.to_string());
                        }
                    }
                }
            }
        }
    }
    Err(anyhow!(
        "no model key found. Set MINT_API_KEY in the environment, or put \
         OPENROUTER_API_KEY=... in a file called .env next to where mint is run"
    ))
}

/// Which request shape the configured endpoint speaks.
///
/// The model is a part, like the eyes and hands are. Two shapes cover almost everything in
/// practice: the OpenAI one (used by gateways and most hosted models) and Anthropic's native
/// one, which differs in the address, the auth header and where the answer lives. Pretending a
/// key alone is enough would be a claim the code does not honour.
#[derive(PartialEq, Clone, Copy)]
enum Api {
    OpenAi,
    Anthropic,
}

fn api_kind() -> Api {
    if let Ok(v) = std::env::var("MINT_MODEL_API") {
        if v.eq_ignore_ascii_case("anthropic") {
            return Api::Anthropic;
        }
        if v.eq_ignore_ascii_case("openai") {
            return Api::OpenAi;
        }
    }
    if endpoint().contains("anthropic.com") {
        Api::Anthropic
    } else {
        Api::OpenAi
    }
}

/// Ask the configured model one question and hand back its text. Both callers in this module
/// go through here, so pointing the runtime at a different provider is one setting and not a
/// rewrite.
fn ask(prompt: &str, max_tokens: u32) -> Result<String> {
    let key = api_key()?;
    let model = model_name();
    let resp: Value = match api_kind() {
        Api::OpenAi => {
            let body = serde_json::json!({
                "model": model,
                "messages": [{ "role": "user", "content": prompt }],
                "max_tokens": max_tokens,
                "temperature": 0.0,
                "response_format": { "type": "json_object" }
            });
            ureq::post(&endpoint())
                .set("Authorization", &format!("Bearer {key}"))
                .set("Content-Type", "application/json")
                .send_json(body)?
                .into_json()?
        }
        Api::Anthropic => {
            // No JSON mode here: the reply is read by taking the first object out of it, which
            // is what the callers already do, so nothing is lost by leaving it off.
            let body = serde_json::json!({
                "model": model,
                "max_tokens": max_tokens,
                "temperature": 0.0,
                "messages": [{ "role": "user", "content": prompt }]
            });
            ureq::post(&endpoint())
                .set("x-api-key", &key)
                .set("anthropic-version", "2023-06-01")
                .set("Content-Type", "application/json")
                .send_json(body)?
                .into_json()?
        }
    };
    let text = match api_kind() {
        Api::OpenAi => resp.pointer("/choices/0/message/content").and_then(Value::as_str),
        Api::Anthropic => resp.pointer("/content/0/text").and_then(Value::as_str),
    };
    text.map(str::to_string)
        .ok_or_else(|| anyhow!("model returned nothing usable"))
}

pub struct Decision {
    /// Index into the current state, or None to stop.
    pub idx: Option<usize>,
    pub intent: String,
    /// The words this element should be recognised by next time.
    pub selector: String,
    /// What to type, when the step is a text field.
    pub text: Option<String>,
    pub confidence: f32,
    pub latency_us: u64,
}

/// Ask for exactly one next step. The reply is constrained to a shape we can check, and
/// anything that does not check is treated as a failure rather than acted upon.
pub fn next_step(goal: &str, url: &str, state: &[Element], trail: &[String], category: &str) -> Result<Decision> {
    let started = Instant::now();
    let mut prompt = String::new();
    prompt.push_str(&format!(
        "You are driving a browser. Goal: {goal}\nTask category: {category}\nCurrent address: {url}\n\n\
         What is on screen (interactive elements):\n{}\n",
        as_table(state)
    ));
    if !trail.is_empty() {
        prompt.push_str("\nWhat has already been done:\n");
        for t in trail.iter().rev().take(6).rev() {
            prompt.push_str(&format!("- {t}\n"));
        }
    }
    prompt.push_str(
        "\nChoose ONE next step. Reply with JSON only:\n\
         {\"idx\": <number>, \"selector\": \"<the words by which this element is recognised on a page of the same kind with different data, up to 6 words>\", \"text\": \"<if this is an input field, what to type, otherwise empty>\", \"intent\": \"<what you are doing, up to 12 words>\", \"confidence\": <0..1>}\n\
         If the goal is already reached — {\"idx\": -1, \"intent\": \"done\", \"confidence\": 1.0}.\n\
         If no element fits — {\"idx\": -2, \"intent\": \"a different approach is needed\", \"confidence\": 0.2}.",
    );

    let content = ask(&prompt, 200)?;
    let latency_us = started.elapsed().as_micros() as u64;

    // The model may wrap its JSON in prose or a code fence; take the first object.
    let cleaned = extract_object(&content).ok_or_else(|| anyhow!("reply was not an object: {}", &content[..content.len().min(120)]))?;
    let parsed: Value = serde_json::from_str(&cleaned)?;
    let raw_idx = parsed.get("idx").and_then(Value::as_i64).ok_or_else(|| anyhow!("reply has no idx"))?;
    let idx = match raw_idx {
        -1 | -2 => None,
        n if n >= 0 && (n as usize) < state.len() => Some(n as usize),
        other => return Err(anyhow!("chose an element that is not on screen: {other}")),
    };
    Ok(Decision {
        idx,
        intent: parsed.get("intent").and_then(Value::as_str).unwrap_or("").to_string(),
        selector: parsed.get("selector").and_then(Value::as_str).unwrap_or("").to_string(),
        text: parsed
            .get("text")
            .and_then(Value::as_str)
            .map(|s| s.to_string())
            .filter(|s| !s.trim().is_empty()),
        confidence: parsed.get("confidence").and_then(Value::as_f64).unwrap_or(0.5) as f32,
        latency_us,
    })
}

fn extract_object(s: &str) -> Option<String> {
    let start = s.find('{')?;
    let mut depth = 0i32;
    for (i, c) in s[start..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(s[start..start + i + 1].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

/// Judge the outcome against the goal. One question, asked once, at the end of a learning
/// run: the final page either satisfies what was asked or it does not. Without this the
/// only thing a skill could be checked against would be its own recording, which proves
/// nothing.
pub fn judge_success(goal: &str, url: &str, title: &str, text: &str) -> Result<(bool, String)> {
    let excerpt: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let excerpt: String = excerpt.chars().take(600).collect();
    let prompt = format!(
        "The task that was set: {goal}\n\nWhere we ended up:\naddress: {url}\ntitle: {title}\n\
         what is visible on the page: {excerpt}\n\nWas the task reached? Reply with JSON only:\
         {{\"success\": true or false, \"why\": \"<up to 15 words>\"}}"
    );
    let content = ask(&prompt, 120)?;
    let cleaned = extract_object(&content).ok_or_else(|| anyhow!("verdict was not an object"))?;
    let v: Value = serde_json::from_str(&cleaned)?;
    Ok((
        v.get("success").and_then(Value::as_bool).unwrap_or(false),
        v.get("why").and_then(Value::as_str).unwrap_or("").to_string(),
    ))
}
