/* global VoxApp */

window.VoxDictionary = {
  mount(root, state) {
    const table = root.querySelector("#dict_table");
    const status = root.querySelector("#dict_status");
    const addTermBtn = root.querySelector("#dict_add_term");

    function escapeHtml(s) {
      return String(s)
        .replace(/&/g, "&amp;")
        .replace(/</g, "&lt;")
        .replace(/>/g, "&gt;");
    }

    async function persist() {
      status.textContent = "Saving…";
      try {
        const patch = {
          output_language: state.settings.output_language,
          input_language: state.settings.input_language,
          whisper_model: state.settings.whisper_model,
          llm_model: state.settings.llm_model,
          auto_paste: state.settings.auto_paste,
          hotkey: state.settings.hotkey,
          custom_dictionary: state.settings.custom_dictionary || [],
          custom_aliases: state.settings.custom_aliases || {},
          profiles: state.settings.profiles,
          system_message_override: state.settings.system_message_override ?? null,
        };
        state.settings = await VoxApp.api("/api/settings", {
          method: "POST",
          body: JSON.stringify(patch),
        });
        status.textContent = "Saved";
        render();
      } catch (e) {
        status.textContent = `Error: ${e.message}`;
      }
    }

    function render() {
      const terms = state.settings.custom_dictionary || [];
      const aliases = state.settings.custom_aliases || {};
      if (!terms.length) {
        table.innerHTML = `<p class="muted">No terms yet. Add one below.</p>`;
        return;
      }
      table.innerHTML = terms
        .map((term) => {
          const chips = (aliases[term] || [])
            .map(
              (a) =>
                `<span class="chip">${escapeHtml(a)} <button type="button" data-remove-alias="${escapeHtml(term)}::${escapeHtml(a)}">&times;</button></span>`
            )
            .join("");
          return `<div class="dict-row" data-term="${escapeHtml(term)}">
            <div class="chip">${escapeHtml(term)} <button type="button" data-remove-term="${escapeHtml(term)}">&times;</button></div>
            <div class="dict-arrow">&rarr;</div>
            <div class="dict-aliases">${chips}<button type="button" class="link" data-add-alias="${escapeHtml(term)}">+ alias</button></div>
          </div>`;
        })
        .join("");

      table.querySelectorAll("[data-remove-term]").forEach((btn) => {
        btn.addEventListener("click", () => {
          const term = btn.dataset.removeTerm;
          state.settings.custom_dictionary = terms.filter((t) => t !== term);
          const nextAliases = { ...aliases };
          delete nextAliases[term];
          state.settings.custom_aliases = nextAliases;
          persist();
        });
      });

      table.querySelectorAll("[data-remove-alias]").forEach((btn) => {
        btn.addEventListener("click", () => {
          const [term, alias] = btn.dataset.removeAlias.split("::");
          const next = (aliases[term] || []).filter((a) => a !== alias);
          const nextAliases = { ...aliases };
          if (next.length) {
            nextAliases[term] = next;
          } else {
            delete nextAliases[term];
          }
          state.settings.custom_aliases = nextAliases;
          persist();
        });
      });

      table.querySelectorAll("[data-add-alias]").forEach((btn) => {
        btn.addEventListener("click", () => {
          const term = btn.dataset.addAlias;
          const alias = prompt(`New alias for "${term}" (how it's heard):`);
          if (!alias || !alias.trim()) return;
          const nextAliases = { ...aliases };
          nextAliases[term] = [...(nextAliases[term] || []), alias.trim()];
          state.settings.custom_aliases = nextAliases;
          persist();
        });
      });
    }

    addTermBtn.addEventListener("click", () => {
      const term = prompt("New dictionary term (the correct spelling):");
      if (!term || !term.trim()) return;
      const terms = state.settings.custom_dictionary || [];
      if (terms.includes(term.trim())) return;
      state.settings.custom_dictionary = [...terms, term.trim()];
      persist();
    });

    render();
  },
};
