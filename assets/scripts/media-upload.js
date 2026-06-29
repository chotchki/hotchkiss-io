// Media library upload (Phase BZ). Drag-drop or click-to-select → POST the files
// to /admin/media/upload (multipart) → on success reload the library. Also wires
// the "Copy ![]()" buttons. Zero deps; the page is admin-gated server-side.
(function () {
  const drop = document.getElementById("media-drop");
  const input = document.getElementById("media-file-input");
  const status = document.getElementById("media-upload-status");

  function setStatus(msg) {
    if (status) status.textContent = msg;
  }

  // Drive the <progress> bar + status text from xhrUpload's callback.
  function showProgress(phase, loaded, total) {
    const bar = document.getElementById("media-upload-bar");
    if (phase === "uploading") {
      const pct = total ? Math.round((loaded / total) * 100) : 0;
      if (bar) {
        bar.classList.remove("hidden");
        bar.value = pct;
      }
      setStatus(
        "Uploading " +
          pct +
          "% — " +
          UploadProgress.fmtBytes(loaded) +
          " / " +
          UploadProgress.fmtBytes(total),
      );
    } else {
      if (bar) {
        bar.classList.remove("hidden");
        bar.removeAttribute("value"); // indeterminate while the server ingests
      }
      setStatus("Processing…");
    }
  }

  function hideBar() {
    const bar = document.getElementById("media-upload-bar");
    if (bar) {
      bar.classList.add("hidden");
      bar.value = 0;
    }
  }

  function upload(fileList) {
    const files = Array.from(fileList || []);
    if (!files.length) return;
    const fd = new FormData();
    for (const f of files) fd.append("file", f, f.name);
    if (drop) drop.classList.add("opacity-50", "pointer-events-none");
    UploadProgress.xhrUpload("/admin/media/upload", fd, showProgress)
      .then(() => location.reload())
      .catch((e) => {
        setStatus("Upload failed: " + e);
        hideBar();
        if (drop) drop.classList.remove("opacity-50", "pointer-events-none");
      });
  }

  if (drop) {
    drop.addEventListener("dragover", (e) => {
      e.preventDefault();
      drop.classList.add("bg-navy/10");
    });
    drop.addEventListener("dragleave", () =>
      drop.classList.remove("bg-navy/10"),
    );
    drop.addEventListener("drop", (e) => {
      e.preventDefault();
      drop.classList.remove("bg-navy/10");
      upload(e.dataTransfer.files);
    });
  }
  if (input) input.addEventListener("change", () => upload(input.files));

  document.querySelectorAll(".copy-ref").forEach((btn) => {
    btn.addEventListener("click", () => {
      const md = "![](/media/" + btn.dataset.ref + ")";
      const original = btn.innerHTML;
      navigator.clipboard.writeText(md).then(
        () => {
          btn.textContent = "Copied!";
          setTimeout(() => {
            btn.innerHTML = original;
          }, 1200);
        },
        () => {
          btn.textContent = "Copy failed";
        },
      );
    });
  });

  // "Copy link": the ABSOLUTE /media/file/<url_key> URL — a direct, unguessable
  // (HMAC-keyed) link to the bytes, for private sharing / download.
  document.querySelectorAll(".copy-link").forEach((btn) => {
    btn.addEventListener("click", () => {
      const url = location.origin + "/media/file/" + btn.dataset.urlKey;
      const original = btn.innerHTML;
      navigator.clipboard.writeText(url).then(
        () => {
          btn.textContent = "Copied!";
          setTimeout(() => {
            btn.innerHTML = original;
          }, 1200);
        },
        () => {
          btn.textContent = "Copy failed";
        },
      );
    });
  });

  // "+ add encode": append another variant (another codec, or an image → poster)
  // to an existing item. Fixes needing all encodes in one simultaneous drop.
  document.querySelectorAll(".add-encode-input").forEach((inp) => {
    inp.addEventListener("change", () => {
      const id = inp.dataset.mediaId;
      const files = Array.from(inp.files || []);
      if (!files.length) return;
      const fd = new FormData();
      for (const f of files) fd.append("file", f, f.name);
      UploadProgress.xhrUpload(
        "/admin/media/" + id + "/encode",
        fd,
        showProgress,
      )
        .then(() => location.reload())
        .catch((e) => {
          setStatus("Add failed: " + e);
          hideBar();
        });
    });
  });

  // Rename the display title (the ref stays fixed so embeds don't break).
  document.querySelectorAll(".rename-media").forEach((btn) => {
    btn.addEventListener("click", () => {
      const id = btn.dataset.mediaId;
      const next = window.prompt(
        "Rename media (display title):",
        btn.dataset.title || "",
      );
      if (next === null) return;
      fetch("/admin/media/" + id + "/rename", {
        method: "POST",
        headers: { "Content-Type": "application/x-www-form-urlencoded" },
        body: new URLSearchParams({ title: next }),
      })
        .then((r) => (r.ok ? location.reload() : Promise.reject(r.status)))
        .catch((e) => setStatus("Rename failed: " + e));
    });
  });

  // Filter cards by name (ref + title) as you type — client-side, instant.
  const search = document.getElementById("media-search");
  if (search) {
    search.addEventListener("input", () => {
      const q = search.value.trim().toLowerCase();
      document.querySelectorAll(".media-card").forEach((card) => {
        const hit = !q || (card.dataset.search || "").includes(q);
        card.classList.toggle("hidden", !hit);
      });
    });
  }
})();
