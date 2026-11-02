# Platform notes

What's specific to each OS: measured performance, known limits, and why they
exist. Vox's pipeline (capture, Whisper, cleanup, history, settings UI) is the
same everywhere; what differs is the part that touches the desktop: the
global hotkey, which app is focused, and synthesizing the paste.

Run `vox doctor` first: it reports all of this for your machine, with a fix
for anything missing.

## macOS

- Whisper runs on the GPU via Metal. Tested on Apple Silicon, macOS 13+.
- Auto-paste needs the **Accessibility** permission (System Settings > Privacy
  & Security > Accessibility > Vox). Without it, Vox falls back to
  `osascript`, and in the worst case leaves the text on your clipboard.
- Per-app profiles key off the bundle ID (`com.microsoft.VSCode`).
- `Vox.app` is a tray-only agent: no Dock icon.

## Windows

Tested on Windows 11 (i7-1185G7, 4 cores, no GPU) with the default `base-q5_1` model,
dictating into a real text box with the real hotkey and microphone:

| What | Result |
|---|---|
| Stop -> text at cursor, 3-5 s utterance | 0.7-1.1 s |
| Stop -> text at cursor, 18 s paragraph | ~2.9 s |
| Accuracy on those clips | word-perfect |
| Idle CPU / memory | ~0.02% / ~260 MB |

- **Per-app profiles** key off the executable name (`Code`, `slack`, `WindowsTerminal`,
  `powershell`). Terminals get the shell profile: no forced capital, no trailing period.
- **Apps running as administrator** can't receive synthesized keystrokes from a normal
  process (Windows UIPI). The text is still on the clipboard; press Ctrl+V. Run Vox elevated
  only if you dictate into elevated apps a lot.
- **`Ctrl+.` is global** while Vox runs, so it takes over VS Code's "Quick Fix" binding.
  Pick another shortcut in Settings if you rely on that.
- Dictations are excluded from Win+V clipboard history and cloud clipboard, and your
  previous clipboard is restored after each paste.
- Logs: `%LOCALAPPDATA%\Vox\tray.log`.

## Linux

The pipeline itself, audio capture, Whisper transcription, Ollama cleanup, the settings web
UI, and history, is identical on Linux, Windows, and macOS. What differs is the handful of things that
touch the desktop directly, and how well those work depends entirely on your session. This
section reflects real testing on Fedora 42 with KDE Plasma on Wayland, not assumptions.

- **Speech-to-text** runs on CPU via `whisper-rs` (no Metal-equivalent GPU backend is wired
  up) and works well: the `small` model (487MB resident) transcribed synthesized speech
  accurately in testing. On lower-RAM machines, `./scripts/download_model.sh tiny` or `base`
  trade accuracy for a smaller footprint.
- **The event loop had a real bug that made everything look broken.** `tao`'s
  `ControlFlow::WaitUntil` does not reliably re-arm on at least one real setup (KDE Plasma
  Wayland): it fired a handful of times at startup and then silently stopped, which meant
  the hotkey grab succeeded and events genuinely arrived in the channel, but nothing was
  left polling for them - holding the hotkey did nothing, and the "Settings..." menu item
  (which separately used the macOS-only `open` command with no Linux fallback) did nothing
  either. Fixed by parking the loop fully idle (`ControlFlow::Wait`, ~0% CPU) and waking it
  on a fixed cadence from a dedicated thread via `EventLoopProxy::send_event` - the same
  mechanism worker threads already use to deliver pipeline results back to the UI, so it
  doesn't depend on the platform re-arming a timer correctly. An earlier `Poll`-based
  workaround "fixed" the symptom but cost 30-50% constant CPU, which starved the actual
  transcription and LLM steps of CPU and made a real dictation take 113 seconds end to end.
