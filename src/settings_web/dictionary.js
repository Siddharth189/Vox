/* global VoxApp */

window.VoxDictionary = {
  mount(root, state) {
    root.innerHTML = `
      <div class="card">
        <h2>Custom dictionary</h2>
        <p class="muted">One term per line. Preferred spellings bias Whisper and the LLM, then deterministic alias normalization runs after.</p>
        <textarea id="dict_terms"></textarea>
      </div>
      <div class="card">
        <h2>Aliases</h2>
        <p class="muted">Format: <span class="mono">CanonicalTerm = alias1, alias2</span></p>
        <textarea id="dict_aliases"></textarea>
      </div>
      <div class="actions">
        <button class="primary" id="save_dict">Save dictionary</button>
        <span class="status" id="dict_status"></span>
      </div>
    `;

    const termsEl = root.querySelector("#dict_terms");
    const aliasesEl = root.querySelector("#dict_aliases");
    const status = root.querySelector("#dict_status");

    function render() {
      termsEl.value = (state.settings.custom_dictionary || []).join("\n");
      const lines = [];
      const aliases = state.settings.custom_aliases || {};
      for (const [term, list] of Object.entries(aliases)) {
        lines.push(`${term} = ${(list || []).join(", ")}`);
      }
      aliasesEl.value = lines.join("\n");
    }

    function parseAliases(text) {
      const out = {};
      for (const line of text.split("\n")) {
        const trimmed = line.trim();
        if (!trimmed || trimmed.startsWith("#")) continue;
        const eq = trimmed.indexOf("=");
        if (eq < 0) continue;
        const term = trimmed.slice(0, eq).trim();
        const vals = trimmed
          .slice(eq + 1)
          .split(",")
          .map((s) => s.trim())
          .filter(Boolean);
        if (term && vals.length) out[term] = vals;
      }
      return out;
    }

    root.querySelector("#save_dict").addEventListener("click", async () => {
      status.textContent = "Saving…";
      try {
        const custom_dictionary = termsEl.value
          .split("\n")
          .map((s) => s.trim())
          .filter(Boolean);
        const custom_aliases = parseAliases(aliasesEl.value);
        const patch = {
          output_language: state.settings.output_language,
          input_language: state.settings.input_language,
          whisper_model: state.settings.whisper_model,
          llm_model: state.settings.llm_model,
          auto_paste: state.settings.auto_paste,
          hotkey: state.settings.hotkey,
          custom_dictionary,
          custom_aliases,
          profiles: state.settings.profiles,
          system_message_override: state.settings.system_message_override ?? null,
        };
        state.settings = await VoxApp.api("/api/settings", {
          method: "POST",
          body: JSON.stringify(patch),
        });
        render();
        status.textContent = "Saved";
      } catch (e) {
        status.textContent = `Error: ${e.message}`;
      }
    });

    render();
  },
};
