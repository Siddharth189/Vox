use std::fs::File;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use global_hotkey::hotkey::{Code, HotKey};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use tao::event::{Event, StartCause};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
#[cfg(target_os = "macos")]
use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

use crate::audio::cue::{self, Cue};
use crate::audio::{Recorder, MAX_RECORDING};
use crate::config::{self, HotkeyConfig, Settings};
use crate::error::{Result, VoxError};
use crate::history::{self, DictationRecord};
use crate::hotkey::{self as hk, PttAction, PushToTalk};
use crate::model::{
    elapsed_ms, AppContext, Format, InjectionResult, LatencyMetrics, Mode, Privacy,
};
use crate::overlay::Overlay;
use crate::permissions;
use crate::pipeline::{build_live_pipeline, EditReport, Pipeline, PreparedReport, Speech};
use crate::speed;
use crate::stats;
use crate::stt::{Live, LiveText, StreamingSession};

enum UserEvent {
    Hotkey(GlobalHotKeyEvent),
    Menu(MenuEvent),
    Done(std::result::Result<PreparedReport, String>),
    Injected {
        report: PreparedReport,
        injection: InjectionResult,
        inject_ms: u64,
    },
    /// An edit by voice finished (pasted) or failed.
    EditDone(std::result::Result<EditReport, String>),
    /// The words so far of recording number `.0`, while it's in progress.
    Live(u64, LiveText),
    /// A pipeline finished building (first run, or after a settings change).
    PipelineReady(std::result::Result<Arc<Pipeline>, String>),
    DownloadProgress(u8),
    DownloadFinished(std::result::Result<PathBuf, String>),
    Tick,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Look {
    Idle,
    Recording,
    Processing,
}

/// What a recording is for: which hotkey started it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// Dictation: the words are typed at the cursor.
    Dictate,
    /// Edit by voice: the words are an instruction for the selection.
    Edit,
}

/// True when any setting that affects the dictation pipeline changed (not
/// the hotkeys, their modes, sound cues, or live text, which apply without
/// a rebuild).
fn pipeline_settings_changed(old: &Settings, new: &Settings) -> bool {
    old.auto_paste != new.auto_paste
        || old.output_language != new.output_language
        || old.input_language != new.input_language
        || old.whisper_model != new.whisper_model
        || old.llm_model != new.llm_model
        || old.llm_cleanup != new.llm_cleanup
        || old.custom_dictionary != new.custom_dictionary
        || old.custom_aliases != new.custom_aliases
        || old.system_message_override != new.system_message_override
        || old.profiles != new.profiles
        || old.llm_provider != new.llm_provider
        || old.llm_base_url != new.llm_base_url
        || old.voice_commands != new.voice_commands
        || old.snippets != new.snippets
}

struct InstanceLock {
    _file: File,
}

impl InstanceLock {
    fn acquire() -> Result<Self> {
        let path = config::data_dir().join("vox.lock");
        crate::secure_fs::ensure_private_dir(&config::data_dir())?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)?;
        // Advisory on Unix, mandatory on Windows; released by the OS if the
        // process dies, so a crash never leaves a stale lock behind.
        file.try_lock().map_err(|_| {
            VoxError::Other("another Vox tray instance is already running (vox.lock held)".into())
        })?;
        Ok(Self { _file: file })
    }
}

/// Logs go to tray.log (and stderr when there is one). The windowless
/// launcher has no console, so the file is the only place errors show up.
fn init_logging() {
    use tracing_subscriber::fmt::writer::MakeWriterExt;

    let path = config::log_path();
    // Keep the log from growing forever across months of use.
    if std::fs::metadata(&path)
        .map(|m| m.len() > 5_000_000)
        .unwrap_or(false)
    {
        let _ = std::fs::remove_file(&path);
    }
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("vox=info,warn"));
    match crate::secure_fs::open_private_append(&path) {
        Ok(file) => {
            let file = std::sync::Mutex::new(file);
            let _ = tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_ansi(false)
                .with_writer(std::io::stderr.and(file))
                .try_init();
        }
        Err(_) => {
            let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
        }
    }
}

/// What the user sees: the tray icon and its status line, and the live
/// text window (or, where there's none, the tray's title).
struct Ui {
    tray: TrayIcon,
    icons: TrayIcons,
    status_item: MenuItem,
    status: String,
    overlay: Option<Overlay>,
    /// Off in settings: the overlay stays hidden.
    live_text: bool,
    /// The recording whose live text is on screen. Updates from any other
    /// (a finished one whose last preview was still in flight) are ignored.
    live_session: Option<u64>,
}

impl Ui {
    fn set_status(&mut self, text: impl Into<String>) {
        self.status = text.into();
        set_status(&self.tray, &self.status_item, &self.status);
    }

    /// Shows the current status again (it may have been overwritten).
    fn show_status(&self) {
        set_status(&self.tray, &self.status_item, &self.status);
    }

    fn set_look(&self, look: Look) {
        set_look(&self.tray, &self.icons, look);
    }

    /// A recording started.
    fn listen(&mut self, session: u64, kind: Kind, hotkey: &HotkeyConfig, tap: &Recorder) {
        self.live_session = Some(session);
        let title = match kind {
            Kind::Dictate => format!(
                "Listening · {} to finish, Esc to cancel",
                hk::display_label(hotkey)
            ),
            Kind::Edit => "Edit · say how to change the selection".to_string(),
        };
        if self.live_text {
            if let Some(overlay) = self.overlay.as_mut() {
                overlay.listen(&title, kind == Kind::Edit, Some(tap.tap()));
            }
        }
    }

    fn live(&mut self, session: u64, text: &LiveText) {
        if self.live_session != Some(session) || !self.live_text {
            return;
        }
        match self.overlay.as_mut() {
            Some(overlay) => overlay.set_text(text),
            // macOS menu bar / Linux indicator label: the last few words.
            None => {
                let full = text.full();
                let tail: String = {
                    let chars: Vec<char> = full.chars().collect();
                    let start = chars.len().saturating_sub(TITLE_CHARS);
                    chars[start..].iter().collect()
                };
                let shown = if tail.len() < full.len() {
                    format!("…{}", tail.trim_start())
                } else {
                    tail
                };
                self.tray.set_title(Some(shown));
            }
        }
    }

