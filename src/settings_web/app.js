/* global VoxDictionary, VoxProfiles, VoxHistory */

const state = {
  settings: null,
  models: { whisper: [], ollama: [] },
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

function showPage(name) {
  document.querySelectorAll(".page").forEach((el) => el.classList.remove("active"));
  document.querySelectorAll("nav.tabs button").forEach((btn) => {
    btn.classList.toggle("active", btn.dataset.page === name);
  });
  const page = document.getElementById(`page-${name}`);
  if (page) page.classList.add("active");
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

function applySettingsToForm(s) {
  document.getElementById("input_language").value = s.input_language || "auto";
  document.getElementById("output_language").value = s.output_language || "auto";
  document.getElementById("auto_paste").checked = !!s.auto_paste;
  document.getElementById("system_override").value = s.system_message_override || "";
  document.getElementById("hotkey_enabled").checked = !!(s.hotkey && s.hotkey.enabled);
  document.getElementById("hotkey_modifiers").value = (s.hotkey?.modifiers || []).join(",");
  document.getElementById("hotkey_key").value = s.hotkey?.key || "Space";
  fillSelect(document.getElementById("whisper_model"), state.models.whisper, s.whisper_model);
  fillSelect(document.getElementById("llm_model"), state.models.ollama, s.llm_model);
}

function readGeneralPatch() {
  const mods = document
    .getElementById("hotkey_modifiers")
    .value.split(",")
    .map((s) => s.trim())
    .filter(Boolean);
  const override = document.getElementById("system_override").value.trim();
  return {
    input_language: document.getElementById("input_language").value.trim() || "auto",
    output_language: document.getElementById("output_language").value.trim() || "auto",
    whisper_model: document.getElementById("whisper_model").value,
    llm_model: document.getElementById("llm_model").value,
    auto_paste: document.getElementById("auto_paste").checked,
    hotkey: {
      enabled: document.getElementById("hotkey_enabled").checked,
      modifiers: mods,
      key: document.getElementById("hotkey_key").value.trim() || "Space",
    },
    custom_dictionary: state.settings.custom_dictionary || [],
    custom_aliases: state.settings.custom_aliases || {},
    profiles: state.settings.profiles,
    system_message_override: override ? override : null,
  };
}

async function refreshSystemPreview() {
  const patch = readGeneralPatch();
  const res = await api("/api/system-message", {
    method: "POST",
    body: JSON.stringify(patch),
  });
  document.getElementById("system_preview").value = res.system_message || "";
}

async function saveGeneral() {
  const status = document.getElementById("general_status");
  status.textContent = "Saving…";
  try {
    const patch = readGeneralPatch();
    state.settings = await api("/api/settings", {
      method: "POST",
      body: JSON.stringify(patch),
    });
    status.textContent = "Saved";
    await refreshSystemPreview();
  } catch (e) {
    status.textContent = `Error: ${e.message}`;
  }
}

async function boot() {
  document.querySelectorAll("nav.tabs button").forEach((btn) => {
    btn.addEventListener("click", () => {
      showPage(btn.dataset.page);
      if (btn.dataset.page === "history" && window.VoxHistory?.reload) {
        window.VoxHistory.reload();
      }
    });
  });
  document.getElementById("save_general").addEventListener("click", saveGeneral);
  ["input_language", "output_language", "system_override"].forEach((id) => {
    document.getElementById(id).addEventListener("change", () => refreshSystemPreview().catch(() => {}));
  });

  const [settings, models] = await Promise.all([
    api("/api/settings"),
    api("/api/models"),
  ]);
  state.settings = settings;
  state.models = models;
  applySettingsToForm(settings);

  if (window.VoxDictionary) VoxDictionary.mount(document.getElementById("page-dictionary"), state);
  if (window.VoxProfiles) VoxProfiles.mount(document.getElementById("page-profiles"), state);
  if (window.VoxHistory) VoxHistory.mount(document.getElementById("page-history"), state);

  await refreshSystemPreview();
}

boot().catch((e) => {
  document.body.insertAdjacentHTML(
    "beforeend",
    `<p style="padding:1.5rem;color:var(--danger)">Failed to load settings: ${e.message}</p>`
  );
});

window.VoxApp = { api, state, applySettingsToForm };
