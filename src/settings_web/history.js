/* global VoxApp */

window.VoxHistory = {
  mount(root, state) {
    root.innerHTML = `
      <div class="actions" style="margin-top:0">
        <button class="ghost" id="refresh_history">Refresh</button>
        <button class="ghost danger" id="clear_history">Clear history</button>
        <span class="status" id="history_status"></span>
      </div>
      <div class="history-layout">
        <div class="history-list-pane">
          <input id="history_search" type="text" placeholder="Search dictations" />
          <div class="history-stats" id="history_stats"></div>
          <div class="history-bottleneck" id="history_bottleneck"></div>
          <div id="history_list" class="history-list"></div>
        </div>
        <div class="history-detail-pane" id="history_detail">
          <p class="muted">No dictations yet.</p>
        </div>
      </div>
    `;

    const status = root.querySelector("#history_status");
    const searchEl = root.querySelector("#history_search");
    const statsEl = root.querySelector("#history_stats");
    const bottleneckEl = root.querySelector("#history_bottleneck");
    const listEl = root.querySelector("#history_list");
    const detailEl = root.querySelector("#history_detail");

    let records = [];
    let filtered = [];
    let selectedId = null;

    function escapeHtml(s) {
      return String(s)
        .replace(/&/g, "&amp;")
        .replace(/</g, "&lt;")
        .replace(/>/g, "&gt;");
    }

    function fmtSeconds(ms) {
      return `${(ms / 1000).toFixed(2)}s`;
    }

    function fmtDate(ms) {
      const d = new Date(ms);
      const day = d.toLocaleDateString(undefined, { day: "numeric", month: "short" });
      const time = d.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
      return `${day}, ${time}`;
    }

    function percentile(values, p) {
      if (!values.length) return 0;
      const sorted = [...values].sort((a, b) => a - b);
      const idx = Math.min(sorted.length - 1, Math.ceil((p / 100) * sorted.length) - 1);
      return sorted[Math.max(0, idx)];
    }

    function avg(values) {
      if (!values.length) return 0;
      return values.reduce((a, b) => a + b, 0) / values.length;
    }

    function renderStats() {
      if (!filtered.length) {
        statsEl.innerHTML = "";
        bottleneckEl.textContent = "";
        return;
      }
      const totals = filtered.map((r) => r.latency?.total_ms || 0);
      const stt = filtered.map((r) => r.latency?.transcribe_ms || 0);
      const llm = filtered.map((r) => r.latency?.process_ms || 0);
      const paste = filtered.map((r) => r.latency?.inject_ms || 0);
      const avgTotal = avg(totals);
      const p95Total = percentile(totals, 95);
      const avgStt = avg(stt);
      const avgLlm = avg(llm);
      const avgPaste = avg(paste);

      statsEl.innerHTML = `
        <div class="stat"><div class="stat-label">Avg total</div><div class="stat-value">${fmtSeconds(avgTotal)}</div></div>
        <div class="stat"><div class="stat-label">P95 total</div><div class="stat-value">${fmtSeconds(p95Total)}</div></div>
        <div class="stat"><div class="stat-label">STT</div><div class="stat-value">${fmtSeconds(avgStt)}</div></div>
        <div class="stat"><div class="stat-label">LLM</div><div class="stat-value">${fmtSeconds(avgLlm)}</div></div>
        <div class="stat"><div class="stat-label">Paste</div><div class="stat-value">${fmtSeconds(avgPaste)}</div></div>
      `;

      const stages = [
        ["Whisper transcription", avgStt],
        ["LLM cleanup", avgLlm],
        ["Paste injection", avgPaste],
      ];
      stages.sort((a, b) => b[1] - a[1]);
      bottleneckEl.textContent = `Bottleneck: ${stages[0][0]}`;
    }

    function renderList() {
      if (!filtered.length) {
        listEl.innerHTML = `<p class="muted">No dictations yet.</p>`;
        return;
      }
      listEl.innerHTML = filtered
        .map((r) => {
          const preview = (r.final_text || r.raw_text || "").slice(0, 60);
          const active = r.id === selectedId ? " active" : "";
          return `<div class="history-item${active}" data-id="${escapeHtml(r.id)}">
            <div class="history-item-meta">${escapeHtml(r.app_name || "")} &middot; ${fmtDate(r.created_at_ms)}</div>
            <div class="history-item-preview">${escapeHtml(preview)}${(r.final_text || "").length > 60 ? "…" : ""}</div>
          </div>`;
        })
        .join("");

      listEl.querySelectorAll(".history-item").forEach((el) => {
        el.addEventListener("click", () => {
          selectedId = el.dataset.id;
          renderList();
          renderDetail();
        });
      });
    }

    function renderDetail() {
      const record = filtered.find((r) => r.id === selectedId) || filtered[0];
      if (!record) {
        detailEl.innerHTML = `<p class="muted">No dictations yet.</p>`;
        return;
      }
      selectedId = record.id;
      const lat = record.latency || {};
      detailEl.innerHTML = `
        <div class="muted" style="font-size:0.85rem">${escapeHtml(record.app_name || "")} &middot; ${fmtDate(record.created_at_ms)} &middot; ${escapeHtml(record.format || "")} &middot; ${escapeHtml(record.privacy || "")}</div>
        <div style="margin:0.5rem 0 1rem;font-size:1rem">${escapeHtml(record.final_text || "")}</div>

        <div class="field-title" style="margin-bottom:0.5rem">Latency</div>
        <div class="latency-row">
          <div class="stat"><div class="stat-label">Total</div><div class="stat-value">${fmtSeconds(lat.total_ms || 0)}</div></div>
          <div class="stat"><div class="stat-label">Privacy</div><div class="stat-value">${fmtSeconds(lat.privacy_ms || 0)}</div></div>
          <div class="stat"><div class="stat-label">Whisper</div><div class="stat-value">${fmtSeconds(lat.transcribe_ms || 0)}</div></div>
          <div class="stat"><div class="stat-label">LLM</div><div class="stat-value">${fmtSeconds(lat.process_ms || 0)}</div></div>
          <div class="stat"><div class="stat-label">Paste</div><div class="stat-value">${fmtSeconds(lat.inject_ms || 0)}</div></div>
        </div>

        <div class="stage-row">
          <div class="stage-card">
            <div class="field-title" style="font-size:0.85rem">01 &middot; Raw Whisper</div>
            <div class="mono stage-text">${escapeHtml(record.raw_text || "")}</div>
          </div>
          <div class="stage-card">
            <div class="field-title" style="font-size:0.85rem">02 &middot; LLM cleanup</div>
            <div class="mono stage-text">${escapeHtml(record.model_text || "")}</div>
          </div>
          <div class="stage-card">
            <div class="field-title" style="font-size:0.85rem">03 &middot; Dictionary final</div>
            <div class="mono stage-text">${escapeHtml(record.final_text || "")}</div>
          </div>
        </div>

        <div class="field-title" style="margin-top:1rem">Teach Vox</div>
        <div class="field-desc" style="margin-bottom:0.5rem">Correct the final text to learn aliases.</div>
        <textarea id="history_correct">${escapeHtml(record.corrected_text || record.final_text || "")}</textarea>
        <div class="actions">
          <button class="primary" id="history_learn">Learn correction</button>
          <span class="status" id="history_learn_status"></span>
        </div>
      `;

      detailEl.querySelector("#history_learn").addEventListener("click", async () => {
        const st = detailEl.querySelector("#history_learn_status");
        const corrected = detailEl.querySelector("#history_correct").value;
        st.textContent = "Learning…";
        try {
          const res = await VoxApp.api("/api/learn-correction", {
            method: "POST",
            body: JSON.stringify({
              record_id: record.id,
              original_text: record.final_text || "",
              corrected_text: corrected,
            }),
          });
          const n = (res.learned_aliases || []).length;
          st.textContent = n ? `Learned ${n} alias(es)` : "No new aliases";
          state.settings = await VoxApp.api("/api/settings");
        } catch (e) {
          st.textContent = `Error: ${e.message}`;
        }
      });
    }

    function applyFilter() {
      const q = searchEl.value.trim().toLowerCase();
      filtered = q
        ? records.filter((r) => (r.final_text || "").toLowerCase().includes(q) || (r.raw_text || "").toLowerCase().includes(q))
        : records;
      if (!filtered.some((r) => r.id === selectedId)) {
        selectedId = filtered[0]?.id ?? null;
      }
      renderStats();
      renderList();
      renderDetail();
    }

    async function load() {
      status.textContent = "Loading…";
      try {
        records = await VoxApp.api("/api/history");
        applyFilter();
        status.textContent = `${records.length} records`;
      } catch (e) {
        status.textContent = `Error: ${e.message}`;
      }
    }

    searchEl.addEventListener("input", applyFilter);
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
