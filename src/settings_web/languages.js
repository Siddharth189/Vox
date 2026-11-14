/* Language settings: two plain dropdowns, "Spoken language" and "Output
 * language", over the settings strings the backend parses ("auto", "Hindi",
 * "English, Hindi", "Hinglish / mixed Hindi-English", ...).
 *
 * The lists are the common languages. A saved value that isn't in them (a
 * Hinglish or multi-language setting from an earlier version, or one typed
 * into settings.yaml) is shown as its own entry, so saving the page never
 * silently changes it.
 */
(function () {
  const LANGUAGES = [
    "English", "Hindi", "Tamil", "Spanish", "Japanese", "French", "German", "Portuguese",
    "Chinese", "Arabic", "Korean", "Italian", "Russian",
  ];
  const SPOKEN_OPTIONS = [["auto", "Auto-detect"], ...LANGUAGES.map((n) => [n, n])];
  const OUTPUT_OPTIONS = [["auto", "Same as spoken"], ...LANGUAGES.map((n) => [n, n])];
  const CODES = {
    en: "English", hi: "Hindi", ta: "Tamil", es: "Spanish", ja: "Japanese", fr: "French",
    de: "German", pt: "Portuguese", zh: "Chinese", ar: "Arabic", ko: "Korean", it: "Italian",
    ru: "Russian",
  };
  /** Friendlier names for saved values that aren't in the lists. */
  const SAVED_LABELS = {
    "hinglish / mixed hindi-english": "Hinglish",
    "english, hindi": "English + Hindi",
  };

  let appState = null;
  let mounted = false;
  const $ = (id) => document.getElementById(id);

  /** Matches a saved value to a list entry ("english", "en" -> "English");
   * anything else is kept exactly as saved. */
  function canonical(value) {
    const v = (value || "").trim();
    if (!v || v.toLowerCase() === "auto") return "auto";
    return LANGUAGES.find((n) => n.toLowerCase() === v.toLowerCase()) ||
      CODES[v.toLowerCase()] || v;
  }

  function fill(select, options, saved) {
    const value = canonical(saved);
    select.innerHTML = "";
    const add = (v, label) => {
      const opt = document.createElement("option");
      opt.value = v;
      opt.textContent = label;
      select.appendChild(opt);
    };
    if (!options.some(([v]) => v === value)) add(value, SAVED_LABELS[value.toLowerCase()] || value);
    for (const [v, label] of options) add(v, label);
    select.value = value;
  }

  /** The setting strings the dropdowns currently hold. */
  function values() {
    return { input: $("input_language").value, output: $("output_language").value };
  }

  /** Re-reads the saved settings into the dropdowns. */
  function refresh() {
    if (!$("input_language") || !appState || !appState.settings) return;
    fill($("input_language"), SPOKEN_OPTIONS, appState.settings.input_language);
    fill($("output_language"), OUTPUT_OPTIONS, appState.settings.output_language);
  }

  function mount(state) {
    appState = state;
    mounted = true;
    refresh();
  }

  window.VoxLanguages = { mount, refresh, values, _canonical: canonical };

  // app.js mounts this once settings load, but if they arrived before this
  // script ran (it loads after app.js), mount now instead of never.
  const app = window.VoxApp;
  if (!mounted && app && app.state && app.state.settings) mount(app.state);
})();