- **Model size matters more than you'd expect on RAM-constrained machines, for both models.**
  This was the single biggest performance issue found in testing: on an 8GB machine, the
  default `llama3.2:latest` (3B, ~2GB) pushed the system into swap once the desktop
  session's own memory use was added in, and a single dictation's cleanup step took 4+
  minutes without completing, well past Vox's 60-second request timeout. Switching to
  `llama3.2:1b` (1.3GB) brought that down to single-digit seconds. Whisper's `small` model
  (487MB) was the other half of the same problem - about 50 seconds to transcribe a 7-second
  clip, competing with the LLM and the tray itself for the same limited cores; `base`
  (141MB) cut that to a few seconds with no noticeable accuracy loss on clear speech. The
  1B LLM tradeoff is real: it occasionally drops the subject of a sentence, invents small
  details, or (fixed in testing) prefaces its answer with unstripped meta-commentary like
  "I'll provide the corrected text:" that the 3B model didn't produce in the same tests.
  `vox doctor` checks your total RAM against the configured LLM's size and warns if they're
  a bad match. If you have 12GB+ RAM, the default 3B model plus the `small` Whisper model
  are worth keeping for the better quality; below that, `install-linux.sh` picks the faster
  pairing (`llama3.2:1b` + `base`) automatically.
- **Per-app profile detection is confirmed working** via `kdotool` on KDE Plasma, X11 or
  Wayland alike - verified against real windows in testing, including correctly identifying
  a Konsole window by its `org.kde.konsole` WM_CLASS. This is notable because the naive
  approach (`xdotool`/`xprop` against `_NET_ACTIVE_WINDOW`) does not work on Plasma Wayland:
  it returns a KWin-internal focus-proxy window, not your actual app, silently. `kdotool`
  goes through KWin's own scripting interface instead and doesn't have this problem. On a
  plain X11 session Vox falls back to `xdotool` directly. Neither present means every
  dictation uses the `default` profile rather than failing - `vox doctor` tells you which
  path you're on.
- **Auto-paste is the rough edge, and here's exactly why.** Plain XTest-based synthesis
  (`enigo`'s default backend, used through XWayland) reports success from Vox's side but did
  not reliably deliver keystrokes into a real Wayland-native window in testing - not
  Vox-specific: raw `xdotool type` and even `ydotool`'s kernel-level `uinput` injection
  (indistinguishable from real hardware input at the driver level) had the same result once
  verified against an actual window. So Vox now also tries `enigo`'s `libei_tokio` backend,
  which goes through the correct, sanctioned mechanism for this - an authorized
  `xdg-desktop-portal` RemoteDesktop session, the same one-time-consent model macOS's
  Accessibility permission uses. On this test machine that path hits what looks like a KDE
  portal bug (`xdp-kde-remotedesktop` fails its own internal permission-store lookup before
  ever showing the consent dialog, confirmed via `journalctl`), so it doesn't fully close the
  gap here - but the fix is real, is the officially-supported mechanism, and may well work on
  other Plasma versions or a fresh permission store. Either way, this path is now
  timeout-bounded (3s) specifically because a broken or absent portal must never turn into an
  indefinite hang for the whole pipeline - it always falls through to the clipboard. What
  works every time, confirmed in testing: **the cleaned-up text always lands on your
  clipboard**, the same safety net macOS falls back to when Accessibility isn't granted.
  `vox doctor` can't detect this specific failure mode since the synthesis call doesn't error
  from Vox's point of view, so treat "Vox: inserted at cursor" as optimistic on Wayland until
  you've confirmed it yourself once.
- **Global hotkey and tray icon** use the `global-hotkey` and `tray-icon` crates. Registration
  succeeds without error on this setup, and per-app detection working confirms the tray
  process itself runs correctly; the actual hold-to-talk key press wasn't verified end-to-end
  by automated testing (it requires physical keyboard input, same class of limitation as
  auto-paste above) - run `vox tray` from a terminal and try it once to confirm.
  `global-hotkey` grabs keys through X11, so on a **native Wayland session the hotkey only
  fires while an XWayland window is focused**. The proper fix is the
  `org.freedesktop.portal.GlobalShortcuts` portal (KDE 6, GNOME 48+), which isn't wired up
  yet. Until then, bind a desktop shortcut to `vox` or use an X11 session.

Run `vox doctor` after installing; it reports all of the above for your specific session,
with an install command or fix suggestion for whatever's missing.