    /// Recording stopped; the text is on its way.
    fn working(&mut self, kind: Kind) {
        if let Some(overlay) = self.overlay.as_mut() {
            overlay.working(match kind {
                Kind::Dictate => "Writing…",
                Kind::Edit => "Editing…",
            });
        }
    }

    /// Done: nothing more to show.
    fn done(&mut self) {
        self.live_session = None;
        if let Some(overlay) = self.overlay.as_mut() {
            overlay.hide();
        }
        self.tray.set_title(None::<&str>);
    }

    /// Done, with something the user should notice.
    fn flash(&mut self, message: &str) {
        self.live_session = None;
        self.tray.set_title(None::<&str>);
        match self.overlay.as_mut() {
            Some(overlay) if self.live_text => overlay.flash(message),
            Some(overlay) => overlay.hide(),
            None => {}
        }
    }
}

/// How many characters of live text fit in a menu bar title.
const TITLE_CHARS: usize = 40;

/// The global hotkeys: dictation, edit by voice, and Esc (registered only
/// while recording).
struct Hotkeys {
    manager: GlobalHotKeyManager,
    dictate: Option<HotKey>,
    edit: Option<HotKey>,
    cancel: HotKey,
    cancel_registered: bool,
}

impl Hotkeys {
    fn kind_of(&self, id: u32) -> Option<Kind> {
        if self.dictate.is_some_and(|k| k.id() == id) {
            Some(Kind::Dictate)
        } else if self.edit.is_some_and(|k| k.id() == id) {
            Some(Kind::Edit)
        } else {
            None
        }
    }

    fn register_cancel(&mut self) {
        if !self.cancel_registered {
            self.cancel_registered = self.manager.register(self.cancel).is_ok();
        }
    }

    fn unregister_cancel(&mut self) {
        if self.cancel_registered {
            let _ = self.manager.unregister(self.cancel);
            self.cancel_registered = false;
        }
    }

    /// Re-registers one of the hotkeys after a settings change, keeping the
    /// old binding if the new one is taken. Returns the error to show.
    fn rebind(&mut self, kind: Kind, old: &HotkeyConfig, new: &HotkeyConfig) -> Option<String> {
        let slot = match kind {
            Kind::Dictate => &mut self.dictate,
            Kind::Edit => &mut self.edit,
        };
        if let Some(previous) = slot.take() {
            let _ = self.manager.unregister(previous);
        }
        if !new.enabled {
            return None;
        }
        match register_hotkey(&self.manager, new) {
            Ok(k) => {
                *slot = Some(k);
                None
            }
            Err(e) => {
                tracing::warn!("hotkey re-register failed: {e}");
                // best-effort restore of the previous binding
                if old.enabled {
                    if let Ok(k) = register_hotkey(&self.manager, old) {
                        *slot = Some(k);
                    }
                }
                Some(e.to_string())
            }
        }
    }
}

/// The recording in progress, if any.
struct Recording {
    recorder: Recorder,
    /// Transcription of the recording in progress, when streaming is on.
    /// Dropping it cancels it.
    stream: Option<StreamingSession>,
    /// Which hotkey started the recording in progress.
    kind: Option<Kind>,
    /// Number of the latest recording, for matching its live text.
    session: u64,
    /// Dictations and edits recorded and still being processed.
    processing: usize,
}

