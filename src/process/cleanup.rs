//! The processor the tray actually uses: the local LLM when it's really
//! there, the rule-based cleanup otherwise.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::config::{LlmProvider, Settings};
use crate::error::Result;
use crate::model::{EnrichedRequest, ProcessedText};
use crate::process::local::LocalCleanup;
use crate::process::ollama::{probe, OllamaProcessor};
use crate::process::TextProcessor;

/// How long a probe result is trusted before re-checking in the background.
const PROBE_TTL: Duration = Duration::from_secs(20);

/// Cached "is Ollama up with this model pulled?" answer.
///
/// Never probed on the dictation's critical path once a result exists: on
/// Windows a connect() to a closed localhost port doesn't fail fast, it
/// retries SYNs for ~2 s, which would be added to every dictation for anyone
/// who doesn't run Ollama.
pub struct Availability {
    provider: LlmProvider,
    host: String,
    model: String,
    state: Mutex<Option<(bool, Instant)>>,
    probing: AtomicBool,
}

impl Availability {
    pub fn new(
        provider: LlmProvider,
        host: impl Into<String>,
        model: impl Into<String>,
    ) -> Arc<Self> {
        Arc::new(Self {
            provider,
            host: host.into(),
            model: model.into(),
            state: Mutex::new(None),
            probing: AtomicBool::new(false),
        })
    }

    /// Starts a background probe unless one is already running.
    pub fn refresh(self: &Arc<Self>) {
        if self.probing.swap(true, Ordering::SeqCst) {
            return;
        }
        let me = Arc::clone(self);
        thread::spawn(move || {
            let ok = probe(me.provider, &me.host, &me.model);
            me.set(ok);
            me.probing.store(false, Ordering::SeqCst);
        });
    }

    pub fn set(&self, ok: bool) {
        *self.state.lock().unwrap_or_else(|p| p.into_inner()) = Some((ok, Instant::now()));
    }

    /// Last known answer (refreshing it in the background when stale);
    /// `false` until the first probe has completed.
    pub fn is_available(self: &Arc<Self>) -> bool {
        let snapshot = *self.state.lock().unwrap_or_else(|p| p.into_inner());
        match snapshot {
            Some((ok, at)) => {
                if at.elapsed() > PROBE_TTL {
                    self.refresh();
                }
                ok
            }
            None => {
                self.refresh();
                false
            }
        }
    }
}

pub struct CleanupProcessor {
    llm: Option<(OllamaProcessor, Arc<Availability>)>,
    local: LocalCleanup,
}

impl CleanupProcessor {
    pub fn from_settings(settings: &Settings) -> Result<Self> {
        let local = LocalCleanup::new(
            settings.custom_dictionary.clone(),
            settings.custom_aliases.clone(),
        );
        let llm = if settings.llm_cleanup {
            let processor = OllamaProcessor::from_settings(settings)?;
            let availability =
                Availability::new(processor.provider(), processor.host(), processor.model());
            availability.refresh();
            Some((processor, availability))
        } else {
            None
        };
        Ok(Self { llm, local })
    }

    /// Synchronously re-checks the LLM server (for one-shot CLI commands).
    pub fn probe_now(&self) {
        if let Some((llm, availability)) = &self.llm {
            availability.set(probe(llm.provider(), llm.host(), llm.model()));
        }
    }

    /// Whether dictations will currently go through the LLM.
    pub fn llm_active(&self) -> bool {
        self.llm.as_ref().is_some_and(|(_, a)| a.is_available())
    }
}

impl TextProcessor for CleanupProcessor {
    fn process(&self, req: &EnrichedRequest) -> Result<ProcessedText> {
        if let Some((llm, availability)) = &self.llm {
            if availability.is_available() {
                match llm.process(req) {
                    Ok(p) => return Ok(p),
                    Err(e) => {
                        tracing::warn!("LLM cleanup failed, using local cleanup: {e}");
                        availability.set(false);
                        availability.refresh();
                    }
                }
            }
        }
        self.local.process(req)
    }

    fn name(&self) -> &'static str {
        "cleanup"
    }

    fn llm_status(&self) -> Option<bool> {
        self.llm.as_ref().map(|(_, a)| a.is_available())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AppContext, Mode, Transcript};

    fn req(text: &str) -> EnrichedRequest {
        EnrichedRequest {
            transcript: Transcript::new(text),
            ctx: AppContext::unknown(),
            mode: Mode::Auto,
        }
    }

    #[test]
    fn llm_disabled_uses_local_rules() {
        let mut s = Settings::with_defaults();
        s.llm_cleanup = false;
        let p = CleanupProcessor::from_settings(&s).unwrap();
        assert!(!p.llm_active());
        let out = p.process(&req("um, hello world.")).unwrap();
        assert_eq!(out.text, "Hello world.");
        assert!(out.model_text.is_none());
    }

    #[test]
    fn unavailable_llm_falls_back_without_waiting() {
        let mut s = Settings::with_defaults();
        s.llm_cleanup = true;
        let p = CleanupProcessor::from_settings(&s).unwrap();
        if let Some((_, a)) = &p.llm {
            a.set(false);
        }
        let start = Instant::now();
        let out = p.process(&req("uh, ship it.")).unwrap();
        assert_eq!(out.text, "Ship it.");
        assert!(start.elapsed() < Duration::from_millis(50));
    }
}
