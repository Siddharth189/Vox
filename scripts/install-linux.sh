#!/usr/bin/env bash
# install-linux.sh - end-to-end Vox installer for Linux.
#
# Primary target is Fedora (dnf); apt and pacman get a best-effort package
# list. Vox's Linux support is younger than its macOS support: the tray,
# global hotkey, and auto-paste all depend on your desktop session (X11 vs
# Wayland, and which compositor) in ways `vox doctor` will tell you about
# after install. See README.md's "Linux support" section for what's solid
# and what's a known gap on your setup.
set -euo pipefail

ROOT_HINT="$(cd "$(dirname "$0")/.." && pwd)"
VOX_REPO_URL="${VOX_REPO_URL:-}"
DRY_RUN="${VOX_INSTALL_DRY_RUN:-0}"

run() {
  if [[ "${DRY_RUN}" == "1" ]]; then
    echo "+ $*"
  else
    echo "+ $*"
    "$@"
  fi
}

require_linux() {
  if [[ "$(uname -s)" != "Linux" ]]; then
    echo "this script is for Linux; see scripts/install.sh for macOS" >&2
    exit 1
  fi
}

pkg_manager() {
  if command -v dnf >/dev/null 2>&1; then
    echo "dnf"
  elif command -v apt-get >/dev/null 2>&1; then
    echo "apt"
  elif command -v pacman >/dev/null 2>&1; then
    echo "pacman"
  else
    echo "none"
  fi
}

# Build deps (alsa/cpal, libxdo/enigo, gtk3+appindicator/tray-icon, cmake+cc
# for whisper-rs) plus kdotool/xdotool for per-app profile detection.
install_build_deps() {
  local pm
  pm="$(pkg_manager)"
  case "${pm}" in
    dnf)
      run sudo dnf install -y \
        cmake gcc gcc-c++ pkgconf-pkg-config \
        alsa-lib-devel libxdo-devel gtk3-devel \
        libayatana-appindicator-gtk3-devel \
        kdotool xdotool
      ;;
    apt)
      run sudo apt-get update
      run sudo apt-get install -y \
        cmake build-essential pkg-config \
        libasound2-dev libxdo-dev libgtk-3-dev \
        libayatana-appindicator3-dev \
        xdotool
      echo "note: kdotool isn't packaged for apt-based distros yet;" >&2
      echo "      install it from https://github.com/jinliu/kdotool if you're on KDE Wayland" >&2
      ;;
    pacman)
      run sudo pacman -S --needed \
        cmake gcc pkgconf \
        alsa-lib libxdo gtk3 libayatana-appindicator \
        xdotool
      echo "note: kdotool may need an AUR helper (e.g. yay -S kdotool) on Arch" >&2
      ;;
    *)
      echo "no supported package manager found (looked for dnf, apt, pacman)" >&2
      echo "install manually: cmake, a C compiler, alsa-lib-devel, libxdo-devel," >&2
      echo "gtk3-devel, libayatana-appindicator-gtk3-devel, and xdotool/kdotool" >&2
      exit 1
      ;;
  esac
}

if ! command -v cmake >/dev/null 2>&1 || ! pkg-config --exists alsa 2>/dev/null; then
  install_build_deps
fi

if ! command -v rustc >/dev/null 2>&1; then
  echo "Rust not found. Install it from https://rustup.rs and re-run this script." >&2
  exit 1
fi

if ! command -v ollama >/dev/null 2>&1; then
  echo "installing Ollama…"
  run bash -c "curl -fsSL https://ollama.com/install.sh | sh"
fi

require_linux

# Resolve source dir
if [[ -f "${ROOT_HINT}/Cargo.toml" ]]; then
  SRC="${ROOT_HINT}"
elif [[ -n "${VOX_REPO_URL}" ]]; then
  SRC="${HOME}/.cache/vox-src"
  if [[ ! -d "${SRC}/.git" ]]; then
    run git clone "${VOX_REPO_URL}" "${SRC}"
  fi
else
  echo "cannot find Vox sources; set VOX_REPO_URL or run from a checkout" >&2
  exit 1
fi

cd "${SRC}"
run cargo build --release --locked

mkdir -p "${HOME}/.local/bin"
run cp -f "${SRC}/target/release/vox" "${HOME}/.local/bin/vox"
run chmod 755 "${HOME}/.local/bin/vox"

# Stop any running tray before replacing it
run bash -c "pkill -f '${HOME}/.local/bin/vox tray' 2>/dev/null || true"

