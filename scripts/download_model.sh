#!/usr/bin/env bash
# download_model.sh — fetch a ggml whisper model into the Vox models dir.
set -euo pipefail

NAME="${1:-small}"
MODELS_DIR="${VOX_MODELS_DIR:-${HOME}/Library/Application Support/vox/models}"
DEST="${MODELS_DIR}/ggml-${NAME}.bin"
URL="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-${NAME}.bin"

mkdir -p "${MODELS_DIR}"
chmod 700 "${MODELS_DIR}" 2>/dev/null || true

if [[ -f "${DEST}" ]]; then
  echo "model already present: ${DEST}"
  exit 0
fi

TMP="$(mktemp "${MODELS_DIR}/.ggml-${NAME}.XXXXXX")"
cleanup() { rm -f "${TMP}"; }
trap cleanup EXIT

download() {
  local url="$1"
  if command -v curl >/dev/null 2>&1; then
    curl -fL --retry 3 --retry-delay 2 -o "${TMP}" "${url}"
  else
    wget -O "${TMP}" "${url}"
  fi
}

echo "downloading ${URL}"
if ! download "${URL}"; then
  echo "plain download failed; trying proxy / redirect fallback…"
  PROXY="${VOX_PROXY:-${HTTPS_PROXY:-${HTTP_PROXY:-}}}"
  if [[ -n "${PROXY}" ]]; then
    REDIRECT="$(curl -sI -x "${PROXY}" -o /dev/null -w '%{url_effective}' "${URL}" || true)"
    if [[ -n "${REDIRECT}" && "${REDIRECT}" != "${URL}" ]]; then
      echo "following redirect via proxy: ${REDIRECT}"
      HTTPS_PROXY="${PROXY}" HTTP_PROXY="${PROXY}" download "${REDIRECT}"
    else
      HTTPS_PROXY="${PROXY}" HTTP_PROXY="${PROXY}" download "${URL}"
    fi
  else
    echo "download failed and no proxy configured" >&2
    exit 1
  fi
fi

# Verify ggml magic "lmgg" = 6c6d6767
MAGIC="$(xxd -p -l 4 "${TMP}" 2>/dev/null || od -An -tx1 -N4 "${TMP}" | tr -d ' \n')"
MAGIC="$(echo "${MAGIC}" | tr -d ' \n')"
if [[ "${MAGIC}" != "6c6d6767" ]]; then
  echo "bad magic bytes: ${MAGIC} (expected 6c6d6767)" >&2
  exit 1
fi

mv "${TMP}" "${DEST}"
chmod 600 "${DEST}"
trap - EXIT
echo "installed ${DEST}"
