/* global VoxApp */

window.VoxHistory = {
  mount(root, state) {
    root.innerHTML = `
      <div class="card">
        <h2>Dictation history</h2>
        <div class="actions" style="margin-top:0">
          <button class="ghost" id="refresh_history">Refresh</button>
          <button class="ghost danger" id="clear_history">Clear history</button>
          <span class="status" id="history_status"></span>
        </div>
        <div id="history_list" style="margin-top:1rem"></div>
      </div>
    `;

    const list = root.querySelector("#history_list");
    const status = root.querySelector("#history_status");

    function escapeHtml(s) {
      return String(s)
        .replace(/&/g, "&amp;")
        .replace(/</g, "&lt;")
        .replace(/>/g, "&gt;");
    }

    async function load() {
      status.textContent = "Loading…";
      try {
        const records = await VoxApp.api("/api/history");
        if (!records.length) {
          list.innerHTML = `<p class="muted">No dictations yet.</p>`;
          status.textContent = "";
          return;
        }
        list.innerHTML = records
          .map((r) => {
            const lat = r.latency || {};
            return `<article class="card" data-id="${escapeHtml(r.id)}" style="padding:0.85rem">
              <div class="muted" style="font-size:0.8rem">${new Date(r.created_at_ms).toLocaleString()} · ${escapeHtml(r.app_name)} · ${escapeHtml(r.injection_strategy || "")}</div>
              <div style="margin:0.4rem 0"><strong>Final</strong><div class="mono">${escapeHtml(r.final_text || "")}</div></div>
              <div class="muted" style="font-size:0.8rem">latency: privacy ${lat.privacy_ms || 0}ms · stt ${lat.transcribe_ms || 0}ms · llm ${lat.process_ms || 0}ms · inject ${lat.inject_ms || 0}ms · total ${lat.total_ms || 0}ms</div>
              <details style="margin-top:0.5rem">
                <summary>Raw / model / correct</summary>
                <label>Raw</label>
                <div class="mono">${escapeHtml(r.raw_text || "")}</div>
                <label>Model</label>
                <div class="mono">${escapeHtml(r.model_text || "")}</div>
                <label>Correct this text</label>
                <textarea data-correct>${escapeHtml(r.corrected_text || r.final_text || "")}</textarea>
                <div class="actions">
                  <button class="primary" data-learn>Learn correction</button>
                  <span class="status" data-learn-status></span>
                </div>
              </details>
            </article>`;
          })
          .join("");

        list.querySelectorAll("[data-learn]").forEach((btn) => {
          btn.addEventListener("click", async () => {
            const article = btn.closest("article");
            const id = article.dataset.id;
            const corrected = article.querySelector("[data-correct]").value;
            const original = records.find((x) => x.id === id)?.final_text || "";
            const st = article.querySelector("[data-learn-status]");
            st.textContent = "Learning…";
            try {
              const res = await VoxApp.api("/api/learn-correction", {
                method: "POST",
                body: JSON.stringify({
                  record_id: id,
                  original_text: original,
                  corrected_text: corrected,
                }),
              });
              const n = (res.learned_aliases || []).length;
              st.textContent = n ? `Learned ${n} alias(es)` : "No new aliases";
              // Refresh settings in shared state for dictionary tab
              state.settings = await VoxApp.api("/api/settings");
            } catch (e) {
              st.textContent = `Error: ${e.message}`;
            }
          });
        });
        status.textContent = `${records.length} records`;
      } catch (e) {
        status.textContent = `Error: ${e.message}`;
      }
    }

    root.querySelector("#refresh_history").addEventListener("click", load);
    root.querySelector("#clear_history").addEventListener("click", async () => {
      if (!confirm("Clear all dictation history?")) return;
      await VoxApp.api("/api/history/clear", { method: "POST", body: "{}" });
      await load();
    });

    this.reload = load;
    load();
  },
};
