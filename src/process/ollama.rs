use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::config::{LlmProvider, Settings};
use crate::error::{Result, VoxError};
use crate::model::{EnrichedRequest, ProcessedText};
use crate::process::dictionary::normalize_processed_text;
use crate::process::prompt::{
    code_switch_fewshot, effective_system_prompt, fewshot, rewrite_only_user_message,
};
use crate::process::TextProcessor;

pub const DEFAULT_MODEL: &str = "llama3.2:latest";
/// Optional bearer token for OpenAI-compatible servers that require one.
/// Read from the environment so it never lands in settings.yaml.
const API_KEY_ENV: &str = "VOX_LLM_API_KEY";
/// Set to 1 to allow an LLM server that isn't on this machine.
const ALLOW_REMOTE_ENV: &str = "VOX_ALLOW_REMOTE_LLM";
/// A dictation that takes longer than this to polish is better delivered
/// with local cleanup than not at all.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_millis(500);
const PROBE_TIMEOUT: Duration = Duration::from_millis(1500);
const KEEP_ALIVE_SECONDS: i64 = -1;
const SEED: i64 = 42;
const NUM_CTX: i64 = 4096;
const MIN_NUM_PREDICT: usize = 128;
const MAX_NUM_PREDICT: usize = 1536;

pub fn scale_num_predict(word_count: usize) -> usize {
    (word_count * 3).clamp(MIN_NUM_PREDICT, MAX_NUM_PREDICT)
}

#[cfg(target_os = "macos")]
fn low_resource_mode() -> bool {
    false
}

#[cfg(not(target_os = "macos"))]
fn low_resource_mode() -> bool {
    crate::config::low_resource_mode()
}

/// LLM cleanup through a local server: Ollama's native API, or any
/// OpenAI-compatible one (LM Studio, llama.cpp's llama-server, Jan, vLLM).
pub struct OllamaProcessor {
    provider: LlmProvider,
    host: String,
    model: String,
    input_language: String,
    output_language: String,
    custom_dictionary: Vec<String>,
    custom_aliases: HashMap<String, Vec<String>>,
    system_message_override: Option<String>,
    client: reqwest::blocking::Client,
}

impl OllamaProcessor {
    pub fn from_settings(settings: &Settings) -> Result<Self> {
        let model = std::env::var("VOX_MODEL").unwrap_or_else(|_| settings.llm_model.clone());
        let processor = Self::new(
            settings.llm_base_url(),
            model,
            &settings.input_language,
            &settings.output_language,
            settings.custom_dictionary.clone(),
            settings.custom_aliases.clone(),
            settings.system_message_override.clone(),
        )?;
        Ok(processor.with_provider(settings.llm_provider))
    }

