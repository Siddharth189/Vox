use std::env;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use vox::audio::record_for;
use vox::config::Settings;
use vox::context::{AppDetector, StubAppDetector};
use vox::doctor;
use vox::error::{Result, VoxError};
use vox::inject::{AutoPasteInjector, ClipboardInjector, TextInjector};
use vox::model::{AppContext, AudioChunk, EnrichedRequest, Mode, Transcript};
use vox::permissions;
use vox::pipeline::Pipeline;
use vox::process::{OllamaProcessor, TextProcessor};
use vox::stt::{FakeTranscriber, LocalWhisper};

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
        "tray" => {
            let model = flag_value(&args, "--model").map(PathBuf::from);
            vox::tray::run(model)
        }
        "inject-test" => cmd_inject_test(&args),
        "-h" | "--help" | "help" => {
            print_usage();
            Ok(())
        }
        other => Err(VoxError::Other(format!("unknown command: {other}"))),
    }
}

fn print_usage() {
    eprintln!(
        "\
Vox — local voice dictation

Usage:
  vox check
  vox doctor [--json]
  vox demo [--app BUNDLE_ID] [--dry-run] [--input LANG] [--lang LANG] [--llm MODEL] \"text\"
  vox listen [--secs N] [--model PATH] [--dry-run]
  vox tray [--model PATH]
  vox inject-test [--app BUNDLE_ID] \"text\""
    );
}

fn reexec_as_tray() -> Result<()> {
    let exe = env::current_exe()?;
    let log_path = vox::config::data_dir().join("tray.log");
    vox::secure_fs::ensure_private_dir(&vox::config::data_dir())?;
    // Pre-create log with 0600
    {
        use std::fs::OpenOptions;
        use std::os::unix::fs::OpenOptionsExt;
        let _ = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(&log_path)?;
    }

    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;
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
        Err(VoxError::Other(format!(
            "tray exited with {status}"
        )))
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
            println!("ollama ok — model replied:");
            println!("{}", p.text);
            Ok(())
        }
        Err(e) => Err(VoxError::Other(format!("ollama check failed: {e}"))),
    }
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

    let mut processor = OllamaProcessor::from_settings(&settings)?;
    if let Some(m) = flag_value(args, "--llm") {
        processor = processor.with_model(m);
    }

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
    );

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
    println!("inject:{} ({})", report.injection.strategy, report.injection.injected_chars);
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

    println!("recording {secs}s…");
    let audio = record_for(secs)?;
    println!(
        "captured {:.2}s @ {} Hz",
        audio.duration_secs(),
        audio.sample_rate
    );

    let transcriber = LocalWhisper::new(&model_path)?
        .with_input_language(settings.input_language.clone())
        .with_initial_prompt(&settings.custom_dictionary);
    let processor = OllamaProcessor::from_settings(&settings)?;
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
        "inject: {} ({} chars)",
        report.injection.strategy, report.injection.injected_chars
    );
    Ok(())
}

fn cmd_inject_test(args: &[String]) -> Result<()> {
    let text = positional_text(args)
        .ok_or_else(|| VoxError::Other("inject-test requires text".into()))?;
    let app = flag_value(args, "--app").unwrap_or_else(|| "unknown".into());
    let settings = Settings::load();
    let ctx = AppContext {
        bundle_id: app.clone(),
        app_name: app,
        window_title: None,
        profile: settings.profile_for("default"),
    };
    let result = AutoPasteInjector.inject(&text, &ctx)?;
    println!("{} — {} chars", result.strategy, result.injected_chars);
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
            if matches!(
                a.as_str(),
                "--app" | "--input" | "--lang" | "--llm" | "--model" | "--secs"
            ) {
                skip_next = true;
            }
            continue;
        }
        return Some(a.clone());
    }
    None
}
