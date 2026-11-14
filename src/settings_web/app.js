/* global VoxDictionary, VoxLanguages, VoxProfiles, VoxHistory, VoxSnippets */

const state = {
  settings: null,
  models: { whisper: [], ollama: [] },
  devices: [],
};

async function api(path, opts = {}) {
  const res = await fetch(path, {
    headers: { "Content-Type": "application/json", ...(opts.headers || {}) },
    ...opts,
  });
  if (!res.ok) {
    const text = await res.text();
    throw new Error(text || res.statusText);
  }
  if (res.status === 204) return null;
  const ct = res.headers.get("content-type") || "";
  if (ct.includes("application/json")) return res.json();
  return res.text();
}

function fillSelect(el, values, current) {
  el.innerHTML = "";
  const set = new Set(values);
  if (current && !set.has(current)) {
    const opt = document.createElement("option");
    opt.value = current;
    opt.textContent = current;
    el.appendChild(opt);
  }
  for (const v of values) {
    const opt = document.createElement("option");
    opt.value = v;
    opt.textContent = v;
    if (v === current) opt.selected = true;
    el.appendChild(opt);
  }
  if (current) el.value = current;
}

// Drafts of the hotkeys being edited, by id prefix: `key` is a
// KeyboardEvent.code name ("Period", "KeyD", "Space"), which is exactly
// what the backend parses.
const hotkeyDrafts = {
  hotkey: { modifiers: ["control"], key: "Period" },
  edit_hotkey: { modifiers: ["control", "shift"], key: "Period" },
};

const IS_MAC = /Mac|iPhone|iPad/.test(navigator.platform);
const IS_WIN = /Win/.test(navigator.platform);
const KEY_GLYPHS = {
  Period: ".", Comma: ",", Slash: "/", Backslash: "\\", Semicolon: ";", Quote: "'",
  Backquote: "`", Minus: "-", Equal: "=", BracketLeft: "[", BracketRight: "]",
};

function hotkeyLabel(mods, key) {
  const names = { control: "Ctrl", alt: IS_MAC ? "Option" : "Alt", shift: "Shift", super: IS_MAC ? "Cmd" : IS_WIN ? "Win" : "Super" };
  const parts = mods.map((m) => names[m] || m);
  let k = KEY_GLYPHS[key] || key;
  k = k.replace(/^Key/, "").replace(/^Digit/, "");
  parts.push(k);
  return parts.join(" + ");
}

function renderHotkey(prefix) {
  const draft = hotkeyDrafts[prefix];
  document.getElementById(`${prefix}_capture`).textContent = hotkeyLabel(draft.modifiers, draft.key);
}

function sameShortcut(a, b) {
  const mods = (d) => [...d.modifiers].sort().join("+");
  return a.key === b.key && mods(a) === mods(b);
}

function setupHotkeyCapture(prefix) {
  const btn = document.getElementById(`${prefix}_capture`);
  const hint = document.getElementById(`${prefix}_hint`);
  const draft = hotkeyDrafts[prefix];
  const defaultHint = hint.textContent;
  let listening = false;

  const stop = (message) => {
    listening = false;
    btn.classList.remove("recording");
    window.removeEventListener("keydown", onKey, true);
    renderHotkey(prefix);
    hint.textContent = message || defaultHint;
  };

  const onKey = (e) => {
    e.preventDefault();
    e.stopPropagation();
    if (e.code === "Escape" && !e.ctrlKey && !e.altKey && !e.metaKey) {
      stop();
      return;
    }
    if (/^(Control|Alt|Shift|Meta|OS)(Left|Right)?$/.test(e.code)) {
      return; // wait for the non-modifier key
    }
    const mods = [];
    if (e.ctrlKey) mods.push("control");
    if (e.altKey) mods.push("alt");
    if (e.shiftKey) mods.push("shift");
    if (e.metaKey) mods.push("super");
    if (!mods.some((m) => m !== "shift")) {
      hint.textContent = `Include ${IS_MAC ? "Ctrl, Option, or Cmd" : "Ctrl, Alt, or Win"} so normal typing isn't captured.`;
      return;
    }
    const other = prefix === "hotkey" ? hotkeyDrafts.edit_hotkey : hotkeyDrafts.hotkey;
    if (sameShortcut({ modifiers: mods, key: e.code }, other)) {
      hint.textContent = "That shortcut is already used for the other hotkey.";
      return;
    }
    draft.modifiers = mods;
    draft.key = e.code;
    stop("Click Save to apply.");
  };

  btn.addEventListener("click", () => {
    if (listening) {
      stop();
      return;
    }
    listening = true;
    btn.classList.add("recording");
    btn.textContent = "Press shortcut…";
    hint.textContent = "Press the new shortcut, or Esc to keep the current one.";
    window.addEventListener("keydown", onKey, true);
  });
}

