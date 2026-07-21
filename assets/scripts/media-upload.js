// Media library UI (Phase BZ; DR — driven off the canonical /media REST surface).
// Drag-drop / click-to-select → POST /media; add-encode → POST /media/<ref>/variants;
// rename/visibility → PUT /media/<ref>; delete → DELETE /media/<ref>[/variants/<key>].
// On success we reload the library (the REST responses are JSON/204, not htmx). Zero
// deps; the page is admin-gated server-side (the mutation layer gates the writes).
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
    // Default visibility for this upload (DC.5) — the drop zone's select.
    const vis = document.getElementById("media-upload-visibility");
    if (vis && vis.value !== "Public") fd.append("min_role", vis.value);
    if (drop) drop.classList.add("opacity-50", "pointer-events-none");
    UploadProgress.xhrUpload("/media", fd, showProgress)
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
      const ref = inp.dataset.mediaRef;
      const files = Array.from(inp.files || []);
      if (!files.length) return;
      const fd = new FormData();
      for (const f of files) fd.append("file", f, f.name);
      UploadProgress.xhrUpload("/media/" + ref + "/variants", fd, showProgress)
        .then(() => location.reload())
        .catch((e) => {
          setStatus("Add failed: " + e);
          hideBar();
        });
    });
  });

  // Rename via the edit page's title input (ED.6 — the prompt() popup died
  // with the modals) → PUT /media/<ref> {title}. The ref stays fixed so
  // embeds don't break; an absent min_role KEEPS the gate (fail-safe).
  document.querySelectorAll(".save-title").forEach((btn) => {
    btn.addEventListener("click", () => {
      const input = document.getElementById("edit-title");
      if (!input) return;
      fetch("/media/" + btn.dataset.mediaRef, {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ title: input.value }),
      })
        .then((r) => (r.ok ? location.reload() : Promise.reject(r.status)))
        .catch((e) => setStatus("Rename failed: " + e));
    });
  });

  // Change the per-item visibility gate → PUT /media/<ref> {min_role}. "Public"
  // clears the gate; a role sets it. Reload so the badge + selector re-render.
  document.querySelectorAll(".media-visibility").forEach((sel) => {
    sel.addEventListener("change", () => {
      fetch("/media/" + sel.dataset.mediaRef, {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ min_role: sel.value }),
      })
        .then((r) => (r.ok ? location.reload() : Promise.reject(r.status)))
        .catch((e) => setStatus("Visibility change failed: " + e));
    });
  });

  // Delete ONE stored stream → DELETE /media/<ref>/variants/<url_key>.
  document.querySelectorAll(".delete-variant").forEach((btn) => {
    btn.addEventListener("click", () => {
      // Destructive intent is gated by hold-confirm.js (data-hold-confirm) —
      // this handler only ever fires after a completed hold.
      fetch(
        "/media/" + btn.dataset.mediaRef + "/variants/" + btn.dataset.urlKey,
        {
          method: "DELETE",
        },
      )
        .then((r) => (r.ok ? location.reload() : Promise.reject(r.status)))
        .catch((e) => setStatus("Delete failed: " + e));
    });
  });

  // The re-derivation after rotate/crop/re-derive is SPAWNED server-side (rav1e
  // takes seconds), and the derived rungs are DROPPED synchronously first — so
  // "an image/avif variant exists again" in the item manifest IS the completion
  // signal. Poll it, then reload so the preview/crop tool show the new state
  // (the prod dogfood bug: rotate worked but nothing on the page ever changed).
  // Timeout fallback reloads anyway (~45s covers a big photo; a small unedited
  // image can legitimately end with zero rungs).
  function awaitDeriveThenReload(ref) {
    const deadline = Date.now() + 45000;
    const tick = () => {
      fetch("/media/" + ref, { headers: { Accept: "application/json" } })
        .then((r) => (r.ok ? r.json() : Promise.reject(r.status)))
        .then((m) => {
          const done = (m.variants || []).some(
            (v) => (v.type || "") === "image/avif",
          );
          if (done || Date.now() > deadline) {
            location.reload();
          } else {
            setTimeout(tick, 1500);
          }
        })
        .catch(() => setTimeout(tick, 1500));
    };
    setTimeout(tick, 1500);
  }
  window.MediaDerive = { awaitDeriveThenReload };

  // Rotate a quarter-turn (ED.3): bumps metadata.edit.rotate server-side and
  // re-derives — the original is never touched; four turns = a full undo.
  document.querySelectorAll(".rotate-media").forEach((btn) => {
    btn.addEventListener("click", () => {
      btn.disabled = true;
      fetch("/admin/media/" + btn.dataset.mediaRef + "/rotate", {
        method: "POST",
        headers: { "Content-Type": "application/x-www-form-urlencoded" },
        body: "dir=" + btn.dataset.dir,
      })
        .then((r) =>
          r.ok ? r.text() : r.text().then((t) => Promise.reject(t || r.status)),
        )
        .then(() => {
          setStatus("Rotating — the page refreshes when the variants land…");
          awaitDeriveThenReload(btn.dataset.mediaRef);
        })
        .catch((e) => {
          btn.disabled = false;
          setStatus("Rotate failed: " + e);
        });
    });
  });

  // Re-derive an image's AVIF rungs from its original (ED.1) — spawned server
  // side; the button reports and the admin refreshes when ready.
  document.querySelectorAll(".rederive-media").forEach((btn) => {
    btn.addEventListener("click", () => {
      btn.disabled = true;
      btn.textContent = "Re-deriving…";
      fetch("/admin/media/" + btn.dataset.mediaRef + "/rederive", {
        method: "POST",
      })
        .then((r) =>
          r.ok ? r.text() : r.text().then((t) => Promise.reject(t || r.status)),
        )
        .then(() => {
          setStatus("Re-deriving — the page refreshes when the variants land…");
          awaitDeriveThenReload(btn.dataset.mediaRef);
        })
        .catch((e) => {
          btn.disabled = false;
          btn.textContent = "Re-derive";
          setStatus("Re-derive failed: " + e);
        });
    });
  });

  // Delete the whole item → DELETE /media/<ref> (CASCADEs its variants).
  document.querySelectorAll(".delete-media").forEach((btn) => {
    btn.addEventListener("click", () => {
      // Destructive intent is gated by hold-confirm.js (data-hold-confirm).
      fetch("/media/" + btn.dataset.mediaRef, { method: "DELETE" })
        .then((r) => {
          if (!r.ok) return Promise.reject(r.status);
          // The edit page deletes its own subject — go back to the library.
          if (btn.dataset.redirect) {
            location.href = btn.dataset.redirect;
          } else {
            location.reload();
          }
        })
        .catch((e) => setStatus("Delete failed: " + e));
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
