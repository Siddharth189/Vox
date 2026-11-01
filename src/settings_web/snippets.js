/* global VoxApp */

// Voice shortcuts: say the trigger, get the stored text verbatim.
window.VoxSnippets = {
  mount(root, state) {
    const list = root.querySelector("#snippet_list");
    const status = root.querySelector("#snippet_status");
    const triggerInput = root.querySelector("#new_snippet_trigger");
    const textInput = root.querySelector("#new_snippet_text");
    const addBtn = root.querySelector("#add_snippet");

    function escapeHtml(s) {
      return String(s)
        .replace(/&/g, "&amp;")
        .replace(/</g, "&lt;")
        .replace(/>/g, "&gt;")
        .replace(/"/g, "&quot;");
    }

    async function persist(snippets) {
      status.textContent = "Saving…";
      try {
        const s = state.settings;
        state.settings = await VoxApp.api("/api/settings", {
          method: "POST",
          body: JSON.stringify({
            output_language: s.output_language,
            input_language: s.input_language,
            whisper_model: s.whisper_model,
            llm_model: s.llm_model,
            auto_paste: s.auto_paste,
            custom_dictionary: s.custom_dictionary || [],
            snippets,
          }),
        });
        status.textContent = "Saved";
        render();
      } catch (e) {
        status.textContent = `Error: ${e.message}`;
      }
    }

    function render() {
      const snippets = state.settings.snippets || [];
      if (!snippets.length) {
        list.innerHTML = `<p class="field-desc">No snippets yet. Try a trigger like “my email” or “my calendar link”.</p>`;
        return;
      }
      list.innerHTML = snippets
        .map(
          (s, i) => `<div class="snippet-row">
            <div class="chip">“${escapeHtml(s.trigger)}”</div>
            <div class="dict-arrow">&rarr;</div>
            <div class="snippet-text">${escapeHtml(s.text)}</div>
            <button type="button" class="link" data-remove-snippet="${i}">Remove</button>
          </div>`
        )
        .join("");
    }

    list.addEventListener("click", (e) => {
      const idx = e.target.getAttribute("data-remove-snippet");
      if (idx === null) return;
      const next = [...(state.settings.snippets || [])];
      next.splice(Number(idx), 1);
      persist(next);
    });

    addBtn.addEventListener("click", () => {
      const trigger = triggerInput.value.trim();
      const text = textInput.value.trim();
      if (!trigger || !text) {
        status.textContent = "Enter both a trigger phrase and the text to insert.";
        return;
      }
      triggerInput.value = "";
      textInput.value = "";
      persist([...(state.settings.snippets || []), { trigger, text }]);
    });

    render();
  },
};
