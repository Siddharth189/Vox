#!/usr/bin/env bash
# bench.sh - fail when Whisper transcribes slower than a speed budget.
#
#   scripts/bench.sh target/release/vox [cache-dir]
#
# Transcribes whisper.cpp's public-domain JFK sample (11 s) with the default
# base-q5_1 model, best of 3 runs, and fails if that's slower than
# VOX_BENCH_MAX_FACTOR x real time (default 1.0) or the transcript is wrong.
# Correct builds run at 0.1-0.5x on CI runners; the MSVC-compiled Windows
# build that once shipped ran at ~3.6x, which this catches with room to
# spare for a slow runner.
set -euo pipefail

VOX="${1:?usage: bench.sh path/to/vox [cache-dir]}"
CACHE="${2:-${RUNNER_TEMP:-/tmp}/vox-bench}"
MAX="${VOX_BENCH_MAX_FACTOR:-1.0}"
WAV_URL="https://raw.githubusercontent.com/ggml-org/whisper.cpp/v1.7.6/samples/jfk.wav"
WAV_SHA256="59dfb9a4acb36fe2a2affc14bacbee2920ff435cb13cc314a08c13f66ba7860e"

mkdir -p "${CACHE}"
# Keeps the model and settings out of the runner's real data dir.
export VOX_DATA_DIR="${CACHE}/data"
WAV="${CACHE}/jfk.wav"

# Hashes stdin: given a filename with backslashes (Windows runners' temp
# dir), sha256sum escapes it and prefixes the hash with "\".
sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum <"$1" | cut -d' ' -f1
  else
    shasum -a 256 <"$1" | cut -d' ' -f1
  fi
}

if [[ ! -f "${WAV}" ]]; then
  curl -fsSL -o "${WAV}" "${WAV_URL}"
fi
if [[ "$(sha256 "${WAV}")" != "${WAV_SHA256}" ]]; then
  echo "::error::benchmark clip checksum mismatch: ${WAV}"
  exit 1
fi

# Idempotent: prints "model already present" when it's cached.
"${VOX}" download-model base-q5_1
MODEL="${VOX_DATA_DIR}/models/ggml-base-q5_1.bin"

# whisper.cpp logs to stderr; the result lines are on stdout.
out="$("${VOX}" transcribe "${WAV}" --model "${MODEL}" --repeat 3 2>/dev/null)"
echo "${out}"

if ! grep -qi "ask not what your country" <<<"${out}"; then
  echo "::error title=Benchmark transcript wrong::expected the JFK quote, got: $(grep '^raw:' <<<"${out}")"
  exit 1
fi

# "stt: first 2210ms, best 1830ms (0.17x realtime)"
factor="$(grep '^stt:' <<<"${out}" | sed -E 's/.*\(([0-9.]+)x realtime\).*/\1/')"
if [[ -z "${factor}" ]]; then
  echo "::error::couldn't read the speed from vox transcribe's output"
  exit 1
fi
if ! awk -v f="${factor}" -v m="${MAX}" 'BEGIN { exit !(f <= m) }'; then
  echo "::error title=Whisper too slow::${factor}x real time, budget ${MAX}x. A compiler or build-flag change is the usual cause (see cmake/portable-cpu.cmake and the clang-cl step)."
  exit 1
fi
echo "speed OK: ${factor}x real time (budget ${MAX}x)"

# Streaming: the same clip played in real time, cut at its pauses and
# transcribed while "speaking". The words must survive the cuts; the wait
# after stop is reported, not gated (shared runners are too noisy for that).
# "stt: 640ms after stop | 2 segments, 1 done while speaking"
sout="$("${VOX}" transcribe "${WAV}" --model "${MODEL}" --stream 2>/dev/null)"
echo "${sout}"
if ! grep -qi "ask not what your country" <<<"${sout}"; then
  echo "::error title=Streamed transcript wrong::expected the JFK quote, got: $(grep '^raw:' <<<"${sout}")"
  exit 1
fi
whole_ms="$(grep '^stt:' <<<"${out}" | sed -E 's/.*best ([0-9]+)ms.*/\1/')"
echo "::notice title=Streaming STT::$(grep '^stt:' <<<"${sout}") (one pass after stop: ${whole_ms}ms)"
