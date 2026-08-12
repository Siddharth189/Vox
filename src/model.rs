use serde::{Deserialize, Deserializer, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioChunk {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
}

impl AudioChunk {
    pub fn duration_secs(&self) -> f64 {
        if self.sample_rate == 0 {
            0.0
        } else {
            self.samples.len() as f64 / self.sample_rate as f64
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transcript {
    pub text: String,
    pub language: Option<String>,
}

impl Transcript {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            language: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    #[default]
    Auto,
    Developer,
    Meeting,
    Ticket,
    PrReview,
    Terminal,
    Programming,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    #[default]
    CleanProse,
    Casual,
    ProfessionalEmail,
    CodeContext,
    Shell,
    Markdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Privacy {
    #[default]
    LocalOnly,
    Disabled,
}

impl<'de> Deserialize<'de> for Privacy {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        match s.trim().to_ascii_lowercase().as_str() {
            "local_only" => Ok(Privacy::LocalOnly),
            "disabled" => Ok(Privacy::Disabled),
            other => Err(serde::de::Error::custom(format!(
                "invalid privacy value `{other}`: expected `local_only` or `disabled`"
            ))),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppProfile {
    #[serde(default)]
    pub format: Format,
    #[serde(default)]
    pub privacy: Privacy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppContext {
    pub bundle_id: String,
    pub app_name: String,
    pub window_title: Option<String>,
    pub profile: AppProfile,
}

impl AppContext {
    pub fn unknown() -> Self {
        Self {
            bundle_id: "unknown".into(),
            app_name: "Unknown".into(),
            window_title: None,
            profile: AppProfile::default(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct EnrichedRequest {
    pub transcript: Transcript,
    pub ctx: AppContext,
    pub mode: Mode,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessedText {
    pub text: String,
    pub model_text: Option<String>,
}

impl ProcessedText {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            model_text: None,
        }
    }

    pub fn with_model_text(final_text: impl Into<String>, raw_model_output: impl Into<String>) -> Self {
        Self {
            text: final_text.into(),
            model_text: Some(raw_model_output.into()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InjectionResult {
    pub injected_chars: usize,
    pub strategy: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LatencyMetrics {
    #[serde(default)]
    pub privacy_ms: u64,
    #[serde(default)]
    pub transcribe_ms: u64,
    #[serde(default)]
    pub process_ms: u64,
    #[serde(default)]
    pub inject_ms: u64,
    #[serde(default)]
    pub total_ms: u64,
}

impl LatencyMetrics {
    pub fn prepare_only(privacy_ms: u64, transcribe_ms: u64, process_ms: u64) -> Self {
        Self {
            privacy_ms,
            transcribe_ms,
            process_ms,
            inject_ms: 0,
            total_ms: privacy_ms + transcribe_ms + process_ms,
        }
    }

    pub fn with_injection(mut self, inject_ms: u64) -> Self {
        self.inject_ms = inject_ms;
        self.total_ms = self.privacy_ms + self.transcribe_ms + self.process_ms + inject_ms;
        self
    }
}

pub fn elapsed_ms(d: Duration) -> u64 {
    d.as_millis().min(u64::MAX as u128) as u64
}
