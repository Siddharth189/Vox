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
use crate::model::{elapsed_ms, InjectionResult, Mode, Privacy};
use crate::permissions;
use crate::pipeline::{build_live_pipeline, Pipeline, PreparedReport};

enum UserEvent {
    Hotkey(GlobalHotKeyEvent),
    Menu(MenuEvent),
    Done(std::result::Result<PreparedReport, String>),
    Injected {
        report: PreparedReport,
        injection: InjectionResult,
        inject_ms: u64,
    },
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

/// True when any setting that affects the dictation pipeline changed (not
/// the hotkey, its mode, or sound cues, which apply without a rebuild).
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

pub fn run(model_override: Option<PathBuf>) -> Result<()> {
    // Without this, failures inside global-hotkey's own worker thread (X11
    // connection, XKB extension setup, XGrabKey) are completely silent -
    // its "tracing" feature is opt-in, and register_hotkey() can still
    // return Ok(()) even when nothing was actually registered.
    init_logging();

    let _lock = InstanceLock::acquire()?;
    crate::settings_web::start_background();

    let mut settings = Settings::load();
    let mut recorder = Recorder::new()?;
    recorder.set_device(settings.input_device.clone());
    let mut ptt = PushToTalk::new(settings.hotkey.mode);

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

    let hotkey_manager =
        GlobalHotKeyManager::new().map_err(|e| VoxError::Other(format!("hotkey manager: {e}")))?;
    let mut current_hotkey: Option<HotKey> = None;
    let mut hotkey_error: Option<String> = None;
    if settings.hotkey.enabled {
        match register_hotkey(&hotkey_manager, &settings.hotkey) {
            Ok(k) => current_hotkey = Some(k),
            Err(e) => {
                tracing::error!("failed to register hotkey: {e}");
                hotkey_error = Some(e.to_string());
            }
        }
    }
    let cancel_hotkey = HotKey::new(None, Code::Escape);
    let mut cancel_registered = false;

    let menu = Menu::new();
    let status_item = MenuItem::new("Vox: starting...", false, None);
    let hotkey_hint = MenuItem::new(
        hotkey_hint_text(&settings.hotkey, hotkey_error.as_deref()),
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
    let mut processing = 0usize;
    let mut status = if downloading {
        "Vox: downloading speech model...".to_string()
    } else {
        "Vox: loading speech model...".to_string()
    };
    set_status(&tray, &status_item, &status);

    tracing::info!(
        "tray started; hotkey {} registered = {}",
        hk::display_label(&settings.hotkey),
        current_hotkey.is_some()
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
                periodic(
                    &mut last_polled_mtime,
                    &mut settings,
                    &mut pipeline_settings,
                    &mut ptt,
                    &hotkey_manager,
                    &mut current_hotkey,
                    &hotkey_hint,
                    &proxy,
                    &model_path,
                    &mut recorder,
                    &mut cancel_registered,
                    &cancel_hotkey,
                    &tray,
                    &icons,
                    &status_item,
                    &mut status,
                    &mut processing,
                    &pipeline,
                );
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
                    if recorder.is_recording() {
                        recorder.cancel();
                    }
                    *control_flow = ControlFlow::Exit;
                }
            }

            UserEvent::Hotkey(event) => {
                let is_cancel = event.id == cancel_hotkey.id();
                if is_cancel {
                    if event.state == HotKeyState::Pressed && recorder.is_recording() {
                        recorder.cancel();
                        ptt.reset();
                        unregister_cancel(&hotkey_manager, &cancel_hotkey, &mut cancel_registered);
                        if settings.sound_cues {
                            cue::play(Cue::Cancel);
                        }
                        status = "Vox: cancelled".into();
                        set_look(&tray, &icons, look_for(false, processing));
                        set_status(&tray, &status_item, &status);
                    }
                    return;
                }
                let Some(ref registered) = current_hotkey else {
                    return;
                };
                if event.id != registered.id() {
                    return;
                }
                let now = Instant::now();
                let action = match event.state {
                    HotKeyState::Pressed => ptt.on_press(now),
                    HotKeyState::Released => ptt.on_release(now),
                };
                match action {
                    PttAction::None => {}
                    PttAction::Start => {
                        let Some(ref p) = pipeline else {
                            ptt.reset();
                            if settings.sound_cues {
                                cue::play(Cue::Cancel);
                            }
                            // keep the download/loading status visible
                            set_status(&tray, &status_item, &status);
                            return;
                        };
                        // Check the privacy switch *before* opening the mic,
                        // not after recording someone's password.
                        if let Ok(ctx) = p.detector.current() {
                            if ctx.profile.privacy == Privacy::Disabled {
                                ptt.reset();
                                if settings.sound_cues {
                                    cue::play(Cue::Cancel);
                                }
                                status = format!("Vox: off in {}", ctx.app_name);
                                set_status(&tray, &status_item, &status);
                                return;
                            }
                        }
                        match recorder.start() {
                            Ok(()) => {
                                if settings.sound_cues {
                                    cue::play(Cue::Start);
                                }
                                register_cancel(
                                    &hotkey_manager,
                                    &cancel_hotkey,
                                    &mut cancel_registered,
                                );
                                set_look(&tray, &icons, Look::Recording);
                                status = format!(
                                    "Vox: listening... ({} to finish, Esc to cancel)",
                                    hk::display_label(&settings.hotkey)
                                );
                                set_status(&tray, &status_item, &status);
                            }
                            Err(e) => {
                                ptt.reset();
                                tracing::error!("recorder start failed: {e}");
                                if settings.sound_cues {
                                    cue::play(Cue::Cancel);
                                }
                                status = format!("Vox: {e}");
                                set_status(&tray, &status_item, &status);
                            }
                        }
                    }
                    PttAction::Stop => {
                        finish_recording(
                            &mut recorder,
                            &pipeline,
                            &proxy,
                            &settings,
                            &hotkey_manager,
                            &cancel_hotkey,
                            &mut cancel_registered,
                            &tray,
                            &icons,
                            &status_item,
                            &mut status,
                            &mut processing,
                        );
                    }
                }
            }

            UserEvent::Done(Ok(report)) => {
                if report.processed_text.is_empty() {
                    processing = processing.saturating_sub(1);
                    tracing::info!(
                        "nothing to insert (raw={:?}); silence or non-speech",
                        report.raw_text
                    );
                    if settings.sound_cues {
                        cue::play(Cue::Cancel);
                    }
                    status = "Vox: didn't catch that".into();
                    set_look(&tray, &icons, look_for(recorder.is_recording(), processing));
                    set_status(&tray, &status_item, &status);
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
                processing = processing.saturating_sub(1);
                tracing::warn!("dictation failed: {e}");
                status = if e.contains("privacy blocked") {
                    "Vox: off in this app".into()
                } else {
                    format!("Vox: failed ({e})")
                };
                set_look(&tray, &icons, look_for(recorder.is_recording(), processing));
                set_status(&tray, &status_item, &status);
            }

            UserEvent::Injected {
                report,
                injection,
                inject_ms,
            } => {
                processing = processing.saturating_sub(1);
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
                };
                if let Err(e) = history::append_record(record) {
                    tracing::warn!("history append failed: {e}");
                }

                tracing::info!(
                    "{} via {} | {} chars | stt={}ms cleanup={}ms inject={}ms total={}ms",
                    report.app_name,
                    injection.strategy,
                    injection.injected_chars,
                    latency.transcribe_ms,
                    latency.process_ms,
                    latency.inject_ms,
                    latency.total_ms
                );

                status = status_for_strategy(&injection.strategy).to_string();
                set_look(&tray, &icons, look_for(recorder.is_recording(), processing));
                set_status(&tray, &status_item, &status);
            }

            UserEvent::PipelineReady(Ok(p)) => {
                pipeline = Some(p);
                status = ready_status(&settings);
                set_status(&tray, &status_item, &status);
                tracing::info!("pipeline ready");
            }

            UserEvent::PipelineReady(Err(e)) => {
                tracing::error!("pipeline build failed: {e}");
                status = format!("Vox: {e}");
                set_status(&tray, &status_item, &status);
            }

            UserEvent::DownloadProgress(pct) => {
                status = format!("Vox: downloading speech model... {pct}%");
                set_status(&tray, &status_item, &status);
            }

            UserEvent::DownloadFinished(Ok(path)) => {
                tracing::info!("downloaded {}", path.display());
                status = "Vox: loading speech model...".into();
                set_status(&tray, &status_item, &status);
                spawn_pipeline_build(&proxy, settings.clone(), path);
            }

            UserEvent::DownloadFinished(Err(e)) => {
                tracing::error!("model download failed: {e}");
                status = "Vox: model download failed (run `vox download-model`)".into();
                set_status(&tray, &status_item, &status);
            }
        }
    });
}