function applySettingsToForm(s) {
  document.getElementById("auto_paste").checked = !!s.auto_paste;
  document.getElementById("llm_cleanup").checked = s.llm_cleanup !== false;
  document.getElementById("sound_cues").checked = s.sound_cues !== false;
  document.getElementById("voice_commands").checked = s.voice_commands !== false;
  document.getElementById("llm_provider").value = s.llm_provider || "ollama";
  document.getElementById("llm_base_url").value = s.llm_base_url || "";
  document.getElementById("streaming_transcription").checked = s.streaming_transcription !== false;
  document.getElementById("system_override").value = s.system_message_override || "";
  document.getElementById("live_text").checked = s.live_text !== false;
  for (const [prefix, fallback] of [["hotkey", ["control"]], ["edit_hotkey", ["control", "shift"]]]) {
    const cfg = s[prefix];
    document.getElementById(`${prefix}_enabled`).checked = !!(cfg && cfg.enabled);
    hotkeyDrafts[prefix].modifiers = (cfg?.modifiers || fallback).map((m) => m.toLowerCase().replace(/^ctrl$/, "control"));
    hotkeyDrafts[prefix].key = cfg?.key || "Period";
    document.getElementById(`${prefix}_mode`).value = cfg?.mode || "hybrid";
    renderHotkey(prefix);
  }
  fillSelect(document.getElementById("whisper_model"), state.models.whisper, s.whisper_model);
  fillSelect(document.getElementById("llm_model"), state.models.ollama, s.llm_model);
  fillDeviceSelect(s.input_device || "");
  if (window.VoxLanguages) VoxLanguages.refresh();
}

function fillDeviceSelect(current) {
  const el = document.getElementById("input_device");
  el.innerHTML = "";
  const def = document.createElement("option");
  def.value = "";
  def.textContent = "System default";
  el.appendChild(def);
  const names = [...state.devices];
  if (current && !names.includes(current)) names.unshift(current);
  for (const name of names) {
    const opt = document.createElement("option");
    opt.value = name;
    opt.textContent = state.devices.includes(name) ? name : `${name} (not connected)`;
    el.appendChild(opt);
  }
  el.value = current;
}

function readGeneralPatch() {
  const override = document.getElementById("system_override").value.trim();
  // The language dropdowns resolve "Other…" and older spellings for us.
  // If the dropdown script never loaded, keep what's saved rather than reset it.
  const lang = window.VoxLanguages
    ? VoxLanguages.values()
    : { input: state.settings.input_language || "auto", output: state.settings.output_language || "auto" };
  return {
    input_language: lang.input,
    output_language: lang.output,
    whisper_model: document.getElementById("whisper_model").value,
    llm_model: document.getElementById("llm_model").value,
    auto_paste: document.getElementById("auto_paste").checked,
    llm_cleanup: document.getElementById("llm_cleanup").checked,
    sound_cues: document.getElementById("sound_cues").checked,
    voice_commands: document.getElementById("voice_commands").checked,
    llm_provider: document.getElementById("llm_provider").value,
    llm_base_url: document.getElementById("llm_base_url").value.trim() || null,
    streaming_transcription: document.getElementById("streaming_transcription").checked,
    live_text: document.getElementById("live_text").checked,
    input_device: document.getElementById("input_device").value || null,
    hotkey: readHotkey("hotkey"),
    edit_hotkey: readHotkey("edit_hotkey"),
    custom_dictionary: state.settings.custom_dictionary || [],
    custom_aliases: state.settings.custom_aliases || {},
    profiles: state.settings.profiles,
    system_message_override: override ? override : null,
  };
}

function readHotkey(prefix) {
  return {
    enabled: document.getElementById(`${prefix}_enabled`).checked,
    modifiers: hotkeyDrafts[prefix].modifiers,
    key: hotkeyDrafts[prefix].key,
    mode: document.getElementById(`${prefix}_mode`).value,
  };
}

function systemMessageMeta() {
  const s = state.settings;
  const custom = (s.system_message_override || "").trim();
  const kind = custom ? "Custom override" : "Generated default";
  return `Default profile · Mode auto · Input ${s.input_language || "auto"} · Output ${s.output_language || "auto"} · ${kind}`;
}

