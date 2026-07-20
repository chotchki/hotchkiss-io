//From https://stackoverflow.com/a/34278578/160208
// biome-ignore lint/correctness/noUnusedVariables: called from template inline handlers
function addLink() {
  const el = document.getElementById("page_markdown");
  const [start, end] = [el.selectionStart, el.selectionEnd];
  const currentText = el.value.slice(start, end);
  el.setRangeText("[" + currentText + "]()", start, end, "select");
  el.dispatchEvent(new Event("change", { bubbles: true }));
}

// biome-ignore lint/correctness/noUnusedVariables: called from template inline handlers
function addImage() {
  const el = document.getElementById("page_markdown");
  const [start, end] = [el.selectionStart, el.selectionEnd];
  const currentText = el.value.slice(start, end);
  el.setRangeText("![" + currentText + "]()", start, end, "select");
  el.dispatchEvent(new Event("change", { bubbles: true }));
}

// biome-ignore lint/correctness/noUnusedVariables: called from template inline handlers
function addChildIndex() {
  const el = document.getElementById("page_markdown");
  const [start, end] = [el.selectionStart, el.selectionEnd];
  // A ```children fence — the child-index widget lists this page's child pages
  // as a card grid (order=manual for volumes/curated; change to newest for a feed).
  el.setRangeText("\n```children order=manual\n```\n", start, end, "end");
  el.focus();
  el.dispatchEvent(new Event("change", { bubbles: true }));
}

// biome-ignore lint/correctness/noUnusedVariables: called from template inline handlers
function addDiagram() {
  const el = document.getElementById("page_markdown");
  const [start, end] = [el.selectionStart, el.selectionEnd];
  // A ```d2 fence — compiled to an inline SVG diagram at render (a starter graph).
  el.setRangeText("\n```d2\na -> b -> c\n```\n", start, end, "end");
  el.focus();
  el.dispatchEvent(new Event("change", { bubbles: true }));
}

// biome-ignore lint/correctness/noUnusedVariables: called from template inline handlers
function addAttachment(event) {
  event.preventDefault();
  const el = document.getElementById("page_markdown");
  const targetUrl = event.currentTarget.href;

  const [start, end] = [el.selectionStart, el.selectionEnd];
  const currentText = el.value.slice(start, end);
  el.setRangeText(
    "![" + currentText + "](" + targetUrl + ")",
    start,
    end,
    "select",
  );
  el.dispatchEvent(new Event("change", { bubbles: true }));
}

// biome-ignore lint/correctness/noUnusedVariables: called from template inline handlers
function setAsCoverId(attachment_id) {
  document.getElementById("page_cover_attachment_id").value = attachment_id;
}

// --- Inline media upload (Phase BZ): async upload → insert ![](/media/<ref>) at
// the cursor, with NO page refresh, so unsaved markdown survives. (The old
// attachment upload returned htmx_refresh(), which reloaded the page and ate
// your edits.) Drop files onto the textarea, or use the toolbar button. ---
// A never-focused textarea reports caret 0, so a mobile upload (tap the toolbar
// button, never touch the text) silently landed the embed at the TOP of the
// markdown (EB.3). Track real focus; until then, insert at the END.
let editorCaretTouched = false;

function insertMediaRef(ref) {
  const el = document.getElementById("page_markdown");
  if (!el) return;
  let [start, end] = [el.selectionStart, el.selectionEnd];
  let text = "![](/media/" + ref + ")";
  if (!editorCaretTouched) {
    start = el.value.length;
    end = start;
    if (start > 0 && !el.value.endsWith("\n")) text = "\n" + text;
  }
  el.setRangeText(text, start, end, "end");
  el.focus();
  el.dispatchEvent(new Event("change", { bubbles: true }));
}

function uploadMediaFiles(files) {
  const list = Array.from(files || []).filter((f) => f && f.size);
  if (!list.length) return;
  const status = document.getElementById("media-upload-status");
  const bar = document.getElementById("media-upload-bar");
  const fd = new FormData();
  for (const f of list) fd.append("file", f, f.name);
  // A file dropped on a GATED page must not mint public media (DC.5): send the
  // page's current Visibility select as this upload's default gate. Scoped to
  // the EDITOR's own form — a bare name-selector would grab the first
  // min_role select in document order if another ever coexists on the page.
  const pageVis = document
    .getElementById("page_markdown")
    ?.closest("form")
    ?.querySelector('select[name="min_role"]');
  if (pageVis && pageVis.value !== "Public")
    fd.append("min_role", pageVis.value);
  UploadProgress.xhrUpload("/media", fd, (phase, loaded, total) => {
    if (phase === "uploading") {
      const pct = total ? Math.round((loaded / total) * 100) : 0;
      if (bar) {
        bar.classList.remove("hidden");
        bar.value = pct;
      }
      if (status)
        status.textContent =
          "Uploading " +
          pct +
          "% — " +
          UploadProgress.fmtBytes(loaded) +
          " / " +
          UploadProgress.fmtBytes(total);
    } else {
      if (bar) {
        bar.classList.remove("hidden");
        bar.removeAttribute("value");
      }
      if (status) status.textContent = "Processing…";
    }
  })
    .then((text) => {
      // POST /media returns the manifest (DR): the ref is `ref`, not `media_ref`.
      const j = JSON.parse(text);
      insertMediaRef(j.ref);
      if (bar) {
        bar.classList.add("hidden");
        bar.value = 0;
      }
      if (status) status.textContent = "Inserted ![](/media/" + j.ref + ")";
    })
    .catch((e) => {
      if (bar) {
        bar.classList.add("hidden");
        bar.value = 0;
      }
      if (status) status.textContent = "Upload failed: " + e;
    });
}

document.addEventListener("DOMContentLoaded", () => {
  // Library picker + the camera capture input (EB.3) share the upload path.
  for (const id of ["media-upload-input", "media-capture-input"]) {
    const input = document.getElementById(id);
    if (input) {
      input.addEventListener("change", () => {
        uploadMediaFiles(input.files);
        input.value = "";
      });
    }
  }
  // Drag files onto the markdown box → upload + insert at the drop.
  const ta = document.getElementById("page_markdown");
  if (ta) {
    ta.addEventListener("focus", () => {
      editorCaretTouched = true;
    });
    ta.addEventListener("dragover", (e) => {
      if (
        e.dataTransfer &&
        Array.from(e.dataTransfer.types || []).includes("Files")
      ) {
        e.preventDefault();
      }
    });
    ta.addEventListener("drop", (e) => {
      if (
        e.dataTransfer &&
        e.dataTransfer.files &&
        e.dataTransfer.files.length
      ) {
        e.preventDefault();
        uploadMediaFiles(e.dataTransfer.files);
      }
    });
  }
});