pub fn run(model_override: Option<PathBuf>) -> Result<()> {
    // Without this, failures inside global-hotkey's own worker thread (X11
    // connection, XKB extension setup, XGrabKey) are completely silent -
    // its "tracing" feature is opt-in, and register_hotkey() can still
    // return Ok(()) even when nothing was actually registered.
    init_logging();

    let _lock = InstanceLock::acquire()?;
    crate::settings_web::start_background();

    let mut settings = Settings::load();
    let mut rec = Recording {
        recorder: Recorder::new()?,
        stream: None,
        kind: None,
        session: 0,
        processing: 0,
    };
    rec.recorder.set_device(settings.input_device.clone());
    let mut ptt = PushToTalk::new(settings.hotkey.mode);
    let mut edit_ptt = PushToTalk::new(settings.edit_hotkey.mode);

    #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
    let mut event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    #[cfg(target_os = "macos")]
    event_loop.set_activation_policy(ActivationPolicy::Accessory);
    let proxy = event_loop.create_proxy();

    // Hotkey and menu events are pushed straight into the event loop, so a
    // key press is handled the instant it happens instead of on the next
    // poll tick.
    {
        let p = proxy.clone();
        GlobalHotKeyEvent::set_event_handler(Some(move |e: GlobalHotKeyEvent| {
            let _ = p.send_event(UserEvent::Hotkey(e));
        }));
        let p = proxy.clone();
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            let _ = p.send_event(UserEvent::Menu(e));
        }));
    }

    // Linux only: ControlFlow::WaitUntil does not reliably re-arm on at
    // least one real desktop (KDE Plasma Wayland, tao 0.35.3) - confirmed by
    // an unconditional per-tick log that fired 3 times at startup and then
    // never again, even though the process stayed alive (a second thread's
    // settings server kept answering). A blocking Poll+sleep loop "fixed"
    // that but burned 30-50% CPU constantly, which starved the actual
    // dictation pipeline of CPU and turned a ~10s transcription into 50s+.
    // EventLoopProxy::send_event is the same mechanism worker threads
    // already use to deliver UserEvent::Done/Injected, and reliably wakes
    // the loop regardless of the WaitUntil bug, so a dedicated thread using
    // only that plus a real sleep (zero CPU while parked) replaces the
    // timer-based rearming without depending on it.
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let heartbeat_proxy = proxy.clone();
        thread::spawn(move || loop {
            thread::sleep(TICK);
            if heartbeat_proxy.send_event(UserEvent::Tick).is_err() {
                break;
            }
        });
    }

    let mut keys = Hotkeys {
        manager: GlobalHotKeyManager::new()
            .map_err(|e| VoxError::Other(format!("hotkey manager: {e}")))?,
        dictate: None,
        edit: None,
        cancel: HotKey::new(None, Code::Escape),
        cancel_registered: false,
    };
    let mut hotkey_error: Option<String> = None;
    if settings.hotkey.enabled {
        match register_hotkey(&keys.manager, &settings.hotkey) {
            Ok(k) => keys.dictate = Some(k),
            Err(e) => {
                tracing::error!("failed to register hotkey: {e}");
                hotkey_error = Some(e.to_string());
            }
        }
    }
    let mut edit_hotkey_error: Option<String> = None;
    if settings.edit_hotkey.enabled {
        match register_hotkey(&keys.manager, &settings.edit_hotkey) {
            Ok(k) => keys.edit = Some(k),
            Err(e) => {
                tracing::warn!("failed to register the edit-by-voice hotkey: {e}");
                edit_hotkey_error = Some(e.to_string());
            }
        }
    }

    let menu = Menu::new();
    let status_item = MenuItem::new("Vox: starting...", false, None);
    let hotkey_hint = MenuItem::new(
        hotkey_hint_text(&settings.hotkey, hotkey_error.as_deref()),
        false,
        None,
    );
    let edit_hint = MenuItem::new(
        edit_hint_text(&settings.edit_hotkey, edit_hotkey_error.as_deref()),
        false,
        None,
    );
    let settings_item = MenuItem::new("Settings...", true, None);
    let autostart_item = CheckMenuItem::new(
        "Launch at login",
        true,
        crate::autostart::is_enabled(),
        None,
    );
    let accessibility_item = MenuItem::new("Open Accessibility Settings...", true, None);
    let quit_item = MenuItem::new("Quit Vox", true, None);
    let menu_err = |e: tray_icon::menu::Error| VoxError::Other(e.to_string());
    menu.append(&status_item).map_err(menu_err)?;
    menu.append(&hotkey_hint).map_err(menu_err)?;
    menu.append(&edit_hint).map_err(menu_err)?;
    menu.append(&PredefinedMenuItem::separator())
        .map_err(menu_err)?;
    menu.append(&settings_item).map_err(menu_err)?;
    menu.append(&autostart_item).map_err(menu_err)?;
    if cfg!(target_os = "macos") {
        menu.append(&accessibility_item).map_err(menu_err)?;
    }
    menu.append(&PredefinedMenuItem::separator())
        .map_err(menu_err)?;
    menu.append(&quit_item).map_err(menu_err)?;

    let icons = TrayIcons::new();
    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("Vox")
        .with_icon(icons.idle.clone())
        .with_icon_as_template(cfg!(target_os = "macos"))
        .with_title("")
        .build()
        .map_err(|e| VoxError::Other(e.to_string()))?;

    let settings_id = settings_item.id().clone();
    let autostart_id = autostart_item.id().clone();
    let accessibility_id = accessibility_item.id().clone();
    let quit_id = quit_item.id().clone();

    let has_model_override = model_override.is_some();
    let model_path = move |s: &Settings| {
        model_override
            .clone()
            .unwrap_or_else(|| PathBuf::from(&s.whisper_model))
    };

    // Pipeline: built off the UI thread (loading Whisper takes a moment).
    // On first run the default model isn't there yet, so download it first.
    let mut pipeline: Option<Arc<Pipeline>> = None;
    let mut pipeline_settings = settings.clone();
    let mut downloading = false;
    {
        let path = model_path(&settings);
        if path.exists() {
            spawn_pipeline_build(&proxy, settings.clone(), path);
        } else if !has_model_override && is_default_model(&settings) {
            downloading = true;
            spawn_default_model_download(&proxy);
        } else {
            tracing::error!("whisper model not found at {}", path.display());
        }
    }

    let mut last_polled_mtime: Option<SystemTime> =
        config::settings_mtime(&config::settings_path());
    let mut ui = Ui {
        tray,
        icons,
        status_item,
        status: String::new(),
        overlay: Overlay::new(),
        live_text: settings.live_text,
        live_session: None,
    };
    ui.set_status(if downloading {
        "Vox: downloading speech model..."
    } else {
        "Vox: loading speech model..."
    });

    tracing::info!(
        "tray started; hotkey {} registered = {}; edit hotkey {} registered = {}",
        hk::display_label(&settings.hotkey),
        keys.dictate.is_some(),
        hk::display_label(&settings.edit_hotkey),
        keys.edit.is_some()
    );

    event_loop.run(move |event, _, control_flow| {
        // macOS/Windows: a cheap timer tick for settings polling and the
        // recording cap. Linux uses the heartbeat thread instead (see above).
        #[cfg(any(target_os = "macos", windows))]
        {
            *control_flow = ControlFlow::WaitUntil(Instant::now() + TICK);
        }
        #[cfg(not(any(target_os = "macos", windows)))]
        {
            *control_flow = ControlFlow::Wait;
        }

        let user = match event {
            Event::UserEvent(user) => user,
            Event::NewEvents(StartCause::ResumeTimeReached { .. }) => UserEvent::Tick,
            _ => return,
        };

        match user {
            UserEvent::Tick => {
                // Safety net for a lost key-up: never record past the cap.
                if rec
                    .recorder
                    .elapsed()
                    .is_some_and(|e| e >= MAX_RECORDING)
                {
                    tracing::warn!(
                        "recording hit the {}s cap; finishing",
                        MAX_RECORDING.as_secs()
                    );
                    ptt.reset();
                    edit_ptt.reset();
                    finish_recording(&mut rec, &pipeline, &proxy, &settings, &mut keys, &mut ui);
                }
                if let Some(new_settings) = poll_settings(&mut last_polled_mtime) {
                    if new_settings.hotkey != settings.hotkey {
                        let error =
                            keys.rebind(Kind::Dictate, &settings.hotkey, &new_settings.hotkey);
                        hotkey_hint
                            .set_text(hotkey_hint_text(&new_settings.hotkey, error.as_deref()));
                        ptt.set_mode(new_settings.hotkey.mode);
                    }
                    if new_settings.edit_hotkey != settings.edit_hotkey {
                        let error = keys.rebind(
                            Kind::Edit,
                            &settings.edit_hotkey,
                            &new_settings.edit_hotkey,
                        );
                        edit_hint.set_text(edit_hint_text(
                            &new_settings.edit_hotkey,
                            error.as_deref(),
                        ));
                        edit_ptt.set_mode(new_settings.edit_hotkey.mode);
                    }
                    if pipeline_settings_changed(&pipeline_settings, &new_settings) {
                        let model = model_path(&new_settings);
                        if model.exists() {
                            spawn_pipeline_build(&proxy, new_settings.clone(), model);
                        } else {
                            tracing::warn!(
                                "whisper model {} not found; keeping the current one",
                                model.display()
                            );
                        }
                        pipeline_settings = new_settings.clone();
                    }
                    rec.recorder.set_device(new_settings.input_device.clone());
                    ui.live_text = new_settings.live_text;
                    settings = new_settings;
                }
            }

            UserEvent::Menu(MenuEvent { id }) => {
                if id == settings_id {
                    if let Err(e) = crate::util::open_url(crate::settings_web::URL) {
                        tracing::warn!(
                            "failed to open settings UI: {e} (browse to {} manually)",
                            crate::settings_web::URL
                        );
                    }
                } else if id == autostart_id {
                    let want = autostart_item.is_checked();
                    if let Err(e) = crate::autostart::set_enabled(want) {
                        tracing::warn!("launch at login: {e}");
                        autostart_item.set_checked(!want);
                    }
                } else if id == accessibility_id {
                    permissions::request_accessibility_trust();
                    permissions::open_accessibility_settings();
                } else if id == quit_id {
                    if rec.recorder.is_recording() {
                        rec.recorder.cancel();
                    }
                    rec.stream = None;
                    *control_flow = ControlFlow::Exit;
                }
            }

            UserEvent::Hotkey(event) => {
                if event.id == keys.cancel.id() {
                    if event.state == HotKeyState::Pressed && rec.recorder.is_recording() {
                        rec.recorder.cancel();
                        rec.stream = None;
                        rec.kind = None;
                        ptt.reset();
                        edit_ptt.reset();
                        keys.unregister_cancel();
                        if settings.sound_cues {
                            cue::play(Cue::Cancel);
                        }
                        ui.set_status("Vox: cancelled");
                        ui.done();
                        ui.set_look(look_for(false, rec.processing));
                    }
                    return;
                }
                let Some(kind) = keys.kind_of(event.id) else {
                    return;
                };
                // While one hotkey's recording runs, the other does nothing.
                if rec.kind.is_some_and(|k| k != kind) {
                    return;
                }
                let machine = match kind {
                    Kind::Dictate => &mut ptt,
                    Kind::Edit => &mut edit_ptt,
                };
                let now = Instant::now();
                let action = match event.state {
                    HotKeyState::Pressed => machine.on_press(now),
                    HotKeyState::Released => machine.on_release(now),
                };
                match action {
                    PttAction::None => {}
                    PttAction::Start => {
                        let started = start_recording(
                            kind, &mut rec, &pipeline, &proxy, &settings, &mut keys, &mut ui,
                        );
                        if !started {
                            machine.reset();
                        }
                    }
                    PttAction::Stop => {
                        finish_recording(&mut rec, &pipeline, &proxy, &settings, &mut keys, &mut ui);
                    }
                }
            }

            UserEvent::Live(session, text) => ui.live(session, &text),

            UserEvent::Done(Ok(report)) => {
                if report.processed_text.is_empty() {
                    rec.processing = rec.processing.saturating_sub(1);
                    tracing::info!(
                        "nothing to insert (raw={:?}); silence or non-speech",
                        report.raw_text
                    );
                    if settings.sound_cues {
                        cue::play(Cue::Cancel);
                    }
                    let message = match report.rejected {
                        Some(ref why) => {
                            tracing::info!("discarded dictation ({why})");
                            "didn't catch that - try a full sentence"
                        }
                        None => "didn't catch that",
                    };
                    ui.set_status(format!("Vox: {message}"));
                    if rec.kind.is_none() {
                        ui.flash(&capitalize(message));
                    }
                    ui.set_look(look_for(rec.recorder.is_recording(), rec.processing));
                } else if let Some(ref p) = pipeline {
                    let p = Arc::clone(p);
                    let proxy = proxy.clone();
                    thread::spawn(move || {
                        let start = Instant::now();
                        let injection = p
                            .inject_processed_text(&report.processed_text, &report.ctx)
                            .unwrap_or_else(|e| InjectionResult {
                                injected_chars: 0,
                                strategy: format!("inject-error: {e}"),
                            });
                        let inject_ms = elapsed_ms(start.elapsed());
                        let _ = proxy.send_event(UserEvent::Injected {
                            report,
                            injection,
                            inject_ms,
                        });
                    });
                }
            }

            UserEvent::Done(Err(e)) => {
                rec.processing = rec.processing.saturating_sub(1);
                tracing::warn!("dictation failed: {e}");
                let message = if e.contains("privacy blocked") {
                    ui.set_status("Vox: off in this app");
                    "Vox is off in this app"
                } else {
                    ui.set_status(format!("Vox: failed ({e})"));
                    "Dictation failed - see the tray menu"
                };
                if rec.kind.is_none() {
                    ui.flash(message);
                }
                ui.set_look(look_for(rec.recorder.is_recording(), rec.processing));
            }

            UserEvent::Injected {
                report,
                injection,
                inject_ms,
            } => {
                rec.processing = rec.processing.saturating_sub(1);
                if rec.kind.is_none() {
                    ui.done();
                }
                let latency = report.latency.clone().with_injection(inject_ms);
                let record = DictationRecord {
                    id: history::new_record_id(history::now_ms()),
                    created_at_ms: history::now_ms(),
                    app_name: report.app_name.clone(),
                    bundle_id: report.ctx.bundle_id.clone(),
                    window_title: report.ctx.window_title.clone(),
                    format: report.ctx.profile.format,
                    privacy: report.ctx.profile.privacy,
                    mode: Mode::Auto,
                    raw_text: report.raw_text.clone(),
                    model_text: report.model_text.clone(),
                    final_text: report.processed_text.clone(),
                    injection_strategy: injection.strategy.clone(),
                    injected_chars: injection.injected_chars,
                    latency: latency.clone(),
                    corrected_text: None,
                    stt_confidence: report.confidence,
                    rejected: report.rejected.clone(),
                    edit: false,
                };
                let slow = save_record(record, &latency, &report.processed_text, report.audio_ms);

                let llm = pipeline.as_ref().and_then(|p| p.llm_status());
                tracing::info!(
                    "{} via {} | {} chars | stt={}ms cleanup={}ms inject={}ms total={}ms | confidence={} llm={}",
                    report.app_name,
                    injection.strategy,
                    injection.injected_chars,
                    latency.transcribe_ms,
                    latency.process_ms,
                    latency.inject_ms,
                    latency.total_ms,
                    report
                        .confidence
                        .map_or_else(|| "-".to_string(), |c| format!("{c:.2}")),
                    match llm {
                        None => "off",
                        Some(true) => "used",
                        Some(false) => "unreachable",
                    }
                );

                let status = match slow {
                    Some(status) => status,
                    None if report.rejected.is_some() && status_for_strategy(&injection.strategy) == STATUS_INSERTED => {
                        tracing::info!(
                            "inserted, but part of the dictation was discarded ({})",
                            report.rejected.as_deref().unwrap_or_default()
                        );
                        STATUS_PART_LOST.to_string()
                    }
                    None => {
                        let pasted = status_for_strategy(&injection.strategy);
                        // Only replace the plain "inserted" message: the
                        // clipboard and permission hints matter more.
                        match llm_off_status(llm, &settings.output_language) {
                            Some(s) if pasted == STATUS_INSERTED => s.to_string(),
                            _ => pasted.to_string(),
                        }
                    }
                };
                ui.set_status(status);
                ui.set_look(look_for(rec.recorder.is_recording(), rec.processing));
            }

            UserEvent::EditDone(Ok(report)) => {
                rec.processing = rec.processing.saturating_sub(1);
                if rec.kind.is_none() {
                    ui.done();
                }
                let record = DictationRecord {
                    id: history::new_record_id(history::now_ms()),
                    created_at_ms: history::now_ms(),
                    app_name: report.ctx.app_name.clone(),
                    bundle_id: report.ctx.bundle_id.clone(),
                    window_title: report.ctx.window_title.clone(),
                    format: report.ctx.profile.format,
                    privacy: report.ctx.profile.privacy,
                    mode: Mode::Auto,
                    raw_text: report.instruction.clone(),
                    model_text: None,
                    final_text: report.text.clone(),
                    injection_strategy: report.injection.strategy.clone(),
                    injected_chars: report.injection.injected_chars,
                    latency: report.latency.clone(),
                    corrected_text: None,
                    stt_confidence: report.confidence,
                    rejected: None,
                    edit: true,
                };
                let slow = save_record(
                    record,
                    &report.latency,
                    &report.instruction,
                    report.latency.audio_ms,
                );
                tracing::info!(
                    "edit in {} via {} | {} chars selected -> {} chars | stt={}ms llm={}ms copy+paste={}ms",
                    report.ctx.app_name,
                    report.injection.strategy,
                    report.selection_chars,
                    report.injection.injected_chars,
                    report.latency.transcribe_ms,
                    report.latency.process_ms,
                    report.latency.inject_ms,
                );
                let pasted = status_for_strategy(&report.injection.strategy);
                ui.set_status(match slow {
                    Some(status) => status,
                    None if pasted != STATUS_INSERTED => pasted.to_string(),
                    None if report.selection_chars == 0 => {
                        "Vox: nothing was selected - inserted at cursor".to_string()
                    }
                    None => "Vox: edited the selection".to_string(),
                });
                ui.set_look(look_for(rec.recorder.is_recording(), rec.processing));
            }

            UserEvent::EditDone(Err(e)) => {
                rec.processing = rec.processing.saturating_sub(1);
                tracing::warn!("edit by voice failed: {e}");
                if settings.sound_cues {
                    cue::play(Cue::Cancel);
                }
                let message = edit_failure_message(&e);
                ui.set_status(format!("Vox: {}", message.to_lowercase()));
                if rec.kind.is_none() {
                    ui.flash(&message);
                }
                ui.set_look(look_for(rec.recorder.is_recording(), rec.processing));
            }

            UserEvent::PipelineReady(Ok(p)) => {
                pipeline = Some(p);
                ui.set_status(ready_status(&settings));
                tracing::info!("pipeline ready");
            }

            UserEvent::PipelineReady(Err(e)) => {
                tracing::error!("pipeline build failed: {e}");
                ui.set_status(format!("Vox: {e}"));
            }

            UserEvent::DownloadProgress(pct) => {
                ui.set_status(format!("Vox: downloading speech model... {pct}%"));
            }

            UserEvent::DownloadFinished(Ok(path)) => {
                tracing::info!("downloaded {}", path.display());
                ui.set_status("Vox: loading speech model...");
                spawn_pipeline_build(&proxy, settings.clone(), path);
            }

            UserEvent::DownloadFinished(Err(e)) => {
                tracing::error!("model download failed: {e}");
                ui.set_status("Vox: model download failed (run `vox download-model`)");
            }
        }
    });
}

