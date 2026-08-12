#!/usr/bin/env bash
# install.sh - end-to-end Vox installer for macOS.
set -euo pipefail

ROOT_HINT="$(cd "$(dirname "$0")/.." && pwd)"
VOX_REPO_URL="${VOX_REPO_URL:-}"
BUNDLE_ID="${VOX_BUNDLE_ID:-com.siddharth.vox}"
DRY_RUN="${VOX_INSTALL_DRY_RUN:-0}"

run() {
  if [[ "${DRY_RUN}" == "1" ]]; then
    echo "+ $*"
  else
    echo "+ $*"
    "$@"
  fi
}

require_macos() {
  if [[ "$(uname -s)" != "Darwin" ]]; then
    echo "Vox requires macOS" >&2
    exit 1
  fi
}

require_xcode_tools() {
  if ! xcode-select -p >/dev/null 2>&1; then
    echo "installing Xcode Command Line Tools…"
    run xcode-select --install || true
    echo "re-run install.sh after CLT installation completes" >&2
    exit 1
  fi
}

ensure_brew_pkg() {
  local pkg="$1"
  if ! command -v "$pkg" >/dev/null 2>&1; then
    if ! command -v brew >/dev/null 2>&1; then
      echo "Homebrew required to install ${pkg}" >&2
      exit 1
    fi
    run brew install "${pkg}"
  fi
}

require_macos
require_xcode_tools
ensure_brew_pkg cmake

if ! command -v rustc >/dev/null 2>&1; then
  ensure_brew_pkg rust
fi

if ! command -v ollama >/dev/null 2>&1; then
  ensure_brew_pkg ollama
fi

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

# Stop any running tray
run bash -c "pkill -f '${HOME}/Applications/Vox.app/Contents/MacOS/Vox' 2>/dev/null || true"
run bash -c "pkill -f '/Vox.app/Contents/MacOS/Vox' 2>/dev/null || true"

REPLACED=0
if [[ -d "${HOME}/Applications/Vox.app" ]]; then
  REPLACED=1
fi

run "${SRC}/scripts/package_app.sh"
run mkdir -p "${HOME}/Applications"
run rm -rf "${HOME}/Applications/Vox.app"
run cp -R "${SRC}/dist/Vox.app" "${HOME}/Applications/Vox.app"

if [[ "${REPLACED}" == "1" ]]; then
  run tccutil reset Accessibility "${BUNDLE_ID}" || true
fi

run "${SRC}/scripts/download_model.sh" small

# Start Ollama if needed
if ! curl -sf --max-time 2 "http://127.0.0.1:11434/api/tags" >/dev/null 2>&1; then
  run open -ga Ollama || true
  for _ in $(seq 1 20); do
    if curl -sf --max-time 1 "http://127.0.0.1:11434/api/tags" >/dev/null 2>&1; then
      break
    fi
    sleep 1
  done
  if ! curl -sf --max-time 2 "http://127.0.0.1:11434/api/tags" >/dev/null 2>&1; then
    run bash -c 'nohup ollama serve >/dev/null 2>&1 &'
    sleep 2
  fi
fi

MODEL="${VOX_MODEL:-llama3.2:latest}"
PULL="${MODEL%:latest}"
run ollama pull "${PULL}"

DATA_DIR="${HOME}/Library/Application Support/vox"
run mkdir -p "${DATA_DIR}"
run chmod 700 "${DATA_DIR}"
SETTINGS="${DATA_DIR}/settings.yaml"

if [[ ! -f "${SETTINGS}" ]]; then
  if [[ "${DRY_RUN}" == "1" ]]; then
    echo "+ write default settings.yaml"
  else
    "${HOME}/.local/bin/vox" doctor >/dev/null 2>&1 || true
    # Settings::load writes defaults on missing file when tray/doctor runs;
    # force via a tiny python/yaml write if still missing:
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
else
  # Migrate legacy relative whisper_model path
  if grep -q 'whisper_model: *["'\'']\?models/ggml-small.bin' "${SETTINGS}" 2>/dev/null; then
    if [[ "${DRY_RUN}" != "1" ]]; then
      sed -i.bak "s|whisper_model:.*models/ggml-small.bin.*|whisper_model: ${DATA_DIR}/models/ggml-small.bin|" "${SETTINGS}" || true
    else
      echo "+ migrate legacy whisper_model path"
    fi
  fi
fi

run bash -c "'${HOME}/.local/bin/vox' doctor || true"

LOG="${DATA_DIR}/tray.log"
if [[ "${DRY_RUN}" != "1" ]]; then
  touch "${LOG}"
  chmod 600 "${LOG}"
fi

if [[ "${VOX_START_TRAY:-1}" != "0" ]]; then
  run bash -c "nohup '${HOME}/Applications/Vox.app/Contents/MacOS/Vox' tray >> '${LOG}' 2>&1 &"
fi

echo "Vox installed. CLI: ~/.local/bin/vox  App: ~/Applications/Vox.app"
echo "Settings UI: http://127.0.0.1:8722"
