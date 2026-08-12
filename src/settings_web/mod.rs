use std::collections::HashMap;
use std::fs;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::thread;

use axum::body::Body;
use axum::extract::Request;
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;

use crate::config::{self, HotkeyConfig, ProfileConfig, Settings, is_blocked_llm_model};
use crate::history::{self, CorrectionRequest, DictationRecord};
use crate::model::{AppProfile, Mode};
use crate::process::effective_system_prompt;

const INDEX_HTML: &str = include_str!("index.html");
const APP_JS: &str = include_str!("app.js");
const DICTIONARY_JS: &str = include_str!("dictionary.js");
const PROFILES_JS: &str = include_str!("profiles.js");
const HISTORY_JS: &str = include_str!("history.js");

pub fn start_background() {
    thread::spawn(|| {
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                eprintln!("settings web runtime failed: {e}");
                return;
            }
        };
        rt.block_on(async {
            if let Err(e) = serve().await {
                // Port already taken (another Vox) — exit silently
                eprintln!("settings web server not started: {e}");
            }
        });
    });
}

async fn serve() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let app = Router::new()
        .route("/", get(index))
        .route("/app.js", get(|| async { js(APP_JS) }))
        .route("/dictionary.js", get(|| async { js(DICTIONARY_JS) }))
        .route("/profiles.js", get(|| async { js(PROFILES_JS) }))
        .route("/history.js", get(|| async { js(HISTORY_JS) }))
        .route("/api/settings", get(get_settings).post(post_settings))
        .route("/api/models", get(get_models))
        .route("/api/history", get(get_history))
        .route("/api/history/clear", post(clear_history))
        .route("/api/diagnostics/last", get(diagnostics_last))
        .route("/api/learn-correction", post(learn_correction))
        .route(
            "/api/system-message",
            get(get_system_message).post(post_system_message),
        )
        .layer(middleware::from_fn(same_origin));

    let addr = SocketAddr::from(([127, 0, 0, 1], 8722));
    let listener = TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

fn js(body: &'static str) -> Response {
    (
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/javascript; charset=utf-8"),
        )],
        body,
    )
        .into_response()
}

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

async fn same_origin(req: Request<Body>, next: Next) -> Response {
    if let Some(origin) = req.headers().get(header::ORIGIN).and_then(|v| v.to_str().ok()) {
        if origin != "http://127.0.0.1:8722" && origin != "http://localhost:8722" {
            return (StatusCode::FORBIDDEN, "forbidden origin").into_response();
        }
    }
    next.run(req).await
}

async fn get_settings() -> Json<Settings> {
    Json(Settings::load())
}

#[derive(Debug, Deserialize)]
pub struct SettingsPatch {
    pub output_language: Option<String>,
    pub input_language: Option<String>,
    pub whisper_model: Option<String>,
    pub llm_model: Option<String>,
    pub auto_paste: Option<bool>,
    pub hotkey: Option<HotkeyConfig>,
    pub custom_dictionary: Option<Vec<String>>,
    pub custom_aliases: Option<HashMap<String, Vec<String>>>,
    pub profiles: Option<HashMap<String, ProfileConfig>>,
    pub system_message_override: Option<Option<String>>,
}

fn apply_patch(settings: &mut Settings, patch: SettingsPatch) {
    if let Some(v) = patch.output_language {
        settings.output_language = v;
    }
    settings.input_language = patch
        .input_language
        .unwrap_or_else(|| "auto".into());
    if let Some(v) = patch.whisper_model {
        settings.whisper_model = v;
    }
    if let Some(v) = patch.llm_model {
        settings.llm_model = v;
    }
    if let Some(v) = patch.auto_paste {
        settings.auto_paste = v;
    }
    if let Some(v) = patch.hotkey {
        settings.hotkey = v;
    }
    if let Some(v) = patch.custom_dictionary {
        settings.custom_dictionary = v;
    }
    if let Some(v) = patch.custom_aliases {
        settings.custom_aliases = v;
    }
    if let Some(v) = patch.profiles {
        settings.profiles = v;
    }
    if let Some(v) = patch.system_message_override {
        settings.system_message_override = v;
    }
}