async function refreshSystemPreview() {
  const patch = readGeneralPatch();
  const res = await api("/api/system-message", {
    method: "POST",
    body: JSON.stringify(patch),
  });
  document.getElementById("system_preview").value = res.system_message || "";
  document.getElementById("sysmsg_meta").textContent = systemMessageMeta();
}

async function saveGeneral() {
  const status = document.getElementById("general_status");
  const btn = document.getElementById("save_general");
  const original = btn.textContent;
  btn.textContent = "Saving…";
  btn.disabled = true;
  try {
    const patch = readGeneralPatch();
    state.settings = await api("/api/settings", {
      method: "POST",
      body: JSON.stringify(patch),
    });
    btn.textContent = "Saved";
    if (status) status.textContent = "";
    await refreshSystemPreview();
  } catch (e) {
    btn.textContent = original;
    if (status) status.textContent = `Error: ${e.message}`;
    else alert(`Error saving: ${e.message}`);
  } finally {
    setTimeout(() => {
      btn.textContent = original;
      btn.disabled = false;
    }, 1200);
  }
}

function setupSystemMessageControls() {
  document.getElementById("sysmsg_refresh").addEventListener("click", () => {
    refreshSystemPreview().catch((e) => alert(`Error: ${e.message}`));
  });
  document.getElementById("sysmsg_reset").addEventListener("click", () => {
    document.getElementById("system_override").value = "";
    refreshSystemPreview().catch(() => {});
  });
  document.getElementById("sysmsg_copy").addEventListener("click", async () => {
    const text = document.getElementById("system_preview").value;
    try {
      await navigator.clipboard.writeText(text);
    } catch (e) {
      // Clipboard API can be unavailable over plain http in some browsers;
      // fall back to a manual select so the user can still Ctrl+C.
      const el = document.getElementById("system_preview");
      el.focus();
      el.select();
    }
  });
}

function setupHistoryToggle() {
  const toggle = document.getElementById("history_toggle");
  const body = document.getElementById("history_body");
  let mounted = false;
  toggle.addEventListener("click", () => {
    const open = body.classList.toggle("open");
    toggle.textContent = open ? "Hide" : "Show";
    if (open) {
      if (!mounted && window.VoxHistory) {
        window.VoxHistory.mount(body, state);
        mounted = true;
      } else if (window.VoxHistory?.reload) {
        window.VoxHistory.reload();
      }
    }
  });
}

function formatMinutes(min) {
  if (min >= 90) return `${(min / 60).toFixed(1)} h`;
  return `${Math.round(min)} min`;
}

async function loadStats() {
  const s = await api("/api/stats");
  document.getElementById("stat_words").textContent = s.words.toLocaleString();
  document.getElementById("stat_saved").textContent = formatMinutes(s.time_saved_minutes);
  document.getElementById("stat_saved_label").textContent = `saved vs typing at ${s.typing_wpm_baseline} wpm`;
  document.getElementById("stat_wpm").textContent = s.words_per_minute ? Math.round(s.words_per_minute) : "–";
  document.getElementById("stat_streak").textContent = s.streak_days;
}

async function boot() {
  document.getElementById("save_general").addEventListener("click", saveGeneral);
  ["input_language", "output_language", "system_override"].forEach((id) => {
    document.getElementById(id).addEventListener("change", () => refreshSystemPreview().catch(() => {}));
  });
  setupSystemMessageControls();
  setupHistoryToggle();
  setupHotkeyCapture("hotkey");
  setupHotkeyCapture("edit_hotkey");

  const [settings, models, devices] = await Promise.all([
    api("/api/settings"),
    api("/api/models"),
    api("/api/devices").catch(() => []),
  ]);
  state.settings = settings;
  state.models = models;
  state.devices = devices || [];
  applySettingsToForm(settings);
  if (window.VoxLanguages) VoxLanguages.mount(state);

  if (window.VoxDictionary) VoxDictionary.mount(document.getElementById("section-dictionary"), state);
  if (window.VoxProfiles) VoxProfiles.mount(document.getElementById("section-profiles"), state);
  if (window.VoxSnippets) VoxSnippets.mount(document.getElementById("section-snippets"), state);
  loadStats().catch(() => {});

  await refreshSystemPreview();
}

boot().catch((e) => {
  document.body.insertAdjacentHTML(
    "beforeend",
    `<p style="padding:1.5rem;color:var(--danger)">Failed to load settings: ${e.message}</p>`
  );
});

window.VoxApp = { api, state, applySettingsToForm };
