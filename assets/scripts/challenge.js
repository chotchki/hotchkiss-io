"use strict";
// Greylist toll interstitial driver (Phase CX). Fetches a fresh challenge, paints the toll
// image so the human sees it, hands the raw pixels + seed to the worker to solve, then hits
// /challenge/verify — a 302 sends the browser back to the page it was trying to reach.
// Self-contained, same-origin only; no external requests.
(function () {
  "use strict";
  const main = document.querySelector("main[data-redir]");
  if (!main) return;
  const redir = main.getAttribute("data-redir") || "/";
  const workerUrl = main.getAttribute("data-worker");
  const statusEl = document.getElementById("toll-status");
  const progEl = document.getElementById("toll-progress");
  const canvas = document.getElementById("toll-canvas");

  function status(t) { if (statusEl) statusEl.textContent = t; }

  function b64urlDecode(s) {
    s = s.replace(/-/g, "+").replace(/_/g, "/");
    const bin = atob(s);
    const out = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
    return out;
  }

  async function run() {
    let tok;
    try {
      const r = await fetch("/challenge/new", { cache: "no-store" });
      tok = await r.json();
    } catch (e) {
      return status("Couldn't reach the toll booth. Reload to try again.");
    }

    let rgba;
    try {
      const buf = await (await fetch(tok.image_url)).arrayBuffer();
      rgba = new Uint8ClampedArray(buf);
    } catch (e) {
      return status("Couldn't load the toll image. Reload to try again.");
    }

    // Paint the tollbooth so the human sees what they're paying for.
    if (canvas && canvas.getContext && rgba.length === tok.width * tok.height * 4) {
      canvas.width = tok.width;
      canvas.height = tok.height;
      try {
        canvas.getContext("2d").putImageData(new ImageData(rgba, tok.width, tok.height), 0, 0);
      } catch (e) { /* paint is decorative; a failure doesn't block the solve */ }
    }

    let worker;
    try {
      worker = new Worker(workerUrl);
    } catch (e) {
      return status("Your browser blocked the worker this toll needs.");
    }

    worker.onmessage = (ev) => {
      const m = ev.data;
      if (m.type === "progress") {
        if (progEl) progEl.value = m.pct;
        status("Paying the toll… " + m.pct + "%");
      } else if (m.type === "done") {
        if (progEl) progEl.value = 100;
        status("Toll paid. Sending you through…");
        const qs =
          "inner_seed=" + encodeURIComponent(tok.inner_seed) +
          "&ts=" + encodeURIComponent(tok.ts) +
          "&version=" + encodeURIComponent(tok.version) +
          "&answer=" + encodeURIComponent(m.answer) +
          "&ms=" + encodeURIComponent(m.ms || 0) +
          "&redir=" + encodeURIComponent(redir);
        window.location.href = "/challenge/verify?" + qs;
      }
    };
    worker.onerror = () => status("The toll solver hit an error. Reload to try again.");

    // Copy the buffer for the worker (structured clone); the main thread keeps its own for paint.
    worker.postMessage({ rgba: new Uint8Array(rgba.buffer.slice(0)), seed: b64urlDecode(tok.seed) });
  }

  run();
})();