async fn post_settings(Json(patch): Json<SettingsPatch>) -> Response {
    if let Some(ref profiles) = patch.profiles {
        if !profiles.contains_key("default") {
            return (
                StatusCode::BAD_REQUEST,
                "profiles must contain a `default` key",
            )
                .into_response();
        }
    }

    let mut settings = Settings::load();
    apply_patch(&mut settings, patch);
    settings.sanitize();
    match settings.save() {
        Ok(()) => (StatusCode::OK, Json(settings)).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[derive(Serialize)]
struct ModelsResponse {
    whisper: Vec<String>,
    ollama: Vec<String>,
}

async fn get_models() -> Json<ModelsResponse> {
    // Blocking HTTP/fs on a dedicated pool — settings server uses a current_thread runtime.
    let result = tokio::task::spawn_blocking(|| ModelsResponse {
        whisper: list_whisper_models(),
        ollama: list_ollama_models(),
    })
    .await;
    Json(result.unwrap_or(ModelsResponse {
        whisper: Vec::new(),
        ollama: Vec::new(),
    }))
}

fn list_whisper_models() -> Vec<String> {
    let mut found = Vec::new();

    let abs_dir = config::model_dir();
    if let Ok(entries) = fs::read_dir(&abs_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("ggml-") && name.ends_with(".bin") {
                found.push(entry.path().to_string_lossy().into_owned());
            }
        }
    }

    let rel = PathBuf::from("models");
    if let Ok(entries) = fs::read_dir(&rel) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("ggml-") && name.ends_with(".bin") {
                found.push(format!("models/{name}"));
            }
        }
    }

    found.sort();
    found.dedup();
    found
}

fn list_ollama_models() -> Vec<String> {
    let client = match reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
    {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let Ok(resp) = client.get("http://127.0.0.1:11434/api/tags").send() else {
        return Vec::new();
    };
    let Ok(body) = resp.json::<serde_json::Value>() else {
        return Vec::new();
    };
    let mut names: Vec<String> = body
        .get("models")
        .and_then(|m| m.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m.get("name").and_then(|n| n.as_str()).map(|s| s.to_string()))
                .filter(|n| !is_blocked_llm_model(n))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

async fn get_history() -> Json<Vec<DictationRecord>> {
    let mut records = history::load_history();
    records.sort_by_key(|r| std::cmp::Reverse(r.created_at_ms));
    Json(records)
}

async fn clear_history() -> StatusCode {
    match history::clear_history() {
        Ok(()) => StatusCode::OK,
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

async fn diagnostics_last() -> Json<Option<DictationRecord>> {
    Json(history::latest_record())
}

async fn learn_correction(Json(req): Json<CorrectionRequest>) -> Response {
    let result = tokio::task::spawn_blocking(move || {
        let mut settings = Settings::load();
        history::learn_from_correction(&mut settings, &req)
    })
    .await;
    match result {
        Ok(Ok(r)) => (StatusCode::OK, Json(r)).into_response(),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[derive(Serialize)]
struct SystemMessageResponse {
    system_message: String,
}

async fn get_system_message() -> Json<SystemMessageResponse> {
    let settings = Settings::load();
    Json(SystemMessageResponse {
        system_message: preview_system_message(&settings),
    })
}

async fn post_system_message(Json(patch): Json<SettingsPatch>) -> Json<SystemMessageResponse> {
    let mut settings = Settings::load();
    apply_patch(&mut settings, patch);
    settings.sanitize();
    Json(SystemMessageResponse {
        system_message: preview_system_message(&settings),
    })
}

fn preview_system_message(settings: &Settings) -> String {
    let profile = settings.profile_for("default");
    effective_system_prompt(
        Mode::Auto,
        &AppProfile {
            format: profile.format,
            privacy: profile.privacy,
        },
        &settings.input_language,
        &settings.output_language,
        &settings.custom_dictionary,
        settings.system_message_override.as_deref(),
    )
}