const TICK: Duration = Duration::from_millis(250);

#[allow(clippy::too_many_arguments)]
fn periodic(
    last_polled_mtime: &mut Option<SystemTime>,
    settings: &mut Settings,
    pipeline_settings: &mut Settings,
    ptt: &mut PushToTalk,
    hotkey_manager: &GlobalHotKeyManager,
    current_hotkey: &mut Option<HotKey>,
    hotkey_hint: &MenuItem,
    proxy: &EventLoopProxy<UserEvent>,
    model_path: &dyn Fn(&Settings) -> PathBuf,
    recorder: &mut Recorder,
    cancel_registered: &mut bool,
    cancel_hotkey: &HotKey,
    tray: &TrayIcon,
    icons: &TrayIcons,
    status_item: &MenuItem,
    status: &mut String,
    processing: &mut usize,
    pipeline: &Option<Arc<Pipeline>>,
) {
    // Safety net for a lost key-up: never record past the cap.
    if recorder.elapsed().is_some_and(|e| e >= MAX_RECORDING) {
        tracing::warn!(
            "recording hit the {}s cap; finishing",
            MAX_RECORDING.as_secs()
        );
        ptt.reset();
        finish_recording(
            recorder,
            pipeline,
            proxy,
            settings,
            hotkey_manager,
            cancel_hotkey,
            cancel_registered,
            tray,
            icons,
            status_item,
            status,
            processing,
        );
    }

    let path = config::settings_path();
    let Some(mtime) = config::settings_mtime(&path) else {
        return;
    };
    if Some(mtime) == *last_polled_mtime {
        return;
    }
    *last_polled_mtime = Some(mtime);
    let new_settings = Settings::load();

    if new_settings.hotkey != settings.hotkey {
        if let Some(old) = current_hotkey.take() {
            let _ = hotkey_manager.unregister(old);
        }
        let mut error = None;
        if new_settings.hotkey.enabled {
            match register_hotkey(hotkey_manager, &new_settings.hotkey) {
                Ok(k) => *current_hotkey = Some(k),
                Err(e) => {
                    tracing::warn!("hotkey re-register failed: {e}");
                    error = Some(e.to_string());
                    // best-effort restore of the previous binding
                    if let Ok(k) = register_hotkey(hotkey_manager, &settings.hotkey) {
                        *current_hotkey = Some(k);
                    }
                }
            }
        }
        hotkey_hint.set_text(hotkey_hint_text(&new_settings.hotkey, error.as_deref()));
        ptt.set_mode(new_settings.hotkey.mode);
    }

    if pipeline_settings_changed(pipeline_settings, &new_settings) {
        let model = model_path(&new_settings);
        if model.exists() {
            spawn_pipeline_build(proxy, new_settings.clone(), model);
        } else {
            tracing::warn!(
                "whisper model {} not found; keeping the current one",
                model.display()
            );
        }
        *pipeline_settings = new_settings.clone();
    }
    recorder.set_device(new_settings.input_device.clone());
    *settings = new_settings;
}

