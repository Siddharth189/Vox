use std::env;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Instant;

use vox::audio::record_for;
use vox::config::Settings;
use vox::context::{AppDetector, StubAppDetector};
use vox::doctor;
use vox::error::{Result, VoxError};
use vox::inject::{AutoPasteInjector, ClipboardInjector, TextInjector};
use vox::model::{AppContext, AudioChunk, EnrichedRequest, Mode, Transcript};
use vox::permissions;
use vox::pipeline::Pipeline;
use vox::process::{CleanupProcessor, OllamaProcessor, TextProcessor, VoiceCommands};
use vox::stt::{FakeTranscriber, LocalWhisper, Transcriber};

fn main() {
    if let Err(e) = real_main() {
        eprintln!("vox: {e}");
        std::process::exit(1);
    }
}

fn real_main() -> Result<()> {
    let mut args: Vec<String> = env::args().skip(1).collect();

    // Finder / open-launch of Vox.app with no args → re-exec as `tray`
    if args.is_empty() && permissions::running_from_app_bundle() {
        return reexec_as_tray();
    }

    if args.is_empty() {
        print_usage();
        return Ok(());
    }

    let cmd = args.remove(0);
    match cmd.as_str() {
        "check" => cmd_check(),
        "doctor" => {
            let json = args.iter().any(|a| a == "--json");
            cmd_doctor(json)
        }
        "demo" => cmd_demo(&args),
        "listen" => cmd_listen(&args),
        "transcribe" => cmd_transcribe(&args),
        "tray" => {
            let model = flag_value(&args, "--model").map(PathBuf::from);
            vox::tray::run(model)
        }
        "inject-test" => cmd_inject_test(&args),
        "download-model" => cmd_download_model(&args),
        "setup" => cmd_setup(&args),
        "autostart" => cmd_autostart(&args),
        "stats" => cmd_stats(&args),
        "devices" => {
            let settings = Settings::load();
            for name in vox::audio::input_device_names() {
                let selected = settings
                    .input_device
                    .as_deref()
                    .is_some_and(|d| name.to_lowercase().contains(&d.to_lowercase()));
                println!("{} {name}", if selected { "*" } else { " " });
            }
            if settings.input_device.is_none() {
                println!("(using the system default microphone; set input_device in settings to pick one)");
            }
            Ok(())
        }
        "-V" | "--version" | "version" => {
            println!("vox {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        "-h" | "--help" | "help" => {
            print_usage();
            Ok(())
        }
        other => Err(VoxError::Other(format!(
            "unknown command: {other} (run `vox help`)"
        ))),
    }
}

fn print_usage() {
    eprintln!(
        "\
Vox {} - local voice dictation

Usage:
  vox tray [--model PATH]          run the tray app (hotkey: Ctrl+. by default)
  vox setup [--model NAME]         download the speech model and check everything
  vox doctor [--json]              readiness report
  vox download-model [NAME]        fetch a Whisper model (default: {}); `--list` shows choices
  vox transcribe FILE.wav [--model PATH] [--lang LANG] [--threads N] [--repeat N]
  vox listen [--secs N] [--model PATH] [--device NAME] [--dry-run]
  vox devices                      list microphones (* = selected)
  vox demo [--app APP_ID] [--dry-run] [--input LANG] [--lang LANG] [--llm MODEL] \"text\"
  vox inject-test [--app APP_ID] \"text\"
  vox autostart [on|off|status]    launch the tray at login
  vox stats [--json|--reset]       words dictated and typing time saved
  vox check                        quick LLM server (optional cleanup) check",
        env!("CARGO_PKG_VERSION"),
        vox::config::DEFAULT_WHISPER_MODEL_NAME
    );
}

fn reexec_as_tray() -> Result<()> {
    let exe = env::current_exe()?;
    let log = vox::secure_fs::open_private_append(&vox::config::log_path())?;
    let log_err = log.try_clone()?;

    let status = Command::new(&exe)
        .arg("tray")
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err))
        .status()?;

    if status.success() {
        Ok(())
    } else {
        Err(VoxError::Other(format!("tray exited with {status}")))
    }
}

