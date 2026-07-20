// EB — the quick-capture flow. Each picked/shot photo: (1) uploads the ORIGINAL
// via the canonical POST /media (progress via UploadProgress), (2) POSTs the ref
// to /admin/capture with the chosen mode. After a "new draft" post, the page
// auto-switches to append-to-that-draft, so a multi-shot session accretes into
// ONE post instead of minting a draft per photo.
(function () {
  function $(id) {
    return document.getElementById(id);
  }

  function mode() {
    const checked = document.querySelector(
      'input[name="capture-mode"]:checked',
    );
    return checked ? checked.value : "draft";
  }

  function setStatus(text) {
    const s = $("capture-status");
    if (s) s.textContent = text;
  }

  function uploadOne(file) {
    const fd = new FormData();
    fd.append("file", file, file.name);
    return UploadProgress.xhrUpload("/media", fd, (phase, loaded, total) => {
      const bar = $("capture-bar");
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
        if (bar) bar.removeAttribute("value");
        setStatus("Processing…");
      }
    }).then((text) => JSON.parse(text).ref);
  }

  function postCapture(ref) {
    const params = new URLSearchParams();
    params.set("media_ref", ref);
    params.set("mode", mode());
    if (mode() === "append") {
      const target = $("capture-target");
      params.set("target", target ? target.value : "");
    }
    const caption = $("capture-caption");
    params.set("caption", caption ? caption.value : "");
    return fetch("/admin/capture", {
      method: "POST",
      headers: {
        "Content-Type": "application/x-www-form-urlencoded",
        Accept: "application/json",
      },
      body: params.toString(),
    }).then((r) => {
      if (!r.ok) {
        return r.text().then((t) => {
          throw new Error(t || "capture failed (" + r.status + ")");
        });
      }
      return r.json();
    });
  }

  function onPosted(page) {
    const url = "/pages/" + page.path_segments.join("/");
    const result = $("capture-result");
    if (result) {
      result.classList.remove("hidden");
      result.textContent = "Added to “" + page.title + "” — ";
      const view = document.createElement("a");
      view.href = url + "?edit";
      view.textContent = "open the draft";
      view.className = "underline text-navy hover:text-navy/70";
      result.appendChild(view);
    }
    const caption = $("capture-caption");
    if (caption) caption.value = "";
    // Session accretion: the draft just minted becomes the append target.
    if (mode() === "draft") {
      const select = $("capture-target");
      const append = document.querySelector(
        'input[name="capture-mode"][value="append"]',
      );
      if (select && append) {
        const opt = document.createElement("option");
        opt.value = page.slug;
        opt.textContent = page.title + (page.scheduled ? " (draft)" : "");
        select.insertBefore(opt, select.firstChild);
        select.value = page.slug;
        select.disabled = false;
        append.disabled = false;
        append.checked = true;
      }
    }
  }

  async function handleFiles(files) {
    const list = Array.from(files || []).filter((f) => f && f.size);
    const bar = $("capture-bar");
    // Sequential on purpose: the first file may create the draft the rest
    // append to (the mode auto-switch above happens between files).
    for (const f of list) {
      try {
        const ref = await uploadOne(f);
        setStatus("Posting…");
        const envelope = await postCapture(ref);
        if (envelope.page) onPosted(envelope.page);
        setStatus("Done");
      } catch (e) {
        setStatus("Failed: " + (e && e.message ? e.message : e));
        break;
      }
    }
    if (bar) {
      bar.classList.add("hidden");
      bar.value = 0;
    }
  }

  document.addEventListener("DOMContentLoaded", () => {
    for (const id of ["capture-camera", "capture-library"]) {
      const input = $(id);
      if (input) {
        input.addEventListener("change", () => {
          handleFiles(input.files);
          input.value = "";
        });
      }
    }
  });
})();
