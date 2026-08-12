use std::fs::OpenOptions;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use tao::event::{Event, StartCause};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
#[cfg(target_os = "macos")]
use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

use crate::audio::Recorder;
use crate::config::{self, HotkeyConfig, Settings};
use crate::context::AppDetector;
#[cfg(target_os = "macos")]
use crate::context::MacAppDetector;
#[cfg(not(target_os = "macos"))]
use crate::context::StubAppDetector;
use crate::error::{Result, VoxError};
use crate::history::{self, DictationRecord};
use crate::inject::{AutoPasteInjector, ClipboardInjector};
use crate::model::{elapsed_ms, InjectionResult, Mode};
use crate::permissions;
use crate::pipeline::{Pipeline, PreparedReport};
use crate::process::OllamaProcessor;
use crate::stt::LocalWhisper;

enum UserEvent {
    Done(std::result::Result<PreparedReport, String>),
    Injected {
        report: PreparedReport,
        injection: InjectionResult,
        inject_ms: u64,
    },
}

struct Runtime {
    settings: Settings,
    /// mtime of settings.yaml at last *pipeline* rebuild (not hotkey-only poll).
    pipeline_mtime: Option<SystemTime>,
    pipeline: Arc<Pipeline>,
    model_override: Option<PathBuf>,
}

impl Runtime {
    fn build(model_override: Option<PathBuf>) -> Result<Self> {
        let settings = Settings::load();
        let mtime = config::settings_mtime(&config::settings_path());
        let model_path = model_override
            .clone()
            .unwrap_or_else(|| PathBuf::from(&settings.whisper_model));
        let pipeline = Arc::new(build_pipeline(&settings, &model_path)?);
        Ok(Self {
            settings,
            pipeline_mtime: mtime,
            pipeline,
            model_override,
        })
    }

    fn refresh_if_changed(&mut self) -> Result<bool> {
        let path = config::settings_path();
        let mtime = config::settings_mtime(&path);
        if mtime == self.pipeline_mtime {
            return Ok(false);
        }
        let settings = Settings::load();
        // Hotkey-only edits are handled by the poll loop; skip Whisper/LLM reload.
        if !pipeline_settings_changed(&self.settings, &settings) {
            self.settings = settings;
            self.pipeline_mtime = mtime;
            return Ok(false);
        }
        let model_path = self
            .model_override
            .clone()
            .unwrap_or_else(|| PathBuf::from(&settings.whisper_model));
        let pipeline = Arc::new(build_pipeline(&settings, &model_path)?);
        self.settings = settings;
        self.pipeline_mtime = mtime;
        self.pipeline = pipeline;
        Ok(true)
    }
}

/// True when any setting that affects the dictation pipeline changed (not hotkey).
fn pipeline_settings_changed(old: &Settings, new: &Settings) -> bool {
    old.auto_paste != new.auto_paste
        || old.output_language != new.output_language
        || old.input_language != new.input_language
        || old.whisper_model != new.whisper_model
        || old.llm_model != new.llm_model
        || old.custom_dictionary != new.custom_dictionary
        || old.custom_aliases != new.custom_aliases
        || old.system_message_override != new.system_message_override
        || old.profiles != new.profiles
}

fn build_pipeline(settings: &Settings, model_path: &std::path::Path) -> Result<Pipeline> {
    #[cfg(target_os = "macos")]
    let detector: Box<dyn AppDetector> = Box::new(MacAppDetector::new(settings.clone()));
    #[cfg(not(target_os = "macos"))]
    let detector: Box<dyn AppDetector> = Box::new(StubAppDetector::with_bundle(
        "unknown",
        "Unknown",
        settings,
    ));

    let transcriber = LocalWhisper::new(model_path)?
        .with_input_language(settings.input_language.clone())
        .with_initial_prompt(&settings.custom_dictionary);

    let processor = OllamaProcessor::from_settings(settings)?;

    let injector: Box<dyn crate::inject::TextInjector> = if settings.auto_paste {
        Box::new(AutoPasteInjector)
    } else {
        Box::new(ClipboardInjector)
    };

    Ok(Pipeline::new(
        detector,
        Box::new(transcriber),
        Box::new(processor),
        injector,
    ))
}

