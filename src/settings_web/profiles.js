/* global VoxApp */

window.VoxProfiles = {
  mount(root, state) {
    const table = root.querySelector("#profiles_table");
    const status = root.querySelector("#profiles_status");
    const newBundleInput = root.querySelector("#new_profile_bundle");
    const addBtn = root.querySelector("#add_profile");

    const formats = [
      "clean_prose",
      "casual",
      "professional_email",
      "code_context",
      "shell",
      "markdown",
    ];
    const privacies = ["local_only", "disabled"];
    const formatLabels = {
      clean_prose: "Clean prose",
      casual: "Casual",
      professional_email: "Professional email",
      code_context: "Code context",
      shell: "Shell",
      markdown: "Markdown",
    };
    const privacyLabels = { local_only: "Local only", disabled: "Disabled" };

    function escapeHtml(s) {
      return String(s).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
    }

    async function persist(profiles) {
      status.textContent = "Saving…";
      try {
        const patch = {
          output_language: state.settings.output_language,
          input_language: state.settings.input_language,
          whisper_model: state.settings.whisper_model,
          llm_model: state.settings.llm_model,
          auto_paste: state.settings.auto_paste,
          hotkey: state.settings.hotkey,
          custom_dictionary: state.settings.custom_dictionary,
          custom_aliases: state.settings.custom_aliases,
          profiles,
          system_message_override: state.settings.system_message_override ?? null,
        };
        state.settings = await VoxApp.api("/api/settings", {
          method: "POST",
          body: JSON.stringify(patch),
        });
        status.textContent = "";
        render();
      } catch (e) {
        status.textContent = `Error: ${e.message}`;
      }
    }

    function render() {
      const profiles = state.settings.profiles || {};
      const bundleIds = Object.keys(profiles).sort((a, b) => (a === "default" ? -1 : b === "default" ? 1 : a.localeCompare(b)));

      table.innerHTML = bundleIds
        .map((bundleId) => {
          const p = profiles[bundleId];
          return `<div class="field-row" data-bundle="${escapeHtml(bundleId)}">
            <div class="field-info"><div class="field-title mono" style="font-weight:500">${escapeHtml(bundleId)}</div></div>
            <div class="field-control" style="display:flex;gap:0.5rem;align-items:center;justify-content:flex-end">
              <select data-field="format">${formats
                .map((f) => `<option value="${f}" ${f === p.format ? "selected" : ""}>${formatLabels[f]}</option>`)
                .join("")}</select>
              <select data-field="privacy">${privacies
                .map((pr) => `<option value="${pr}" ${pr === p.privacy ? "selected" : ""}>${privacyLabels[pr]}</option>`)
                .join("")}</select>
              ${bundleId === "default" ? "" : `<button class="ghost danger" data-remove="${escapeHtml(bundleId)}" style="flex:0 0 auto">Remove</button>`}
            </div>
          </div>`;
        })
        .join("");

      table.querySelectorAll("select").forEach((el) => {
        el.addEventListener("change", () => {
          const row = el.closest("[data-bundle]");
          const bundleId = row.dataset.bundle;
          const next = { ...state.settings.profiles };
          next[bundleId] = { ...next[bundleId], [el.dataset.field]: el.value };
          persist(next);
        });
      });

      table.querySelectorAll("[data-remove]").forEach((btn) => {
        btn.addEventListener("click", () => {
          const next = { ...state.settings.profiles };
          delete next[btn.dataset.remove];
          persist(next);
        });
      });
    }

    addBtn.addEventListener("click", () => {
      const bundleId = newBundleInput.value.trim();
      if (!bundleId) return;
      const next = { ...state.settings.profiles };
      if (next[bundleId]) {
        status.textContent = "That profile already exists";
        return;
      }
      next[bundleId] = { format: "clean_prose", privacy: "local_only" };
      newBundleInput.value = "";
      persist(next);
    });

    render();
  },
};