fn cmd_check() -> Result<()> {
    let settings = Settings::load();
    let processor = OllamaProcessor::from_settings(&settings)?;
    let req = EnrichedRequest {
        transcript: Transcript::new("say hello in one short sentence"),
        ctx: AppContext::unknown(),
        mode: Mode::Auto,
    };
    match processor.process(&req) {
        Ok(p) => {
            println!("{} ok - model replied:", settings.llm_provider.label());
            println!("{}", p.text);
            Ok(())
        }
        Err(e) => Err(VoxError::Other(format!(
            "LLM check failed: {e} (optional: without an LLM server, Vox uses its built-in cleanup)"
        ))),
    }
}

fn cmd_stats(args: &[String]) -> Result<()> {
    if args.iter().any(|a| a == "--reset") {
        vox::stats::reset()?;
        println!("usage stats reset");
        return Ok(());
    }
    let summary = vox::stats::load().summary(vox::history::now_ms());
    if args.iter().any(|a| a == "--json") {
        println!("{}", serde_json::to_string_pretty(&summary)?);
        return Ok(());
    }
    if summary.dictations == 0 {
        println!("No dictations yet. Press Ctrl+. in any text field and start talking.");
        return Ok(());
    }
    println!("dictations      {}", summary.dictations);
    println!("words           {}", summary.words);
    println!(
        "speaking time   {:.1} min ({} wpm)",
        summary.speaking_minutes, summary.words_per_minute
    );
    println!(
        "time saved      {:.1} min vs typing at {} wpm",
        summary.time_saved_minutes, summary.typing_wpm_baseline
    );
    println!(
        "today / 7 days  {} / {} words",
        summary.words_today, summary.words_last_7_days
    );
    println!("streak          {} day(s)", summary.streak_days);
    Ok(())
}

fn cmd_doctor(json: bool) -> Result<()> {
    let settings = Settings::load();
    let report = doctor::run(&settings);
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        report.print_human();
    }
    if report.fail > 0 {
        std::process::exit(1);
    }
    Ok(())
}

/// CLI commands are one-shot, so wait for the Ollama probe instead of
/// using the tray's "assume unavailable until known" behaviour.
fn cleanup_processor(settings: &Settings) -> Result<CleanupProcessor> {
    let processor = CleanupProcessor::from_settings(settings)?;
    processor.probe_now();
    Ok(processor)
}

fn cmd_demo(args: &[String]) -> Result<()> {
    let text = positional_text(args).ok_or_else(|| VoxError::Other("demo requires text".into()))?;
    let dry_run = args.iter().any(|a| a == "--dry-run");
    let app = flag_value(args, "--app");
    let input = flag_value(args, "--input");
    let lang = flag_value(args, "--lang");
    let llm = flag_value(args, "--llm");

    let mut settings = Settings::load();
    if let Some(l) = input {
        settings.input_language = l;
    }
    if let Some(l) = lang {
        settings.output_language = l;
    }
    if let Some(m) = llm {
        settings.llm_model = m;
    }

    let bundle = app.unwrap_or_else(|| "default".into());
    let detector = StubAppDetector::with_bundle(&bundle, &bundle, &settings);
    let ctx = detector.current()?;

    let processor = cleanup_processor(&settings)?;
    println!(
        "cleanup: {}",
        if processor.llm_active() {
            settings.llm_provider.label()
        } else {
            "local rules"
        }
    );

    let pipeline = Pipeline::new(
        Box::new(StubAppDetector::new(ctx.clone())),
        Box::new(FakeTranscriber::new(text)),
        Box::new(processor),
        if dry_run {
            Box::new(vox::inject::FakeTextInjector::new())
        } else if settings.auto_paste {
            Box::new(AutoPasteInjector)
        } else {
            Box::new(ClipboardInjector)
        },
    )
    .with_commands(VoiceCommands::new(
        settings.voice_commands,
        &settings.snippets,
    ));

    let audio = AudioChunk {
        samples: vec![],
        sample_rate: 16_000,
    };
    let report = pipeline.run_with_ctx(&audio, ctx, Mode::Auto)?;
    println!("raw:  {}", report.prepared.raw_text);
    if let Some(m) = &report.prepared.model_text {
        println!("model:{m}");
    }
    println!("final:{}", report.prepared.processed_text);
    println!(
        "inject:{} ({})",
        report.injection.strategy, report.injection.injected_chars
    );
    vox::inject::wait_for_clipboard_restore(&report.injection.strategy);
    Ok(())
}