run "${SRC}/scripts/download_model.sh" small

# Start Ollama if needed
if ! curl -sf --max-time 2 "http://127.0.0.1:11434/api/tags" >/dev/null 2>&1; then
  if command -v systemctl >/dev/null 2>&1 && systemctl --user list-unit-files ollama.service >/dev/null 2>&1; then
    run systemctl --user start ollama || true
  else
    run bash -c 'nohup ollama serve >/dev/null 2>&1 &'
  fi
  for _ in $(seq 1 20); do
    if curl -sf --max-time 1 "http://127.0.0.1:11434/api/tags" >/dev/null 2>&1; then
      break
    fi
    sleep 1
  done
fi

# On memory-constrained machines the default 3B model can take minutes per
# request (observed: 4+ minutes, well past Vox's 60s request timeout, once
# the desktop session + Ollama's model together exceed physical RAM and the
# kernel starts swapping). Below ~10GB total RAM, default to the 1B model
# instead - it responds in single-digit seconds. Override with VOX_MODEL if
# you'd rather have the 3B model's better instruction-following and don't
# mind the latency (or have more RAM than this heuristic assumes).
if [[ -z "${VOX_MODEL:-}" ]]; then
  TOTAL_RAM_KB="$(awk '/MemTotal/ {print $2}' /proc/meminfo 2>/dev/null || echo 0)"
  if [[ "${TOTAL_RAM_KB}" -gt 0 && "${TOTAL_RAM_KB}" -lt 10485760 ]]; then
    VOX_MODEL="llama3.2:1b"
    echo "note: ${TOTAL_RAM_KB}KB total RAM detected; defaulting to llama3.2:1b for a responsive experience (set VOX_MODEL to override)"
  else
    VOX_MODEL="llama3.2:latest"
  fi
fi
MODEL="${VOX_MODEL}"
PULL="${MODEL%:latest}"
run ollama pull "${PULL}"

DATA_DIR="${XDG_DATA_HOME:-${HOME}/.local/share}/vox"
run mkdir -p "${DATA_DIR}"
run chmod 700 "${DATA_DIR}"
SETTINGS="${DATA_DIR}/settings.yaml"

if [[ ! -f "${SETTINGS}" ]]; then
  if [[ "${DRY_RUN}" == "1" ]]; then
    echo "+ write default settings.yaml"
  else
    "${HOME}/.local/bin/vox" doctor >/dev/null 2>&1 || true
    if [[ ! -f "${SETTINGS}" ]]; then
      cat > "${SETTINGS}" <<EOF
auto_paste: true
output_language: auto
input_language: auto
whisper_model: ${DATA_DIR}/models/ggml-small.bin
llm_model: ${MODEL}
custom_dictionary: []
custom_aliases: {}
hotkey:
  enabled: true
  modifiers: [control, alt]
  key: Space
profiles:
  default:
    format: clean_prose
    privacy: local_only
EOF
      chmod 600 "${SETTINGS}"
    fi
  fi
fi

run bash -c "'${HOME}/.local/bin/vox' doctor || true"

LOG="${DATA_DIR}/tray.log"
if [[ "${DRY_RUN}" != "1" ]]; then
  touch "${LOG}"
  chmod 600 "${LOG}"
fi

# systemd user service, the Linux-native equivalent of the macOS LaunchAgent-
# style background start. Not enabled by default: a hotkey app fighting for
# the same key bindings across every login is a bigger annoyance than typing
# one command, and Wayland global hotkeys are compositor-dependent enough
# that you'll likely want to watch the first run's output.
UNIT_DIR="${XDG_CONFIG_HOME:-${HOME}/.config}/systemd/user"
run mkdir -p "${UNIT_DIR}"
if [[ "${DRY_RUN}" != "1" ]]; then
  cat > "${UNIT_DIR}/vox.service" <<EOF
[Unit]
Description=Vox voice dictation tray
After=graphical-session.target

[Service]
ExecStart=${HOME}/.local/bin/vox tray
Restart=on-failure

[Install]
WantedBy=graphical-session.target
EOF
  run systemctl --user daemon-reload
fi

echo "Vox installed. CLI: ~/.local/bin/vox"
echo ""
echo "Start it now with:   vox tray"
echo "Or run it on login:  systemctl --user enable --now vox.service"
echo ""
echo "Settings UI (while the tray is running): http://127.0.0.1:8722"
