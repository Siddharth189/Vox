# Settings UI

Local web UI served by the tray process at [http://127.0.0.1:8722](http://127.0.0.1:8722).

## Pages

- **General** — input/output language, Whisper + Ollama model pickers, auto-paste,
  optional system-message override with live prompt preview, hotkey editor.
- **Dictionary** — custom dictionary terms + `Canonical = alias1, alias2` aliases.
- **Profiles** — per-bundle-id format/privacy table (`default` required).
- **History** — recent dictations with latency breakdown and inline correction
  learning (`POST /api/learn-correction`).

## Design notes

Minimal single-accent card layout, light/dark via `prefers-color-scheme`. No
external assets; HTML/JS are `include_str!`'d into the binary.