const TICK: Duration = Duration::from_millis(250);

/// The settings file, if it changed since the last look.
fn poll_settings(last_polled_mtime: &mut Option<SystemTime>) -> Option<Settings> {
    let mtime = config::settings_mtime(&config::settings_path())?;
    if Some(mtime) == *last_polled_mtime {
        return None;
    }
    *last_polled_mtime = Some(mtime);
    Some(Settings::load())
}

/// Adds a finished dictation or edit to history and the usage stats.
/// Returns the status to show instead of the usual one if it was slow.
fn save_record(
    record: DictationRecord,
    latency: &LatencyMetrics,
    spoken: &str,
    audio_ms: u64,
) -> Option<String> {
    let slow = speed::check(latency, &history::load_history());
    if let Err(e) = history::append_record(record) {
        tracing::warn!("history append failed: {e}");
    }
    if let Err(e) = stats::record_dictation(spoken, audio_ms) {
        tracing::warn!("stats update failed: {e}");
    }
    slow.map(|slow| {
        let on_battery = speed::on_battery();
        tracing::warn!(
            "slow transcription: {} ({}ms for {}ms of audio, on_battery={on_battery:?})",
            speed::describe(slow.factor),
            slow.transcribe_ms,
            slow.audio_ms
        );
        speed::status_text(&slow, on_battery)
    })
}