#[allow(clippy::too_many_arguments)]
fn finish_recording(
    recorder: &mut Recorder,
    pipeline: &Option<Arc<Pipeline>>,
    proxy: &EventLoopProxy<UserEvent>,
    settings: &Settings,
    hotkey_manager: &GlobalHotKeyManager,
    cancel_hotkey: &HotKey,
    cancel_registered: &mut bool,
    tray: &TrayIcon,
    icons: &TrayIcons,
    status_item: &MenuItem,
    status: &mut String,
    processing: &mut usize,
) {
    unregister_cancel(hotkey_manager, cancel_hotkey, cancel_registered);
    let audio = match recorder.stop() {
        Ok(a) => a,
        Err(e) => {
            tracing::warn!("recorder stop failed: {e}");
            set_look(tray, icons, look_for(false, *processing));
            return;
        }
    };
    if settings.sound_cues {
        cue::play(Cue::Stop);
    }
    let Some(p) = pipeline.as_ref() else {
        set_look(tray, icons, look_for(false, *processing));
        return;
    };

    // App detection stays on the main thread (AppKit requires it).
    let ctx = match p.detector.current() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("app detection failed (canceling dictation): {e}");
            *status = "Vox: couldn't detect the active app".into();
            set_status(tray, status_item, status);
            set_look(tray, icons, look_for(false, *processing));
            return;
        }
    };

    *processing += 1;
    *status = "Vox: transcribing...".into();
    set_status(tray, status_item, status);
    set_look(tray, icons, Look::Processing);

    let p = Arc::clone(p);
    let proxy = proxy.clone();
    thread::spawn(move || {
        let result = p
            .prepare_with_ctx(&audio, ctx, Mode::Auto)
            .map_err(|e| e.to_string());
        let _ = proxy.send_event(UserEvent::Done(result));
    });
}

fn register_cancel(manager: &GlobalHotKeyManager, key: &HotKey, registered: &mut bool) {
    if !*registered {
        *registered = manager.register(*key).is_ok();
    }
}

fn unregister_cancel(manager: &GlobalHotKeyManager, key: &HotKey, registered: &mut bool) {
    if *registered {
        let _ = manager.unregister(*key);
        *registered = false;
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

fn status_for_strategy(strategy: &str) -> &'static str {
    if strategy.contains("clipboard kept") {
        "Vox: paste sent (clipboard kept)"
    } else if strategy.starts_with("auto-paste") {
        "Vox: inserted at cursor"
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