fn cmd_listen(args: &[String]) -> Result<()> {
    let secs: f32 = flag_value(args, "--secs")
        .and_then(|s| s.parse().ok())
        .unwrap_or(5.0);
    let dry_run = args.iter().any(|a| a == "--dry-run");
    let model = flag_value(args, "--model").map(PathBuf::from);

    let settings = Settings::load();
    let model_path = model.unwrap_or_else(|| PathBuf::from(&settings.whisper_model));

    let transcriber = LocalWhisper::new(&model_path)?
        .with_input_language(settings.input_language.clone())
        .with_initial_prompt(&settings.custom_dictionary);
    let processor = cleanup_processor(&settings)?;

    let device = flag_value(args, "--device").or_else(|| settings.input_device.clone());
    println!(
        "recording {secs}s from {}…",
        device.as_deref().unwrap_or("the default microphone")
    );
    let audio = record_for(secs, device)?;
    println!(
        "captured {:.2}s @ {} Hz",
        audio.duration_secs(),
        audio.sample_rate
    );

    let injector: Box<dyn TextInjector> = if dry_run {
        Box::new(vox::inject::FakeTextInjector::new())
    } else if settings.auto_paste {
        Box::new(AutoPasteInjector)
    } else {
        Box::new(ClipboardInjector)
    };

    let pipeline = Pipeline::new(
        Box::new(StubAppDetector::with_bundle("default", "CLI", &settings)),
        Box::new(transcriber),
        Box::new(processor),
        injector,
    );

    let report = pipeline.run(&audio, Mode::Auto)?;
    println!("raw:   {}", report.prepared.raw_text);
    println!("final: {}", report.prepared.processed_text);
    println!(
        "inject: {} ({} chars) | stt={}ms cleanup={}ms",
        report.injection.strategy,
        report.injection.injected_chars,
        report.latency.transcribe_ms,
        report.latency.process_ms
    );
    vox::inject::wait_for_clipboard_restore(&report.injection.strategy);
    Ok(())
}

fn cmd_transcribe(args: &[String]) -> Result<()> {
    let files = positional_all(args);
    if files.is_empty() {
        return Err(VoxError::Other("transcribe requires a .wav file".into()));
    }
    let settings = Settings::load();
    let model_path = flag_value(args, "--model")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(&settings.whisper_model));
    let lang = flag_value(args, "--lang").unwrap_or_else(|| settings.input_language.clone());
    let repeat: usize = flag_value(args, "--repeat")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1)
        .max(1);

    let load_start = Instant::now();
    let mut transcriber = LocalWhisper::new(&model_path)?
        .with_input_language(lang)
        .with_initial_prompt(&settings.custom_dictionary);
    if let Some(t) = flag_value(args, "--threads").and_then(|v| v.parse().ok()) {
        transcriber = transcriber.with_threads(t);
    }
    let load_ms = load_start.elapsed().as_millis();
    println!(
        "model: {} (load {load_ms}ms)",
        model_path.file_name().unwrap_or_default().to_string_lossy()
    );
    if args.iter().any(|a| a == "--warm") {
        let start = Instant::now();
        transcriber.warm_up();
        println!("warm-up: {}ms", start.elapsed().as_millis());
    }

    let ctx = AppContext::unknown();
    for file in files {
        let audio = vox::audio::wav::read_as_capture(std::path::Path::new(&file))?;
        if files_header_needed(args) {
            println!("file: {file}");
        }
        println!("audio: {:.2}s", audio.duration_secs());

        let mut timings = Vec::new();
        let mut raw = String::new();
        for _ in 0..repeat {
            let start = Instant::now();
            raw = transcriber.transcribe(&audio, &ctx)?.text;
            timings.push(start.elapsed().as_millis() as u64);
        }
        let best = *timings.iter().min().unwrap_or(&0);
        let first = timings[0];
        let rtf = best as f64 / 1000.0 / audio.duration_secs().max(0.001);
        println!("stt: first {first}ms, best {best}ms ({rtf:.2}x realtime)");
        println!("raw: {raw}");
        let cleaned = vox::process::clean_text(
            &raw,
            settings.profile_for("default").format,
            &settings.custom_dictionary,
            &settings.custom_aliases,
        );
        println!("final: {cleaned}");
    }
    Ok(())
}

fn files_header_needed(args: &[String]) -> bool {
    positional_all(args).len() > 1
}

/// Every non-flag argument (skipping flag values), e.g. multiple .wav files.
fn positional_all(args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut skip_next = false;
    for a in args {
        if skip_next {
            skip_next = false;
            continue;
        }
        if a.starts_with("--") {
            skip_next = flag_takes_value(a);
            continue;
        }
        out.push(a.clone());
    }
    out
}