struct InstanceLock {
    _file: std::fs::File,
}

impl InstanceLock {
    fn acquire() -> Result<Self> {
        let path = config::data_dir().join("vox.lock");
        crate::secure_fs::ensure_private_dir(&config::data_dir())?;
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .mode(0o600)
            .open(&path)?;
        let fd = file.as_raw_fd();
        let rc = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
        if rc != 0 {
            return Err(VoxError::Other(
                "another Vox tray instance is already running (vox.lock held)".into(),
            ));
        }
        Ok(Self { _file: file })
    }
}

pub fn run(model_override: Option<PathBuf>) -> Result<()> {
    let _lock = InstanceLock::acquire()?;
    crate::settings_web::start_background();

    let mut runtime = Runtime::build(model_override)?;
    let mut recorder = Recorder::new()?;

    let mut event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    #[cfg(target_os = "macos")]
    event_loop.set_activation_policy(ActivationPolicy::Accessory);
    let proxy = event_loop.create_proxy();

    let hotkey_manager = GlobalHotKeyManager::new()
        .map_err(|e| VoxError::Other(format!("hotkey manager: {e}")))?;
    let mut current_hotkey: Option<HotKey> = None;
    if runtime.settings.hotkey.enabled {
        match register_hotkey(&hotkey_manager, &runtime.settings.hotkey) {
            Ok(hk) => current_hotkey = Some(hk),
            Err(e) => eprintln!("warning: failed to register hotkey: {e}"),
        }
    }

    let menu = Menu::new();
    let status_item = MenuItem::new("Vox: idle", false, None);
    let hotkey_hint = MenuItem::new(hotkey_hint_text(&runtime.settings.hotkey), false, None);
    let settings_item = MenuItem::new("Settings...", true, None);
    let accessibility_item = MenuItem::new("Open Accessibility Settings...", true, None);
    let quit_item = MenuItem::new("Quit Vox", true, None);
    menu.append(&status_item)
        .map_err(|e| VoxError::Other(e.to_string()))?;
    menu.append(&hotkey_hint)
        .map_err(|e| VoxError::Other(e.to_string()))?;
    menu.append(&PredefinedMenuItem::separator())
        .map_err(|e| VoxError::Other(e.to_string()))?;
    menu.append(&settings_item)
        .map_err(|e| VoxError::Other(e.to_string()))?;
    menu.append(&accessibility_item)
        .map_err(|e| VoxError::Other(e.to_string()))?;
    menu.append(&quit_item)
        .map_err(|e| VoxError::Other(e.to_string()))?;

    let idle_icon = make_tray_icon(false);
    let recording_icon = make_tray_icon(true);

    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("Vox: idle")
        .with_icon(idle_icon.clone())
        .with_icon_as_template(true)
        .with_title("")
        .build()
        .map_err(|e| VoxError::Other(e.to_string()))?;

    let tray = Arc::new(Mutex::new(tray));
    let recording = Arc::new(Mutex::new(false));

    let menu_channel = MenuEvent::receiver();
    let hotkey_channel = GlobalHotKeyEvent::receiver();

    let settings_id = settings_item.id().clone();
    let accessibility_id = accessibility_item.id().clone();
    let quit_id = quit_item.id().clone();

    let mut last_hotkey_cfg = runtime.settings.hotkey.clone();
    // Separate from pipeline_mtime so hotkey-only polls don't swallow rebuilds.
    let mut last_polled_mtime = runtime.pipeline_mtime;

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(100));

        // Poll settings mtime for hotkey-only changes (do not advance pipeline_mtime).
        let path = config::settings_path();
        if let Some(mtime) = config::settings_mtime(&path) {
            if Some(mtime) != last_polled_mtime {
                let new_settings = Settings::load();
                if new_settings.hotkey != last_hotkey_cfg {
                    if let Some(old) = current_hotkey.take() {
                        let _ = hotkey_manager.unregister(old);
                    }
                    if new_settings.hotkey.enabled {
                        match register_hotkey(&hotkey_manager, &new_settings.hotkey) {
                            Ok(hk) => {
                                current_hotkey = Some(hk);
                                last_hotkey_cfg = new_settings.hotkey.clone();
                                hotkey_hint.set_text(hotkey_hint_text(&new_settings.hotkey));
                            }
                            Err(e) => {
                                eprintln!("warning: hotkey re-register failed: {e}");
                                // best-effort restore
                                if let Ok(hk) = register_hotkey(&hotkey_manager, &last_hotkey_cfg) {
                                    current_hotkey = Some(hk);
                                }
                            }
                        }
                    } else {
                        last_hotkey_cfg = new_settings.hotkey.clone();
                        hotkey_hint.set_text(hotkey_hint_text(&new_settings.hotkey));
                    }
                }
                // Do not assign runtime.settings here — refresh_if_changed compares
                // against the settings the pipeline was built with.
                last_polled_mtime = Some(mtime);
            }
        }

        while let Ok(MenuEvent { id }) = menu_channel.try_recv() {
            if id == settings_id {
                let _ = std::process::Command::new("open")
                    .arg("http://127.0.0.1:8722")
                    .status();
            } else if id == accessibility_id {
                permissions::request_accessibility_trust();
                permissions::open_accessibility_settings();
            } else if id == quit_id {
                *control_flow = ControlFlow::Exit;
            }
        }

        while let Ok(event) = hotkey_channel.try_recv() {
            let Some(ref hk) = current_hotkey else {
                continue;
            };
            if event.id != hk.id() {
                continue;
            }
            match event.state {
                HotKeyState::Pressed => {
                    if let Ok(mut rec) = recording.lock() {
                        if !*rec {
                            if let Err(e) = recorder.start() {
                                eprintln!("recorder start failed: {e}");
                            } else {
                                *rec = true;
                                set_tray_recording(&tray, &recording_icon, &status_item, true);
                            }
                        }
                    }
                }
                HotKeyState::Released => {
                    let was_recording = {
                        let mut rec = recording.lock().unwrap_or_else(|p| p.into_inner());
                        let was = *rec;
                        *rec = false;
                        was
                    };
                    if !was_recording {
                        continue;
                    }
                    set_tray_recording(&tray, &idle_icon, &status_item, false);
                    status_item.set_text("Vox: processing...");

                    let audio = match recorder.stop() {
                        Ok(a) => a,
                        Err(e) => {
                            eprintln!("recorder stop failed: {e}");
                            status_item.set_text("Vox: idle");
                            continue;
                        }
                    };

                    if let Err(e) = runtime.refresh_if_changed() {
                        eprintln!("pipeline refresh failed: {e}");
                    }

                    // Detect app on main thread
                    let ctx = match runtime.pipeline.detector.current() {
                        Ok(c) => c,
                        Err(e) => {
                            eprintln!("app detection failed (canceling dictation): {e}");
                            status_item.set_text("Vox: idle");
                            continue;
                        }
                    };

                    let pipeline = Arc::clone(&runtime.pipeline);
                    let proxy = proxy.clone();
                    thread::spawn(move || {
                        let result = pipeline
                            .prepare_with_ctx(&audio, ctx, Mode::Auto)
                            .map_err(|e| e.to_string());
                        let _ = proxy.send_event(UserEvent::Done(result));
                    });
                }
            }
        }

        if let Event::UserEvent(ref user) = event {
            match user {
                UserEvent::Done(Ok(report)) => {
                    if report.processed_text.is_empty() {
                        status_item.set_text("Vox: idle");
                    } else {
                        let pipeline = Arc::clone(&runtime.pipeline);
                        let proxy = proxy.clone();
                        let report = report.clone();
                        thread::spawn(move || {
                            let start = Instant::now();
                            let injection = pipeline
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
                    eprintln!("dictation failed: {e}");
                    status_item.set_text("Vox: idle");
                }
                UserEvent::Injected {
                    report,
                    injection,
                    inject_ms,
                } => {
                    let latency = report.latency.clone().with_injection(*inject_ms);
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
                        eprintln!("history append failed: {e}");
                    }

                    println!(
                        "vox: {} via {} | privacy={}ms stt={}ms llm={}ms inject={}ms total={}ms",
                        report.app_name,
                        injection.strategy,
                        latency.privacy_ms,
                        latency.transcribe_ms,
                        latency.process_ms,
                        latency.inject_ms,
                        latency.total_ms
                    );

                    let msg = status_for_strategy(&injection.strategy);
                    status_item.set_text(msg);
                    if let Ok(t) = tray.lock() {
                        let _ = t.set_tooltip(Some(msg));
                    }
                }
            }
        }

        if let Event::NewEvents(StartCause::Init) = event {
            // ready
        }
    });
}