/// Why edit by voice can't run here at all, checked before the microphone
/// opens so nobody speaks an instruction for nothing.
fn edit_unavailable(
    settings: &Settings,
    ctx: Option<&AppContext>,
    llm: Option<bool>,
) -> Option<&'static str> {
    if !settings.auto_paste {
        Some("Edit by voice needs auto-paste - turn it on in Settings")
    } else if !settings.llm_cleanup || llm.is_none() {
        Some("Edit by voice needs LLM cleanup - turn it on in Settings")
    } else if ctx.is_some_and(|c| c.profile.format == Format::Shell) {
        Some("Edit by voice is off in terminals")
    } else if llm == Some(false) {
        Some("Edit by voice needs your LLM server running (see vox doctor)")
    } else {
        None
    }
}

/// A short explanation of a failed edit, for the bubble and the tray.
fn edit_failure_message(error: &str) -> String {
    let e = error.to_ascii_lowercase();
    if e.contains("didn't catch") {
        "Didn't catch the instruction".into()
    } else if e.contains("didn't answer") || e.contains("returned nothing") {
        "The LLM didn't answer - nothing was changed".into()
    } else if e.contains("too long") {
        "Selection is too long to edit".into()
    } else if e.contains("privacy blocked") {
        "Vox is off in this app".into()
    } else if e.contains("terminals") {
        "Edit by voice is off in terminals".into()
    } else {
        "Edit failed - nothing was changed".into()
    }
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Opens the microphone for a dictation or an edit. False if it didn't
/// start (the hotkey's state machine should then be reset).
fn start_recording(
    kind: Kind,
    rec: &mut Recording,
    pipeline: &Option<Arc<Pipeline>>,
    proxy: &EventLoopProxy<UserEvent>,
    settings: &Settings,
    keys: &mut Hotkeys,
    ui: &mut Ui,
) -> bool {
    let Some(ref p) = pipeline else {
        if settings.sound_cues {
            cue::play(Cue::Cancel);
        }
        // keep the download/loading status visible
        ui.show_status();
        return false;
    };
    // Check the privacy switch *before* opening the mic, not after
    // recording someone's password.
    let start_ctx = p.detector.current().ok();
    if let Some(ref ctx) = start_ctx {
        if ctx.profile.privacy == Privacy::Disabled {
            if settings.sound_cues {
                cue::play(Cue::Cancel);
            }
            ui.set_status(format!("Vox: off in {}", ctx.app_name));
            return false;
        }
    }
    if kind == Kind::Edit {
        if let Some(why) = edit_unavailable(settings, start_ctx.as_ref(), p.llm_status()) {
            if settings.sound_cues {
                cue::play(Cue::Cancel);
            }
            ui.set_status(format!("Vox: {}", why.to_lowercase()));
            ui.flash(why);
            return false;
        }
    }
    match rec.recorder.start() {
        Ok(()) => {
            if settings.sound_cues {
                cue::play(Cue::Start);
            }
            rec.session += 1;
            rec.kind = Some(kind);
            let ctx = start_ctx.unwrap_or_else(AppContext::unknown);
            rec.stream = settings.streaming_transcription.then(|| {
                let live = settings
                    .live_text
                    .then(|| live_text(p, &ctx, proxy, rec.session));
                start_stream(p, &rec.recorder, ctx, live)
            });
            keys.register_cancel();
            ui.set_look(Look::Recording);
            ui.listen(rec.session, kind, &settings.hotkey, &rec.recorder);
            let hotkey = match kind {
                Kind::Dictate => &settings.hotkey,
                Kind::Edit => &settings.edit_hotkey,
            };
            ui.set_status(match kind {
                Kind::Dictate => format!(
                    "Vox: listening... ({} to finish, Esc to cancel)",
                    hk::display_label(hotkey)
                ),
                Kind::Edit => format!(
                    "Vox: say how to change the selection ({} to finish, Esc to cancel)",
                    hk::display_label(hotkey)
                ),
            });
            true
        }
        Err(e) => {
            tracing::error!("recorder start failed: {e}");
            if settings.sound_cues {
                cue::play(Cue::Cancel);
            }
            ui.set_status(format!("Vox: {e}"));
            false
        }
    }
}

fn finish_recording(
    rec: &mut Recording,
    pipeline: &Option<Arc<Pipeline>>,
    proxy: &EventLoopProxy<UserEvent>,
    settings: &Settings,
    keys: &mut Hotkeys,
    ui: &mut Ui,
) {
    keys.unregister_cancel();
    let kind = rec.kind.take().unwrap_or(Kind::Dictate);
    // Streaming: most of the audio is already transcribed; the session
    // finishes the rest. Otherwise transcribe the whole clip now.
    let speech = match rec.stream.take() {
        Some(session) => {
            rec.recorder.stop_capture();
            Speech::Streamed(session)
        }
        None => match rec.recorder.stop() {
            Ok(a) => Speech::Clip(a),
            Err(e) => {
                tracing::warn!("recorder stop failed: {e}");
                ui.done();
                ui.set_look(look_for(false, rec.processing));
                return;
            }
        },
    };
    if settings.sound_cues {
        cue::play(Cue::Stop);
    }
    let Some(p) = pipeline.as_ref() else {
        ui.done();
        ui.set_look(look_for(false, rec.processing));
        return;
    };

    // App detection stays on the main thread (AppKit requires it).
    let ctx = match p.detector.current() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("app detection failed (canceling dictation): {e}");
            ui.set_status("Vox: couldn't detect the active app");
            ui.flash("Couldn't detect the active app");
            ui.set_look(look_for(false, rec.processing));
            return;
        }
    };

    rec.processing += 1;
    ui.set_status(match kind {
        Kind::Dictate => "Vox: transcribing...",
        Kind::Edit => "Vox: editing...",
    });
    ui.set_look(Look::Processing);
    ui.working(kind);

    let p = Arc::clone(p);
    let proxy = proxy.clone();
    thread::spawn(move || {
        let event = match kind {
            Kind::Dictate => UserEvent::Done(
                match speech {
                    Speech::Clip(audio) => p.prepare_with_ctx(&audio, ctx, Mode::Auto),
                    Speech::Streamed(session) => p.prepare_streamed(session, ctx, Mode::Auto),
                }
                .map_err(|e| e.to_string()),
            ),
            Kind::Edit => UserEvent::EditDone(p.edit(speech, ctx).map_err(|e| e.to_string())),
        };
        let _ = proxy.send_event(event);
    });
}

