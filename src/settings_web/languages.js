/* Language settings: two plain dropdowns, "Spoken language" and "Output
 * language", over the settings strings the backend already parses:
 *   input_language:  "auto" | "Hindi" | "English, Hindi" | "Hinglish / mixed Hindi-English" | ...
 *   output_language: "auto" | "English" | "Hindi" | "Hinglish / mixed Hindi-English" | any name
 *
 * The dropdowns carry what the old free-text boxes couldn't show: the
 * combinations people actually speak ("English + Hindi", Hinglish), and, for
 * output languages that need the local AI to translate, whether that will
 * work on this machine right now. A choice that can't work is never silently
 * accepted: its label and a note say so.
 */
(function () {
  const HINGLISH = "Hinglish / mixed Hindi-English";
  const OTHER = "__other__";
  const NAMES = [
    "Tamil", "Telugu", "Marathi", "Bengali", "Spanish", "Japanese", "French", "German",
    "Portuguese", "Chinese", "Arabic", "Korean", "Italian", "Russian",
  ];
  const SPOKEN_OPTIONS = [
    ["auto", "Auto-detect"],
    ["English", "English"],
    ["Hindi", "Hindi"],
    ["English, Hindi", "English + Hindi"],
    [HINGLISH, "Hinglish (Hindi & English mixed)"],
    ...NAMES.map((n) => [n, n]),
    [OTHER, "Other / several…"],
  ];
  const OUTPUT_OPTIONS = [
    ["auto", "Same as spoken"],
    ["English", "English"],
    ["Hindi", "Hindi (देवनागरी)"],
    [HINGLISH, "Hinglish (Latin letters)"],
    ...NAMES.map((n) => [n, n]),
    [OTHER, "Other…"],
  ];
  const CODES = {
    en: "English", hi: "Hindi", ta: "Tamil", te: "Telugu", mr: "Marathi", bn: "Bengali",
    es: "Spanish", fr: "French", de: "German", pt: "Portuguese", it: "Italian", ru: "Russian",
    ja: "Japanese", ko: "Korean", zh: "Chinese", ar: "Arabic",
  };
  const KNOWN = ["English", "Hindi", ...NAMES];

  let appState = null;
  let mounted = false;
  const $ = (id) => document.getElementById(id);

  /** Canonical form of a spoken-language setting ("en+hi" -> "English, Hindi"). */
  function canonicalSpoken(value) {
    const v = (value || "").trim();
    const lower = v.toLowerCase();
    if (!v || lower === "auto") return "auto";
    if (/hinglish|mixed|code-switch/.test(lower)) return HINGLISH;
    const out = [];
    for (const raw of v.split(/[,+/&;]| and /i)) {
      const p = raw.trim();
      if (!p) continue;
      const name = KNOWN.find((n) => n.toLowerCase() === p.toLowerCase()) ||
        CODES[p.toLowerCase()] || p;
      if (!out.some((o) => o.toLowerCase() === name.toLowerCase())) out.push(name);
    }
    return out.length ? out.join(", ") : "auto";
  }

  /** Canonical form of an output-language setting ("english" -> "English"). */
  function canonicalOutput(value) {
    const v = (value || "").trim();
    if (!v || v.toLowerCase() === "auto") return "auto";
    if (/hinglish|mixed|code-switch/i.test(v)) return HINGLISH;
    const known = KNOWN.find((n) => n.toLowerCase() === v.toLowerCase());
    return known || CODES[v.toLowerCase()] || v;
  }

  function llmReady() {
    return !!(appState && appState.models && appState.models.llm_ready);
  }

  function englishOnlyModel() {
    const model = ($("whisper_model") && $("whisper_model").value) || "";
    return /\.en[-.]/i.test(model.split(/[\\/]/).pop() || "");
  }

  /** Fills `select` with `options`, selecting `value`; a saved value that
   * isn't in the list ("English, Gujarati") is shown as its own entry. */
  function fill(select, options, value, labelFor) {
    select.innerHTML = "";
    const add = (v, label) => {
      const opt = document.createElement("option");
      opt.value = v;
      opt.textContent = labelFor(v, label);
      select.appendChild(opt);
    };
    if (!options.some(([v]) => v === value)) add(value, value);
    for (const [v, label] of options) add(v, label);
    select.value = value;
  }

  function outputLabel(value, label) {
    if (value === "auto" || value === OTHER || llmReady()) return label;
    return `${label} · needs local AI`;
  }

  /** The setting strings the dropdowns currently mean. */
  function values() {
    const read = (selectId, otherId) => {
      const v = $(selectId).value;
      if (v !== OTHER) return v;
      return $(otherId).value.trim() || "auto";
    };
    return {
      input: canonicalSpoken(read("input_language", "input_language_other")),
      output: canonicalOutput(read("output_language", "output_language_other")),
    };
  }

  function renderNote() {
    const { input, output } = values();
    const notes = [];
    if (output !== "auto" && !llmReady()) {
      const name = output === HINGLISH ? "Hinglish" : output;
      notes.push(`${name} output needs the local AI, which isn't running. Until it is, Vox ` +
        "inserts what you said, untranslated. Set it up under Models below.");
    }
    const spoken = input === HINGLISH ? ["English", "Hindi"] : input.split(", ");
    if (englishOnlyModel() && input !== "auto" && spoken.some((l) => l !== "English")) {
      notes.push("Your speech model only understands English. Pick one without “.en” under Models.");
    }
    const note = $("lang_note");
    note.textContent = notes.join(" ");
    note.hidden = notes.length === 0;
  }

  function showOther(selectId, otherId) {
    const other = $(otherId);
    const on = $(selectId).value === OTHER;
    other.hidden = !on;
    if (on) other.focus();
  }

  /** Re-reads the saved settings into the dropdowns. */
  function refresh() {
    if (!$("lang_note") || !appState || !appState.settings) return;
    const s = appState.settings;
    fill($("input_language"), SPOKEN_OPTIONS, canonicalSpoken(s.input_language), (v, l) => l);
    fill($("output_language"), OUTPUT_OPTIONS, canonicalOutput(s.output_language), outputLabel);
    $("input_language_other").hidden = true;
    $("output_language_other").hidden = true;
    renderNote();
  }

  function mount(state) {
    appState = state;
    if (mounted) {
      refresh();
      return;
    }
    mounted = true;
    for (const [selectId, otherId] of [
      ["input_language", "input_language_other"],
      ["output_language", "output_language_other"],
    ]) {
      $(selectId).addEventListener("change", (e) => {
        // Typing in the "Other" box re-fires change; only open it on a pick.
        if (e.isTrusted) showOther(selectId, otherId);
        renderNote();
      });
      $(otherId).addEventListener("input", () => {
        renderNote();
        $(selectId).dispatchEvent(new Event("change"));
      });
    }
    const model = $("whisper_model");
    if (model) model.addEventListener("change", renderNote);
    refresh();
  }

  window.VoxLanguages = {
    mount, refresh, values,
    // For tests and the console.
    _canonicalSpoken: canonicalSpoken, _canonicalOutput: canonicalOutput,
  };

  // app.js mounts this once settings load, but if they arrived before this
  // script ran (it loads after app.js), mount now instead of never.
  const app = window.VoxApp;
  if (app && app.state && app.state.settings) mount(app.state);
})();