fn status_for_strategy(strategy: &str) -> &'static str {
    if strategy.contains("clipboard kept") {
        "Vox: paste sent (clipboard kept)"
    } else if strategy.starts_with("auto-paste") {
        "Vox: inserted at cursor"
    } else if strategy.contains("Accessibility") {
        "Vox: clipboard — grant Accessibility"
    } else {
        "Vox: on clipboard (Cmd+V)"
    }
}

fn set_tray_recording(
    tray: &Arc<Mutex<TrayIcon>>,
    icon: &Icon,
    status_item: &MenuItem,
    recording: bool,
) {
    if recording {
        status_item.set_text("Vox: recording...");
        if let Ok(t) = tray.lock() {
            // Recording uses a red (non-template) icon; idle uses template tinting.
            let _ = t.set_icon_with_as_template(Some(icon.clone()), false);
            let _ = t.set_tooltip(Some("Vox: recording..."));
        }
    } else if let Ok(t) = tray.lock() {
        let _ = t.set_icon_with_as_template(Some(icon.clone()), true);
        let _ = t.set_tooltip(Some("Vox: idle"));
    }
}

fn hotkey_hint_text(cfg: &HotkeyConfig) -> String {
    if !cfg.enabled {
        return "Hotkey: disabled".into();
    }
    let mods = cfg.modifiers.join("+");
    format!("Hotkey: {mods}+{}", cfg.key)
}