/// Starts transcribing the dictation `recorder` just started capturing,
/// segment by segment, on the pipeline's Whisper.
fn start_stream(
    p: &Arc<Pipeline>,
    recorder: &Recorder,
    ctx: AppContext,
    live: Option<Live>,
) -> StreamingSession {
    let p = Arc::clone(p);
    StreamingSession::start_live(
        recorder.tap(),
        move |audio, hint| p.transcriber.transcribe_segment(audio, &ctx, hint),
        live,
    )
}

/// Live text for recording number `session`: previews from the pipeline's
/// Whisper, delivered to the event loop.
fn live_text(
    p: &Arc<Pipeline>,
    ctx: &AppContext,
    proxy: &EventLoopProxy<UserEvent>,
    session: u64,
) -> Live {
    let p = Arc::clone(p);
    let ctx = ctx.clone();
    let proxy = proxy.clone();
    Live {
        preview: Box::new(move |audio, hint, stop| p.transcriber.preview(audio, &ctx, hint, stop)),
        sink: Box::new(move |text| {
            let _ = proxy.send_event(UserEvent::Live(session, text));
        }),
    }
}

fn is_default_model(settings: &Settings) -> bool {
    let default = config::whisper_model_path(config::DEFAULT_WHISPER_MODEL_NAME);
    std::path::Path::new(&settings.whisper_model) == default
}

