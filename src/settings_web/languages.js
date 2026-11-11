/* Language settings: "Languages you speak" and "Write it as".
 *
 * Plain controls on top of the two settings the backend already parses, kept
 * in the hidden #input_language / #output_language fields, so saving and the
 * prompt preview work unchanged:
 *   input_language:  "auto" | "Hindi" | "English, Hindi" | "Hinglish / mixed Hindi-English"
 *   output_language: "auto" | "English" | "Hindi" | "Hinglish / mixed Hindi-English" | any name
 *
 * Every choice says whether it works on this machine right now: translating
 * needs the local AI (LLM), and a choice that can't work is never silently
 * accepted.
 */
(function () {
  const SPOKEN = [
    "English", "Hindi", "Tamil", "Telugu", "Marathi", "Bengali", "Spanish", "French",
    "German", "Portuguese", "Italian", "Russian", "Japanese", "Korean", "Chinese", "Arabic",
  ];
  const CODES = {
    en: "English", hi: "Hindi", ta: "Tamil", te: "Telugu", mr: "Marathi", bn: "Bengali",
    es: "Spanish", fr: "French", de: "German", pt: "Portuguese", it: "Italian", ru: "Russian",
    ja: "Japanese", ko: "Korean", zh: "Chinese", ar: "Arabic",
  };
  const HINGLISH = "Hinglish / mixed Hindi-English";
  const OUTPUTS = [
    { value: "auto", label: "As I said it", desc: "In the language you spoke.", llm: false },
    { value: "English", label: "English", desc: "Translated into English.", llm: true },
    { value: "Hindi", label: "Hindi (देवनागरी)", desc: "Translated into Hindi script.", llm: true },
    { value: HINGLISH, label: "Hinglish (Latin letters)", desc: "Hindi written in English letters.", llm: true },
    { value: "other", label: "Another language", desc: "Spanish, French, Japanese…", llm: true },
  ];

  const ui = { spoken: { langs: [], mixed: false, other: "" }, output: "auto", outputOther: "" };
  let appState = null;

  const $ = (id) => document.getElementById(id);
  const findName = (p) =>
    SPOKEN.find((n) => n.toLowerCase() === p.toLowerCase()) || CODES[p.toLowerCase()];

  function parseSpoken(value) {
    const v = (value || "").trim();
    const lower = v.toLowerCase();
    if (!v || lower === "auto") return { langs: [], mixed: false, other: "" };
    if (/hinglish|mixed|code-switch/.test(lower)) {
      return { langs: ["English", "Hindi"], mixed: true, other: "" };
    }
    const langs = [];
    const other = [];
    for (const raw of v.split(/[,+/&;]| and /i)) {
      const p = raw.trim();
      if (!p) continue;
      const name = findName(p);
      if (name) {
        if (!langs.includes(name)) langs.push(name);
      } else if (!other.includes(p)) {
        other.push(p);
      }
    }
    return { langs, mixed: false, other: other.join(", ") };
  }

  function otherList(text) {
    return (text || "").split(",").map((s) => s.trim()).filter(Boolean);
  }

  /** The input_language string for the current choices. */
  function spokenValue(s) {
    const all = [...s.langs, ...otherList(s.other)];
    if (canMix(s) && s.mixed) return HINGLISH;
    return all.length ? all.join(", ") : "auto";
  }

  /** Mixing in one sentence is supported for exactly Hindi + English. */
  function canMix(s) {
    return s.langs.length === 2 && s.langs.includes("English") && s.langs.includes("Hindi") &&
      otherList(s.other).length === 0;
  }

  function parseOutput(value) {
    const v = (value || "").trim();
    if (!v || v.toLowerCase() === "auto") return { output: "auto", other: "" };
    if (/hinglish|mixed|code-switch/i.test(v)) return { output: HINGLISH, other: "" };
    const known = OUTPUTS.find((o) => o.value.toLowerCase() === v.toLowerCase());
    if (known) return { output: known.value, other: "" };
    return { output: "other", other: v };
  }

  function outputValue() {
    if (ui.output === "other") return ui.outputOther.trim() || "auto";
    return ui.output;
  }

  function llmReady() {
    return !!(appState && appState.models && appState.models.llm_ready);
  }

  function englishOnlyModel() {
    const model = ($("whisper_model") && $("whisper_model").value) || "";
    return /\.en[-.]/i.test(model.split(/[\\/]/).pop() || "");
  }

  function writeHidden() {
    const pairs = [["input_language", spokenValue(ui.spoken)], ["output_language", outputValue()]];
    for (const [id, value] of pairs) {
      const el = $(id);
      if (el.value !== value) {
        el.value = value;
        el.dispatchEvent(new Event("change"));
      }
    }
  }

  function listJoin(items) {
    if (items.length <= 1) return items.join("");
    return `${items.slice(0, -1).join(", ")} and ${items[items.length - 1]}`;
  }

  function renderSpoken() {
    const box = $("spoken_chips");
    box.innerHTML = "";
    for (const name of SPOKEN) {
      const on = ui.spoken.langs.includes(name);
      const b = document.createElement("button");
      b.type = "button";
      b.className = "lang-chip" + (on ? " on" : "");
      b.setAttribute("aria-pressed", on ? "true" : "false");
      b.textContent = name;
      b.addEventListener("click", () => {
        const langs = ui.spoken.langs;
        ui.spoken.langs = on ? langs.filter((l) => l !== name) : [...langs, name];
        update();
      });
      box.appendChild(b);
    }
    $("spoken_other").value = ui.spoken.other;
    const mixable = canMix(ui.spoken);
    $("mixed_row").hidden = !mixable;
    $("spoken_mixed").checked = mixable && ui.spoken.mixed;

    const note = [];
    if (englishOnlyModel() && spokenValue(ui.spoken) !== "auto" &&
        [...ui.spoken.langs, ...otherList(ui.spoken.other)].some((l) => l !== "English")) {
      note.push("Your speech model only understands English. Pick one without “.en” under Models.");
    }
    $("spoken_note").textContent = note.join(" ");
  }

  function renderOutput() {
    const box = $("output_choices");
    box.innerHTML = "";
    const ready = llmReady();
    for (const o of OUTPUTS) {
      const on = ui.output === o.value;
      const label = document.createElement("label");
      label.className = "lang-choice" + (on ? " on" : "");
      const input = document.createElement("input");
      input.type = "radio";
      input.name = "write_as";
      input.value = o.value;
      input.checked = on;
      input.addEventListener("change", () => {
        ui.output = o.value;
        update();
        if (o.value === "other") $("output_other").focus();
      });
      const text = document.createElement("span");
      text.className = "lang-choice-text";
      text.innerHTML = `<span class="lang-choice-label"></span><span class="lang-choice-desc"></span>`;
      text.querySelector(".lang-choice-label").textContent = o.label;
      text.querySelector(".lang-choice-desc").textContent = o.desc;
      label.append(input, text);
      if (o.llm) {
        const badge = document.createElement("span");
        badge.className = "lang-badge" + (ready ? " ready" : " needs");
        badge.textContent = ready ? "Uses local AI" : "Needs local AI";
        label.appendChild(badge);
      }
      box.appendChild(label);
    }
    const other = $("output_other");
    other.hidden = ui.output !== "other";
    other.value = ui.outputOther;
  }

  function renderSummary() {
    const s = ui.spoken;
    const spoken = [...s.langs, ...otherList(s.other)];
    const out = outputValue();
    const choice = OUTPUTS.find((o) => o.value === ui.output);
    let said = spoken.length
      ? `You speak ${listJoin(spoken)}${canMix(s) && s.mixed ? ", mixed in the same sentence" : ""}.`
      : "Vox detects the language you speak.";
    let result;
    if (out === "auto") {
      result = " Text comes out in the language you spoke.";
    } else {
      const target = ui.output === "other" ? out : choice.label;
      result = ` Text comes out in ${target}.`;
    }
    const summary = $("lang_summary");
    summary.className = "lang-summary";
    if (out !== "auto" && !llmReady()) {
      summary.classList.add("warn");
      result += " Local AI isn't running, so until it is, Vox inserts what you said untranslated." +
        " Set it up under Models below.";
    }
    summary.textContent = said + result;
  }

  function update() {
    writeHidden();
    renderSpoken();
    renderOutput();
    renderSummary();
  }

  /** Re-reads the hidden fields (after settings load or reset). */
  function refresh() {
    if (!$("spoken_chips")) return;
    ui.spoken = parseSpoken($("input_language").value);
    const o = parseOutput($("output_language").value);
    ui.output = o.output;
    ui.outputOther = o.other;
    renderSpoken();
    renderOutput();
    renderSummary();
  }

  let mounted = false;

  function mount(state) {
    appState = state;
    if (mounted) {
      refresh();
      return;
    }
    mounted = true;
    $("spoken_other").addEventListener("input", (e) => {
      ui.spoken.other = e.target.value;
      writeHidden();
      $("mixed_row").hidden = !canMix(ui.spoken);
      renderSummary();
    });
    $("spoken_mixed").addEventListener("change", (e) => {
      ui.spoken.mixed = e.target.checked;
      update();
    });
    $("output_other").addEventListener("input", (e) => {
      ui.outputOther = e.target.value;
      writeHidden();
      renderSummary();
    });
    const model = $("whisper_model");
    if (model) model.addEventListener("change", () => renderSpoken());
    refresh();
  }

  window.VoxLanguages = {
    mount, refresh,
    // For tests and the console.
    _parseSpoken: parseSpoken, _spokenValue: spokenValue, _parseOutput: parseOutput,
  };

  // app.js mounts this once settings load, but if they arrived before this
  // script ran (it loads after app.js), mount now instead of never.
  const app = window.VoxApp;
  if (app && app.state && app.state.settings) mount(app.state);
})();
