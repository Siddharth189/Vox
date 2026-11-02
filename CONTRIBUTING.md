# Contributing to Vox

Thanks for helping. Vox is a small Rust codebase (~10k lines) with one rule that shapes
everything: **dictations never leave the user's machine.** Keep that in mind and you'll fit
right in.

## Ways to help

- **Report a bug.** Use the bug template and include `vox doctor --json`. For transcription
  problems, attach a short WAV that reproduces it (`vox transcribe clip.wav` should show the
  issue).
- **Test your desktop.** Hotkeys, active-window detection, and pasting differ between
  compositors and window managers. A report like "works on Sway 1.10, paste fails in Firefox" is
  valuable even without a fix. There's an issue template for it.
- **Improve docs.** If something confused you, fix it for the next person.
- **Write code.** Look for [`good first issue`](https://github.com/Siddharth189/Vox/labels/good%20first%20issue)
  and [`help wanted`](https://github.com/Siddharth189/Vox/labels/help%20wanted), or pick a
  [roadmap](README.md#roadmap) item. For anything bigger than a bug fix, open an issue first so we
  can agree on the approach.

## Development setup

You need [Rust](https://rustup.rs) 1.89+, CMake, and a C/C++ compiler (whisper.cpp is built from
source by `whisper-rs`).

| OS | Extra requirements |
|---|---|
| macOS | Xcode Command Line Tools, `brew install cmake` |
| Windows | Visual Studio 2022 Build Tools (C++ workload), `winget install Kitware.CMake LLVM.LLVM` |
| Linux | `./scripts/install-linux.sh` installs everything (ALSA, libxdo, GTK 3, appindicator headers) |

```bash
cargo test                  # all tests; no microphone, model, or desktop needed
cargo clippy --all-targets  # lint
cargo fmt                   # format
cargo run -- doctor         # check your local setup
cargo run -- download-model # fetch the default speech model (57 MB)
cargo run -- tray           # run the app
```

The first build compiles whisper.cpp and takes a few minutes. Later builds are incremental.

### Fast feedback without a microphone

```bash
# Text cleanup, voice commands, snippets, and (if running) your LLM, without paste:
cargo run -- demo --dry-run "hi sam, new paragraph. um, ship it friday"

# Speech-to-text accuracy and speed on a recording:
cargo run --release -- transcribe clip.wav --repeat 3

# Paste on its own:
cargo run -- inject-test "hello from vox"
```

Set `VOX_DATA_DIR=/tmp/vox-dev` to keep development settings and history apart from your real ones.

## Where things live

```text
src/
  main.rs              CLI commands
  tray.rs              tray icon, event loop, hotkey handling, worker threads
  pipeline.rs          capture → privacy → STT → commands → cleanup → inject
  config.rs            settings.yaml schema, defaults, sanitization
  audio/               microphone capture, resampling, silence detection, sound cues
  stt/                 Whisper transcription
  context/             "which app is focused" (one file per OS)
  process/
    local.rs           rule-based cleanup (filler words, spacing, capitalization)
    commands.rs        "new line", "scratch that", snippets
    ollama.rs          LLM cleanup: Ollama and OpenAI-compatible servers
    prompt.rs          LLM system prompt and few-shot examples
    dictionary.rs      custom vocabulary and aliases
  inject/              clipboard and synthesized paste
  settings_web/        settings page (axum server + plain HTML/JS, embedded in the binary)
  doctor.rs            `vox doctor` checks
  stats.rs             usage totals
tests/                 pipeline integration tests
docs/                  ARCHITECTURE.md, PLATFORMS.md, SETTINGS_UI.md
```

Read [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) before changing the pipeline or tray.

## Common changes

**Add a setting.** Add the field to `Settings` in `config.rs` with `#[serde(default)]` so existing
`settings.yaml` files still load, sanitize it in `Settings::sanitize`, add it as an `Option` to
`SettingsPatch` in `settings_web/mod.rs`, and add it to `pipeline_settings_changed` in `tray.rs`
if it affects dictation. Add a test that an old settings file without the field still loads.

**Change cleanup behavior.** Rule-based cleanup is in `process/local.rs`; voice commands and
snippets are in `process/commands.rs`. Both are pure functions with table-style tests: add a
case for the input you're fixing, and a case for the input you must *not* change. Over-eager
cleanup that rewrites real words is worse than leaving a filler word in.

**Change the LLM prompt.** Edit `process/prompt.rs`, and update or add a test there. Small
models are sensitive to prompt length; check that the low-RAM trimmed prompt still makes sense.

**Support another desktop or OS.** Platform code is isolated behind traits: `AppDetector`
(`context/`), `TextInjector` (`inject/`), and autostart. Add a new implementation behind
`#[cfg(...)]` and leave the other platforms untouched. Fall back gracefully: an unknown app uses
the `default` profile, and a failed paste leaves text on the clipboard. Add a `doctor` check that
tells users which path they're on.

**Add a speech backend.** Implement the `Transcriber` trait in `stt/`. Keep it local: no network
calls during transcription.

## Pull requests

- Keep each PR focused on one change. Separate refactors from behavior changes.
- `cargo test` and `cargo clippy --all-targets` must pass. CI runs both on macOS, Linux, and Windows.
- Add or update tests for behavior changes. The pipeline is built from traits with fakes
  (`FakeTranscriber`, `FakeTextProcessor`, `FakeTextInjector`, `StubAppDetector`) so most logic
  can be tested without hardware.
- Update the README or docs when user-visible behavior changes, and add a line to
  `CHANGELOG.md` under "Unreleased".
- Never add a network call on the dictation path, telemetry, or a dependency on a cloud service.
  Optional integrations must default to local-only.
- Comments should explain *why*, especially for platform workarounds. Many existing comments
  record what was measured on real hardware. Keep that standard: say what you tested and on what.

## Commit messages

Use the imperative mood and describe the user-visible effect: "Fix paste hanging after the second
dictation on KDE", not "update autopaste.rs".

## Cutting a release

Maintainers only. Releases are built by `.github/workflows/release.yml`.

1. On `main`, set `version` in `Cargo.toml` and run `cargo check` to update `Cargo.lock`.
2. In `CHANGELOG.md`, rename "Unreleased" to `[X.Y.Z] - YYYY-MM-DD`, add a fresh empty
   "Unreleased" above it, and update the compare links at the bottom.
3. Merge that, then tag and push: `git tag vX.Y.Z && git push origin vX.Y.Z`.

The workflow refuses a tag that doesn't match `Cargo.toml` or has no changelog section. It builds
Windows, macOS (Apple Silicon), and Linux packages and publishes a GitHub Release with the
changelog section as its notes. Check the Actions tab if the release doesn't appear.

## License

By contributing, you agree that your contributions are licensed under the [MIT License](LICENSE).