fn register_hotkey(manager: &GlobalHotKeyManager, cfg: &HotkeyConfig) -> Result<HotKey> {
    let mut mods = Modifiers::empty();
    let mut has_primary = false;
    for m in &cfg.modifiers {
        match m.to_ascii_lowercase().as_str() {
            "control" | "ctrl" => {
                mods |= Modifiers::CONTROL;
                has_primary = true;
            }
            "alt" | "option" => {
                mods |= Modifiers::ALT;
                has_primary = true;
            }
            "shift" => {
                mods |= Modifiers::SHIFT;
            }
            "super" | "cmd" | "command" => {
                mods |= Modifiers::SUPER;
                has_primary = true;
            }
            _ => {}
        }
    }
    if !has_primary {
        return Err(VoxError::Config(
            "hotkey requires control, alt, or super (shift alone is rejected)".into(),
        ));
    }
    let code = Code::from_str(&cfg.key).unwrap_or(Code::Space);
    let hotkey = HotKey::new(Some(mods), code);
    manager
        .register(hotkey)
        .map_err(|e| VoxError::Other(format!("register hotkey: {e}")))?;
    Ok(hotkey)
}

/// Procedural 32x32 RGBA "sound bars" glyph; template-style (black or red with alpha mask).
fn make_tray_icon(recording: bool) -> Icon {
    const SIZE: u32 = 32;
    let mut rgba = vec![0u8; (SIZE * SIZE * 4) as usize];

    // 4 vertical capsules of varying height/position
    let bars = [
        (6.0f32, 22.0, 8.0),  // x-center, height, y-center
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
            let a = (alpha * 255.0) as u8;
            if recording {
                rgba[i] = 220;
                rgba[i + 1] = 40;
                rgba[i + 2] = 40;
            } else {
                rgba[i] = 0;
                rgba[i + 1] = 0;
                rgba[i + 2] = 0;
            }
            rgba[i + 3] = a;
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
