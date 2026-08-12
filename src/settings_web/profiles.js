/* global VoxApp */

window.VoxProfiles = {
  mount(root, state) {
    root.innerHTML = `
      <div class="card">
        <h2>Per-app profiles</h2>
        <p class="muted">The <span class="mono">default</span> profile is required. Privacy <span class="mono">disabled</span> blocks transcription and injection for that app.</p>
        <div id="profiles_table"></div>
        <div class="actions">
          <button class="ghost" id="add_profile">Add profile</button>
          <button class="primary" id="save_profiles">Save profiles</button>
          <span class="status" id="profiles_status"></span>
        </div>
      </div>
    `;

    const tableHost = root.querySelector("#profiles_table");
    const status = root.querySelector("#profiles_status");
    let rows = [];

    function syncFromState() {
      rows = Object.entries(state.settings.profiles || {}).map(([bundle_id, p]) => ({
        bundle_id,
        format: p.format || "clean_prose",
        privacy: p.privacy || "local_only",
      }));
      if (!rows.some((r) => r.bundle_id === "default")) {
        rows.unshift({ bundle_id: "default", format: "clean_prose", privacy: "local_only" });
      }
    }

    function render() {
      const formats = [
        "clean_prose",
        "casual",
        "professional_email",
        "code_context",
        "shell",
        "markdown",
      ];
      const privacies = ["local_only", "disabled"];
      let html = `<table><thead><tr><th>Bundle ID</th><th>Format</th><th>Privacy</th><th></th></tr></thead><tbody>`;
      rows.forEach((row, i) => {
        html += `<tr data-i="${i}">
          <td><input type="text" data-field="bundle_id" value="${escapeAttr(row.bundle_id)}" ${row.bundle_id === "default" ? "readonly" : ""}/></td>
          <td><select data-field="format">${formats
            .map((f) => `<option value="${f}" ${f === row.format ? "selected" : ""}>${f}</option>`)
            .join("")}</select></td>
          <td><select data-field="privacy">${privacies
            .map((p) => `<option value="${p}" ${p === row.privacy ? "selected" : ""}>${p}</option>`)
            .join("")}</select></td>
          <td>${row.bundle_id === "default" ? "" : `<button class="ghost danger" data-remove="${i}">Remove</button>`}</td>
        </tr>`;
      });
      html += `</tbody></table>`;
      tableHost.innerHTML = html;

      tableHost.querySelectorAll("input, select").forEach((el) => {
        el.addEventListener("change", () => {
          const tr = el.closest("tr");
          const i = Number(tr.dataset.i);
          rows[i][el.dataset.field] = el.value.trim();
        });
      });
      tableHost.querySelectorAll("[data-remove]").forEach((btn) => {
        btn.addEventListener("click", () => {
          rows.splice(Number(btn.dataset.remove), 1);
          render();
        });
      });
    }

    function escapeAttr(s) {
      return String(s).replace(/&/g, "&amp;").replace(/"/g, "&quot;").replace(/</g, "&lt;");
    }

    root.querySelector("#add_profile").addEventListener("click", () => {
      rows.push({ bundle_id: "com.example.app", format: "clean_prose", privacy: "local_only" });
      render();
    });

    root.querySelector("#save_profiles").addEventListener("click", async () => {
      status.textContent = "Saving…";
      try {
        const profiles = {};
        for (const row of rows) {
          if (!row.bundle_id) continue;
          profiles[row.bundle_id] = { format: row.format, privacy: row.privacy };
        }
        if (!profiles.default) throw new Error("default profile is required");
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
        syncFromState();
        render();
        status.textContent = "Saved";
      } catch (e) {
        status.textContent = `Error: ${e.message}`;
      }
    });

    syncFromState();
    render();
  },
};