fn flag_takes_value(flag: &str) -> bool {
    matches!(
        flag,
        "--app"
            | "--input"
            | "--lang"
            | "--llm"
            | "--model"
            | "--secs"
            | "--threads"
            | "--repeat"
            | "--device"
    )
}

fn cmd_download_model(args: &[String]) -> Result<()> {
    if args.iter().any(|a| a == "--list") {
        for (name, desc) in vox::models::KNOWN_MODELS {
            println!("{name:<22} {desc}");
        }
        return Ok(());
    }
    let name =
        positional_text(args).unwrap_or_else(|| vox::config::DEFAULT_WHISPER_MODEL_NAME.into());
    let path = download_with_progress(&name)?;

    let mut settings = Settings::load();
    let path_str = path.to_string_lossy().into_owned();
    if settings.whisper_model != path_str && !args.iter().any(|a| a == "--no-select") {
        settings.whisper_model = path_str;
        settings.save()?;
        println!("selected as the active model (restart-free: the tray reloads it)");
    }
    Ok(())
}

fn download_with_progress(name: &str) -> Result<PathBuf> {
    let name = vox::models::normalize_name(name)?;
    let dest = vox::config::whisper_model_path(&name);
    if dest.exists() && vox::models::is_valid_model_file(&dest) {
        println!("model already present: {}", dest.display());
        return Ok(dest);
    }
    println!("downloading ggml-{name}.bin ...");
    let mut last = u64::MAX;
    let path = vox::models::download(&name, |done, total| {
        let mb = done / 1_000_000;
        if mb == last {
            return;
        }
        last = mb;
        match total {
            Some(t) if t > 0 => eprint!("\r  {mb} / {} MB ({}%)   ", t / 1_000_000, done * 100 / t),
            _ => eprint!("\r  {mb} MB   "),
        }
        let _ = std::io::stderr().flush();
    })?;
    eprintln!();
    println!("installed {}", path.display());
    Ok(path)
}

fn cmd_setup(args: &[String]) -> Result<()> {
    let mut settings = Settings::load();
    let name = flag_value(args, "--model");
    let model = PathBuf::from(&settings.whisper_model);
    if name.is_some() || !model.exists() {
        let name = name.unwrap_or_else(|| vox::config::DEFAULT_WHISPER_MODEL_NAME.into());
        let path = download_with_progress(&name)?;
        settings.whisper_model = path.to_string_lossy().into_owned();
        settings.save()?;
    } else {
        println!("model already present: {}", model.display());
    }
    println!();
    let report = doctor::run(&settings);
    report.print_human();
    println!(
        "\nNext: start the tray (`vox tray`, or Vox from the Start menu/Applications), click into any text field, and press {} to dictate.",
        vox::hotkey::display_label(&settings.hotkey)
    );
    Ok(())
}

fn cmd_autostart(args: &[String]) -> Result<()> {
    match args.first().map(String::as_str).unwrap_or("status") {
        "on" | "enable" => {
            vox::autostart::set_enabled(true)?;
            println!("Vox will start at login");
        }
        "off" | "disable" => {
            vox::autostart::set_enabled(false)?;
            println!("Vox will not start at login");
        }
        _ => println!(
            "launch at login: {}",
            if vox::autostart::is_enabled() {
                "on"
            } else {
                "off"
            }
        ),
    }
    Ok(())
}

fn cmd_inject_test(args: &[String]) -> Result<()> {
    let text =
        positional_text(args).ok_or_else(|| VoxError::Other("inject-test requires text".into()))?;
    let app = flag_value(args, "--app").unwrap_or_else(|| "unknown".into());
    let settings = Settings::load();
    let ctx = AppContext {
        bundle_id: app.clone(),
        app_name: app,
        window_title: None,
        profile: settings.profile_for("default"),
    };
    let result = AutoPasteInjector.inject(&text, &ctx)?;
    println!("{} - {} chars", result.strategy, result.injected_chars);
    vox::inject::wait_for_clipboard_restore(&result.strategy);
    Ok(())
}

fn flag_value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1).cloned())
}

fn positional_text(args: &[String]) -> Option<String> {
    let mut skip_next = false;
    for a in args {
        if skip_next {
            skip_next = false;
            continue;
        }
        if a.starts_with("--") {
            skip_next = flag_takes_value(a);
            continue;
        }
        return Some(a.clone());
    }
    None
}