fn spawn_default_model_download(proxy: &EventLoopProxy<UserEvent>) {
    let proxy = proxy.clone();
    thread::spawn(move || {
        let mut last_pct = u8::MAX;
        let result = crate::models::download(config::DEFAULT_WHISPER_MODEL_NAME, |done, total| {
            if let Some(total) = total.filter(|t| *t > 0) {
                let pct = ((done * 100) / total).min(100) as u8;
                if pct != last_pct {
                    last_pct = pct;
                    let _ = proxy.send_event(UserEvent::DownloadProgress(pct));
                }
            }
        })
        .map_err(|e| e.to_string());
        let _ = proxy.send_event(UserEvent::DownloadFinished(result));
    });
}

fn spawn_pipeline_build(proxy: &EventLoopProxy<UserEvent>, settings: Settings, model: PathBuf) {
    let proxy = proxy.clone();
    thread::spawn(move || {
        let result = build_live_pipeline(&settings, &model)
            .map(Arc::new)
            .map_err(|e| e.to_string());
        let _ = proxy.send_event(UserEvent::PipelineReady(result));
    });
}

fn ready_status(settings: &Settings) -> String {
    if settings.hotkey.enabled {
        format!(
            "Vox: ready - press {} to dictate",
            hk::display_label(&settings.hotkey)
        )
    } else {
        "Vox: ready (hotkey disabled)".into()
    }
}

#[cfg(target_os = "macos")]
const PASTE_HINT: &str = "Vox: on clipboard (Cmd+V)";
#[cfg(not(target_os = "macos"))]
const PASTE_HINT: &str = "Vox: on clipboard (Ctrl+V)";

const STATUS_INSERTED: &str = "Vox: inserted at cursor";
const STATUS_PART_LOST: &str = "Vox: inserted (part not caught)";

/// The status after a paste when LLM cleanup is turned on but its server
/// isn't answering, so the user knows the text skipped polishing (and
/// translation) instead of wondering why it reads raw.
fn llm_off_status(llm: Option<bool>, output_language: &str) -> Option<&'static str> {
    if llm != Some(false) {
        return None;
    }
    let lang = output_language.trim().to_ascii_lowercase();
    Some(if lang.is_empty() || lang == "auto" {
        "Vox: inserted unpolished - LLM not running (see vox doctor)"
    } else {
        "Vox: inserted untranslated - LLM not running (see vox doctor)"
    })
}

fn status_for_strategy(strategy: &str) -> &'static str {
    if strategy.contains("clipboard kept") {
        "Vox: paste sent (clipboard kept)"
    } else if strategy.starts_with("auto-paste") {
        STATUS_INSERTED
    } else if strategy.contains("Accessibility") {
        "Vox: clipboard - grant Accessibility"
    } else {
        PASTE_HINT
    }
}

fn look_for(recording: bool, processing: usize) -> Look {
    if recording {
        Look::Recording
    } else if processing > 0 {
        Look::Processing
    } else {
        Look::Idle
    }
}

fn set_status(tray: &TrayIcon, status_item: &MenuItem, text: &str) {
    status_item.set_text(text);
    let _ = tray.set_tooltip(Some(text));
}

fn set_look(tray: &TrayIcon, icons: &TrayIcons, look: Look) {
    let (icon, template) = match look {
        // Idle uses template tinting on macOS so it matches the menu bar.
        Look::Idle => (&icons.idle, cfg!(target_os = "macos")),
        Look::Recording => (&icons.recording, false),
        Look::Processing => (&icons.processing, false),
    };
    let _ = tray.set_icon_with_as_template(Some(icon.clone()), template);
}

fn hotkey_hint_text(cfg: &HotkeyConfig, error: Option<&str>) -> String {
    if !cfg.enabled {
        return "Hotkey: disabled".into();
    }
    let label = hk::display_label(cfg);
    if let Some(e) = error {
        return format!("Hotkey {label} unavailable: {e}");
    }
    let how = match cfg.mode {
        crate::config::HotkeyMode::Hybrid => "tap to toggle, or hold",
        crate::config::HotkeyMode::Hold => "hold to talk",
        crate::config::HotkeyMode::Toggle => "press to start/stop",
    };
    format!("Hotkey: {label} ({how})")
}

