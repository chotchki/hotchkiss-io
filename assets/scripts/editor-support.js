//From https://stackoverflow.com/a/34278578/160208
function addLink() {
  const el = document.getElementById("page_markdown");
  const [start, end] = [el.selectionStart, el.selectionEnd];
  const currentText = el.value.slice(start, end);
  el.setRangeText("[" + currentText + "]()", start, end, "select");
  el.dispatchEvent(new Event("change", { bubbles: true }));
}

function addImage() {
  const el = document.getElementById("page_markdown");
  const [start, end] = [el.selectionStart, el.selectionEnd];
  const currentText = el.value.slice(start, end);
  el.setRangeText("![" + currentText + "]()", start, end, "select");
  el.dispatchEvent(new Event("change", { bubbles: true }));
}

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

function setAsCoverId(attachment_id) {
  document.getElementById("page_cover_attachment_id").value = attachment_id;
}

// --- Inline media upload (Phase BZ): async upload → insert ![](/media/<ref>) at
// the cursor, with NO page refresh, so unsaved markdown survives. (The old
// attachment upload returned htmx_refresh(), which reloaded the page and ate
// your edits.) Drop files onto the textarea, or use the toolbar button. ---
function insertMediaRef(ref) {
  const el = document.getElementById("page_markdown");
  if (!el) return;
  const [start, end] = [el.selectionStart, el.selectionEnd];
  el.setRangeText("![](/media/" + ref + ")", start, end, "end");
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
  UploadProgress.xhrUpload(
    "/admin/media/upload",
    fd,
    (phase, loaded, total) => {
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
    },
  )
    .then((text) => {
      const j = JSON.parse(text);
      insertMediaRef(j.media_ref);
      if (bar) {
        bar.classList.add("hidden");
        bar.value = 0;
      }
      if (status)
        status.textContent = "Inserted ![](/media/" + j.media_ref + ")";
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
  const input = document.getElementById("media-upload-input");
  if (input) {
    input.addEventListener("change", () => {
      uploadMediaFiles(input.files);
      input.value = "";
    });
  }
  // Drag files onto the markdown box → upload + insert at the drop.
  const ta = document.getElementById("page_markdown");
  if (ta) {
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