    pub fn new(
        host: impl Into<String>,
        model: impl Into<String>,
        input_language: &str,
        output_language: &str,
        custom_dictionary: Vec<String>,
        custom_aliases: HashMap<String, Vec<String>>,
        system_message_override: Option<String>,
    ) -> Result<Self> {
        let client = reqwest::blocking::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .map_err(|e| VoxError::Processing(e.to_string()))?;
        Ok(Self {
            provider: LlmProvider::Ollama,
            host: host.into(),
            model: model.into(),
            input_language: input_language.into(),
            output_language: output_language.into(),
            custom_dictionary,
            custom_aliases,
            system_message_override,
            client,
        })
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    pub fn with_provider(mut self, provider: LlmProvider) -> Self {
        self.provider = provider;
        self
    }

    pub fn provider(&self) -> LlmProvider {
        self.provider
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    fn build_messages(&self, raw: &str, req: &EnrichedRequest) -> Vec<ChatMessage> {
        let system = effective_system_prompt(
            req.mode,
            &req.ctx.profile,
            &self.input_language,
            &self.output_language,
            &self.custom_dictionary,
            self.system_message_override.as_deref(),
        );

        let mut messages = vec![ChatMessage {
            role: "system".into(),
            content: system,
        }];

        let input_lower = self.input_language.to_ascii_lowercase();
        let mixed_input = input_lower.contains("hinglish")
            || input_lower.contains("mixed")
            || input_lower.contains("code-switch");
        let out_trim = self.output_language.trim();
        let translating = !out_trim.is_empty() && !out_trim.eq_ignore_ascii_case("auto");
        let targeted_code_switch =
            out_trim.eq_ignore_ascii_case("English") || out_trim.eq_ignore_ascii_case("Hindi");

        let examples = if mixed_input || targeted_code_switch {
            // Keep these even in low-resource mode: Hinglish/code-switch handling
            // is the case the small model needs the most guidance on.
            code_switch_fewshot(&self.output_language)
        } else if !translating && !low_resource_mode() {
            fewshot()
        } else {
            vec![]
        };

        for (user, assistant) in examples {
            messages.push(ChatMessage {
                role: "user".into(),
                content: user,
            });
            messages.push(ChatMessage {
                role: "assistant".into(),
                content: assistant,
            });
        }

        messages.push(ChatMessage {
            role: "user".into(),
            content: rewrite_only_user_message(raw),
        });

        messages
    }

    fn chat(&self, messages: &[ChatMessage], num_predict: usize) -> Result<String> {
        let (url, body) = match self.provider {
            LlmProvider::Ollama => (
                format!("{}/api/chat", self.host.trim_end_matches('/')),
                build_request_body(&self.model, messages, num_predict),
            ),
            LlmProvider::OpenaiCompatible => (
                openai_url(&self.host, "/chat/completions"),
                build_openai_request_body(&self.model, messages, num_predict),
            ),
        };
        let resp = with_api_key(self.client.post(&url), self.provider)
            .json(&body)
            .send()
            .map_err(|e| VoxError::Processing(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(VoxError::Processing(format!(
                "{} returned HTTP {}",
                self.provider.label(),
                resp.status()
            )));
        }

        match self.provider {
            LlmProvider::Ollama => {
                let parsed: ChatResponse = resp
                    .json()
                    .map_err(|e| VoxError::Processing(e.to_string()))?;
                Ok(parsed.message.content)
            }
            LlmProvider::OpenaiCompatible => {
                let parsed: OpenAiChatResponse = resp
                    .json()
                    .map_err(|e| VoxError::Processing(e.to_string()))?;
                parsed
                    .choices
                    .into_iter()
                    .next()
                    .map(|c| c.message.content)
                    .ok_or_else(|| VoxError::Processing("server returned no choices".into()))
            }
        }
    }
}

/// `{base}/v1{path}`, accepting a base URL given with or without `/v1`
/// (LM Studio shows it with, llama-server's docs without).
pub fn openai_url(base: &str, path: &str) -> String {
    let base = base.trim_end_matches('/');
    if base.ends_with("/v1") {
        format!("{base}{path}")
    } else {
        format!("{base}/v1{path}")
    }
}

fn with_api_key(
    req: reqwest::blocking::RequestBuilder,
    provider: LlmProvider,
) -> reqwest::blocking::RequestBuilder {
    match std::env::var(API_KEY_ENV) {
        Ok(key) if provider == LlmProvider::OpenaiCompatible && !key.trim().is_empty() => {
            req.bearer_auth(key.trim())
        }
        _ => req,
    }
}

/// True for URLs whose host is this machine.
pub fn is_loopback_url(url: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return false;
    };
    let Some(host) = parsed.host_str() else {
        return false;
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// Vox's promise is that dictations stay on this machine, so an LLM server
/// elsewhere is ignored unless the user opts in explicitly.
pub fn llm_url_permitted(url: &str) -> bool {
    is_loopback_url(url) || std::env::var(ALLOW_REMOTE_ENV).is_ok_and(|v| v.trim() == "1")
}

/// Model names the server offers, or None if it isn't reachable.
pub fn list_models(provider: LlmProvider, host: &str) -> Option<Vec<String>> {
    let client = reqwest::blocking::Client::builder()
        .timeout(PROBE_TIMEOUT)
        .connect_timeout(CONNECT_TIMEOUT)
        .build()
        .ok()?;
    let (url, list_key, name_key) = match provider {
        LlmProvider::Ollama => (
            format!("{}/api/tags", host.trim_end_matches('/')),
            "models",
            "name",
        ),
        LlmProvider::OpenaiCompatible => (openai_url(host, "/models"), "data", "id"),
    };
    let resp = with_api_key(client.get(url), provider).send().ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let body = resp.json::<serde_json::Value>().ok()?;
    Some(
        body.get(list_key)
            .and_then(|m| m.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| m.get(name_key).and_then(|n| n.as_str()))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
    )
}

/// True when the server answers at `host` (and is allowed to be used) and
/// has `model` available.
pub fn probe(provider: LlmProvider, host: &str, model: &str) -> bool {
    llm_url_permitted(host)
        && list_models(provider, host)
            .is_some_and(|names| names.iter().any(|n| crate::doctor::model_matches(n, model)))
}

fn build_request_body(
    model: &str,
    messages: &[ChatMessage],
    num_predict: usize,
) -> serde_json::Value {
    json!({
        "model": model,
        "messages": messages,
        "stream": false,
        "keep_alive": KEEP_ALIVE_SECONDS,
        "options": {
            "temperature": 0.0,
            "seed": SEED,
            "num_predict": num_predict,
            "num_ctx": NUM_CTX,
        }
    })
}

fn build_openai_request_body(
    model: &str,
    messages: &[ChatMessage],
    num_predict: usize,
) -> serde_json::Value {
    json!({
        "model": model,
        "messages": messages,
        "stream": false,
        "temperature": 0.0,
        "seed": SEED,
        "max_tokens": num_predict,
    })
}

impl TextProcessor for OllamaProcessor {
    fn process(&self, req: &EnrichedRequest) -> Result<ProcessedText> {
        let raw = req.transcript.text.as_str();
        if raw.trim().is_empty() {
            return Ok(ProcessedText::new(""));
        }
        if !llm_url_permitted(&self.host) {
            return Err(VoxError::Processing(format!(
                "{} is not on this machine; set {ALLOW_REMOTE_ENV}=1 to allow it",
                self.host
            )));
        }

        let word_count = raw.split_whitespace().count();
        let num_predict = scale_num_predict(word_count);
        let messages = self.build_messages(raw, req);
        let resp = self.chat(&messages, num_predict)?;
        let cleaned = post_process_model_output(&resp);
        let final_text =
            normalize_processed_text(&cleaned, &self.custom_dictionary, &self.custom_aliases);
        Ok(ProcessedText::with_model_text(final_text, cleaned))
    }

    fn name(&self) -> &'static str {
        "ollama"
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    message: ChatMessageContent,
}

#[derive(Debug, Deserialize)]
struct ChatMessageContent {
    content: String,
}

#[derive(Debug, Deserialize)]
struct OpenAiChatResponse {
    choices: Vec<OpenAiChoice>,
}

#[derive(Debug, Deserialize)]
struct OpenAiChoice {
    message: ChatMessageContent,
}

pub fn post_process_model_output(resp: &str) -> String {
    let t = resp.trim();
    strip_wrapping(t)
}

fn strip_wrapping(text: &str) -> String {
    let t = text.trim();
    if let Some(inner) = strip_fenced_code(t) {
        return strip_model_preface(&inner);
    }
    if t.len() >= 2 && t.starts_with('"') && t.ends_with('"') {
        return strip_model_preface(t[1..t.len() - 1].trim());
    }
    strip_model_preface(t)
}

fn strip_fenced_code(text: &str) -> Option<String> {
    let t = text.trim();
    if t.len() < 6 || !(t.starts_with("```") && t.ends_with("```")) {
        return None;
    }
    let without = &t[3..t.len() - 3];
    let without = without.trim();
    // Drop optional first-line language tag
    let body = if let Some((first, rest)) = without.split_once('\n') {
        if first
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-')
        {
            rest.trim()
        } else {
            without
        }
    } else {
        without
    };
    Some(body.to_string())
}

fn strip_model_preface(text: &str) -> String {
    const PREFIXES: &[&str] = &[
        "here's a rewritten version of the dictated speech:",
        "here is a rewritten version of the dictated speech:",
        "here's a rewritten version:",
        "here is a rewritten version:",
        "here's the rewritten version:",
        "here is the rewritten version:",
        "rewritten version:",
        "corrected text:",
        "final text:",
    ];
    let lower = text.to_ascii_lowercase();
    for prefix in PREFIXES {
        if lower.starts_with(prefix) {
            return text[prefix.len()..].trim_start().to_string();
        }
    }
    strip_preamble(text)
}

fn strip_preamble(text: &str) -> String {
    let first = text.lines().next().unwrap_or(text);
    if let Some((intro, _)) = first.split_once(':') {
        if looks_like_preamble_intro(intro) {
            return text[intro.len() + 1..].trim_start().to_string();
        }
    }
    text.to_string()
}

fn looks_like_preamble_intro(intro: &str) -> bool {
    let lower = intro.trim().to_ascii_lowercase();
    // Smaller/weaker models (observed with a 1B-class model in practice, not
    // just theoretically) don't reliably stick to "Here is/Here's" - they
    // also preface with "I'll ..." or "I will ...", e.g. "I'll provide the
    // corrected text:" or "I'll provide the corrected text without
    // answering or fulfilling any requests:". Same keyword-gated approach
    // as the "here is" case, just with a broader set of accepted openers.
    const OPENERS: &[&str] = &["here is", "here's", "i'll", "i will"];
    if !OPENERS.iter().any(|o| lower.starts_with(o)) {
        return false;
    }
    if intro.chars().count() > 80 {
        return false;
    }
    const NEEDLES: &[&str] = &[
        "rewritten",
        "rewrite",
        "cleaned",
        "edited",
        "corrected",
        "step-by-step",
        "step by step",
        "approach",
        "version",
        "provide",
    ];
    NEEDLES.iter().any(|n| lower.contains(n))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_predict_clamps() {
        assert_eq!(scale_num_predict(1), 128);
        assert_eq!(scale_num_predict(100), 300);
        assert_eq!(scale_num_predict(10_000), 1536);
    }

    #[test]
    fn request_body_keys() {
        // Exercises the same build_request_body() the real chat() call uses,
        // so a regression that drops/renames a key here fails this test too.
        let body = build_request_body("llama3.2:latest", &[], 128);
        let obj = body.as_object().unwrap();
        assert_eq!(obj["model"], "llama3.2:latest");
        assert_eq!(obj["stream"], false);
        assert!(obj.contains_key("keep_alive"));
        assert_eq!(obj["keep_alive"], -1);
        assert_eq!(obj["options"]["temperature"], 0.0);
        assert_eq!(obj["options"]["seed"], 42);
        assert_eq!(obj["options"]["num_predict"], 128);
        assert_eq!(obj["options"]["num_ctx"], 4096);
    }

    #[test]
    fn openai_request_and_response_shapes() {
        let body = build_openai_request_body("local-model", &[], 200);
        assert_eq!(body["model"], "local-model");
        assert_eq!(body["stream"], false);
        assert_eq!(body["max_tokens"], 200);
        assert_eq!(body["temperature"], 0.0);
        assert!(body.get("options").is_none());

        let parsed: OpenAiChatResponse = serde_json::from_str(
            r#"{"id":"x","choices":[{"index":0,"message":{"role":"assistant","content":"Hello."}}]}"#,
        )
        .unwrap();
        assert_eq!(parsed.choices[0].message.content, "Hello.");
    }

    #[test]
    fn openai_url_accepts_base_with_or_without_v1() {
        assert_eq!(
            openai_url("http://127.0.0.1:1234", "/models"),
            "http://127.0.0.1:1234/v1/models"
        );
        assert_eq!(
            openai_url("http://localhost:8080/v1/", "/chat/completions"),
            "http://localhost:8080/v1/chat/completions"
        );
    }

    #[test]
    fn only_loopback_servers_count_as_local() {
        assert!(is_loopback_url("http://127.0.0.1:11434"));
        assert!(is_loopback_url("http://localhost:1234/v1"));
        assert!(is_loopback_url("http://[::1]:8080"));
        assert!(is_loopback_url("http://127.0.0.2:8080"));
        assert!(!is_loopback_url("http://192.168.1.20:11434"));
        assert!(!is_loopback_url("https://api.example.com/v1"));
        assert!(!is_loopback_url("http://localhost.example.com"));
        assert!(!is_loopback_url("not a url"));
    }

    #[test]
    fn strip_fenced_code_handles_degenerate_input_without_panicking() {
        assert_eq!(post_process_model_output("```"), "```");
        assert_eq!(post_process_model_output("````"), "````");
        assert_eq!(post_process_model_output("`````"), "`````");
        assert_eq!(post_process_model_output("``````"), "");
    }

    #[test]
    fn strip_preface_and_fence() {
        assert_eq!(
            post_process_model_output("Here's a rewritten version:\nHello there."),
            "Hello there."
        );
        assert_eq!(post_process_model_output("```\nHello\n```"), "Hello");
        assert_eq!(
            post_process_model_output("Here is the deal: we ship Friday"),
            "Here is the deal: we ship Friday"
        );
    }

    #[test]
    fn strips_ill_provide_preamble() {
        // Reproduced against a real 1B-class model in practice: it
        // regularly prefaces with "I'll ..." instead of "Here is/Here's".
        assert_eq!(
            post_process_model_output(
                "I'll provide the corrected text:\nCan you review PR #684 after lunch?"
            ),
            "Can you review PR #684 after lunch?"
        );
        assert_eq!(
            post_process_model_output(
                "I'll provide the corrected text without answering or fulfilling any requests:\nHello there."
            ),
            "Hello there."
        );
        // Genuine content starting with "I'll" and no preamble keywords
        // must not be touched.
        assert_eq!(
            post_process_model_output("I'll be at the office by 3pm: see you then"),
            "I'll be at the office by 3pm: see you then"
        );
    }
}