fn edit_hint_text(cfg: &HotkeyConfig, error: Option<&str>) -> String {
    if !cfg.enabled {
        return "Edit by voice: off".into();
    }
    let label = hk::display_label(cfg);
    match error {
        Some(e) => format!("Edit by voice {label} unavailable: {e}"),
        None => format!("Edit by voice: select text, then {label}"),
    }
}

fn register_hotkey(manager: &GlobalHotKeyManager, cfg: &HotkeyConfig) -> Result<HotKey> {
    let hotkey = hk::hotkey_from_config(cfg)?;
    manager.register(hotkey).map_err(|e| {
        VoxError::Other(format!(
            "{} is already in use by another app ({e})",
            hk::display_label(cfg)
        ))
    })?;
    Ok(hotkey)
}

struct TrayIcons {
    idle: Icon,
    recording: Icon,
    processing: Icon,
}

impl TrayIcons {
    fn new() -> Self {
        // macOS tints the idle template icon to match the menu bar; the
        // Windows/Linux trays have no such tinting and may be light or dark,
        // so idle uses a blue that reads on both instead of black.
        let idle = if cfg!(target_os = "macos") {
            [0, 0, 0]
        } else {
            [59, 130, 246]
        };
        Self {
            idle: make_tray_icon(idle),
            recording: make_tray_icon([220, 40, 40]),
            processing: make_tray_icon([245, 158, 11]),
        }
    }
}

/// Procedural 32x32 RGBA "sound bars" glyph in a single color with an alpha mask.
fn make_tray_icon(rgb: [u8; 3]) -> Icon {
    const SIZE: u32 = 32;
    let mut rgba = vec![0u8; (SIZE * SIZE * 4) as usize];

    // 4 vertical capsules of varying height/position
    let bars = [
        (6.0f32, 22.0, 8.0), // x-center, height, y-center
        (12.0, 28.0, 8.0),
        (18.0, 18.0, 8.0),
        (24.0, 24.0, 8.0),
    ];
    let radius = 2.2f32;

    for y in 0..SIZE {
        for x in 0..SIZE {
            let fx = x as f32 + 0.5;
            let fy = y as f32 + 0.5;
            let mut alpha = 0.0f32;
            for &(cx, height, _pad) in &bars {
                let half_h = height / 2.0;
                let cy = SIZE as f32 / 2.0;
                let d = sd_capsule(fx, fy, cx, cy - half_h, cx, cy + half_h, radius);
                let a = (1.0 - d).clamp(0.0, 1.0);
                alpha = alpha.max(a);
            }
            let i = ((y * SIZE + x) * 4) as usize;
            rgba[i] = rgb[0];
            rgba[i + 1] = rgb[1];
            rgba[i + 2] = rgb[2];
            rgba[i + 3] = (alpha * 255.0) as u8;
        }
    }

    Icon::from_rgba(rgba, SIZE, SIZE).expect("valid tray icon")
}

fn sd_capsule(px: f32, py: f32, ax: f32, ay: f32, bx: f32, by: f32, r: f32) -> f32 {
    let pax = px - ax;
    let pay = py - ay;
    let bax = bx - ax;
    let bay = by - ay;
    let baba = bax * bax + bay * bay;
    let paba = pax * bax + pay * bay;
    let h = if baba > 0.0 {
        (paba / baba).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let dx = pax - bax * h;
    let dy = pay - bay * h;
    (dx * dx + dy * dy).sqrt() - r
}

#[cfg(test)]
mod llm_status_tests {
    use super::*;

    #[test]
    fn status_says_when_llm_was_skipped() {
        assert_eq!(llm_off_status(None, "English"), None);
        assert_eq!(llm_off_status(Some(true), "English"), None);
        assert!(llm_off_status(Some(false), "auto")
            .unwrap()
            .contains("unpolished"));
        assert!(llm_off_status(Some(false), "English")
            .unwrap()
            .contains("untranslated"));
    }

    fn ctx(format: Format) -> AppContext {
        AppContext {
            profile: crate::model::AppProfile {
                format,
                privacy: Privacy::LocalOnly,
            },
            ..AppContext::unknown()
        }
    }

    #[test]
    fn edit_is_refused_before_listening_when_it_cant_work() {
        let ok = Settings::with_defaults();
        let prose = ctx(Format::CleanProse);
        assert_eq!(edit_unavailable(&ok, Some(&prose), Some(true)), None);
        // Unknown app: allowed, the pipeline checks again.
        assert_eq!(edit_unavailable(&ok, None, Some(true)), None);

        let mut no_paste = ok.clone();
        no_paste.auto_paste = false;
        assert!(edit_unavailable(&no_paste, Some(&prose), Some(true))
            .unwrap()
            .contains("auto-paste"));

        let mut no_llm = ok.clone();
        no_llm.llm_cleanup = false;
        assert!(edit_unavailable(&no_llm, Some(&prose), None)
            .unwrap()
            .contains("LLM cleanup"));

        assert!(edit_unavailable(&ok, Some(&ctx(Format::Shell)), Some(true))
            .unwrap()
            .contains("terminals"));
        assert!(edit_unavailable(&ok, Some(&prose), Some(false))
            .unwrap()
            .contains("LLM server"));
    }

    #[test]
    fn edit_failures_read_as_what_happened() {
        assert_eq!(
            edit_failure_message("transcription error: didn't catch the instruction"),
            "Didn't catch the instruction"
        );
        assert_eq!(
            edit_failure_message("processing error: Ollama (llama3.2) didn't answer: refused"),
            "The LLM didn't answer - nothing was changed"
        );
        assert_eq!(
            edit_failure_message(
                "processing error: selection is too long to edit (over 12000 characters)"
            ),
            "Selection is too long to edit"
        );
        assert_eq!(
            edit_failure_message("injection error: clipboard busy"),
            "Edit failed - nothing was changed"
        );
    }
}
