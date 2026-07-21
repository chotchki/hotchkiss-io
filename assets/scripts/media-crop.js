// ED.4/ED.6 — the 4-corner crop tool, mounted INLINE into `#crop-tool` on the
// media EDIT page (no modal — chris's call). RECT ⇄ 4-POINT mode toggle: rect
// drags axis-locked corners (standard crop feel); 4-point frees each corner so
// an angled sheet of paper lays flat. BOTH store the same 4 normalized corners
// (TL,TR,BR,BL) — the server homography-warps them into the derived rungs; the
// original is never touched. When a crop is already applied the preview shows
// CROPPED rungs, so the tool pushes Reset first (re-cropping over a cropped
// view would compound).
(function () {
  function setStatus(text) {
    const s = document.getElementById("media-upload-status");
    if (s) s.textContent = text;
  }

  function postCrop(ref, corners) {
    return fetch("/admin/media/" + ref + "/crop", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ corners: corners }),
    }).then((r) =>
      r.ok ? r.text() : r.text().then((t) => Promise.reject(t || r.status)),
    );
  }

  function mount(root) {
    const ref = root.dataset.mediaRef;
    const hasCrop = !!root.dataset.hasCrop;
    // Normalized TL,TR,BR,BL; start at a 10%-inset rect.
    let corners = [
      [0.1, 0.1],
      [0.9, 0.1],
      [0.9, 0.9],
      [0.1, 0.9],
    ];
    let mode = "rect";

    root.className = "flex flex-col gap-2";
    const bar = document.createElement("div");
    bar.className = "flex flex-row flex-wrap items-center gap-2";
    const mkBtn = (label, cls) => {
      const b = document.createElement("button");
      b.type = "button";
      b.textContent = label;
      b.className = "text-sm px-3 py-1.5 rounded " + cls;
      bar.appendChild(b);
      return b;
    };
    const modeBtn = mkBtn(
      "Mode: rectangle",
      "bg-white border border-navy text-navy hover:bg-navy/10",
    );
    const applyBtn = mkBtn(
      "Apply crop",
      "bg-yellow text-navy hover:bg-yellow/90",
    );
    let resetBtn = null;
    if (hasCrop) {
      resetBtn = mkBtn(
        "Reset existing crop",
        "bg-red-600 text-div-grey hover:bg-red-700",
      );
      const note = document.createElement("span");
      note.className = "text-xs text-navy/60";
      note.textContent =
        "A crop is applied — the preview below shows it. Reset before re-cropping.";
      bar.appendChild(note);
    }

    const stage = document.createElement("div");
    stage.className = "relative self-start max-w-full";
    const img = document.createElement("img");
    img.src = "/media/file/" + root.dataset.previewKey;
    img.className =
      "max-w-full max-h-[60vh] select-none rounded border border-navy/20";
    img.draggable = false;
    const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    svg.setAttribute("class", "absolute inset-0 w-full h-full");
    const poly = document.createElementNS(
      "http://www.w3.org/2000/svg",
      "polygon",
    );
    poly.setAttribute("fill", "rgba(255,201,53,0.15)");
    poly.setAttribute("stroke", "#ffc935");
    poly.setAttribute("stroke-width", "2");
    svg.appendChild(poly);
    const handles = corners.map(() => {
      const c = document.createElementNS(
        "http://www.w3.org/2000/svg",
        "circle",
      );
      c.setAttribute("r", "9");
      c.setAttribute("fill", "#ffc935");
      c.setAttribute("stroke", "#14213D");
      c.setAttribute("stroke-width", "2");
      c.style.cursor = "grab";
      svg.appendChild(c);
      return c;
    });
    stage.appendChild(img);
    stage.appendChild(svg);
    root.appendChild(bar);
    root.appendChild(stage);

    function redraw() {
      const w = img.clientWidth;
      const h = img.clientHeight;
      poly.setAttribute(
        "points",
        corners.map(([x, y]) => x * w + "," + y * h).join(" "),
      );
      handles.forEach((c, i) => {
        c.setAttribute("cx", corners[i][0] * w);
        c.setAttribute("cy", corners[i][1] * h);
      });
    }
    img.addEventListener("load", redraw);
    window.addEventListener("resize", redraw);
    redraw();

    // Rect mode keeps the quad axis-aligned: moving corner i drags the shared
    // x of its vertical neighbor and the shared y of its horizontal neighbor.
    // Neighbor tables for TL(0),TR(1),BR(2),BL(3).
    const X_PAIR = [3, 2, 1, 0];
    const Y_PAIR = [1, 0, 3, 2];
    let dragging = -1;
    handles.forEach((c, i) => {
      c.addEventListener("pointerdown", (e) => {
        dragging = i;
        c.setPointerCapture(e.pointerId);
        e.preventDefault();
      });
    });
    svg.addEventListener("pointermove", (e) => {
      if (dragging < 0) return;
      const r = img.getBoundingClientRect();
      const x = Math.min(1, Math.max(0, (e.clientX - r.left) / r.width));
      const y = Math.min(1, Math.max(0, (e.clientY - r.top) / r.height));
      corners[dragging] = [x, y];
      if (mode === "rect") {
        corners[X_PAIR[dragging]][0] = x;
        corners[Y_PAIR[dragging]][1] = y;
      }
      redraw();
    });
    svg.addEventListener("pointerup", () => {
      dragging = -1;
    });

    modeBtn.addEventListener("click", () => {
      if (mode === "rect") {
        mode = "quad";
        modeBtn.textContent = "Mode: 4-point";
      } else {
        mode = "rect";
        modeBtn.textContent = "Mode: rectangle";
        // Snap the free quad to its bounding box on the way back.
        const xs = corners.map((c) => c[0]);
        const ys = corners.map((c) => c[1]);
        const [x0, x1] = [Math.min(...xs), Math.max(...xs)];
        const [y0, y1] = [Math.min(...ys), Math.max(...ys)];
        corners = [
          [x0, y0],
          [x1, y0],
          [x1, y1],
          [x0, y1],
        ];
        redraw();
      }
    });

    // Reload only once the spawned re-derivation LANDS (window.MediaDerive
    // polls the manifest) — a fixed-delay reload showed the old/empty rungs
    // (the prod rotate-preview bug's sibling).
    applyBtn.addEventListener("click", () => {
      applyBtn.disabled = true;
      postCrop(ref, corners)
        .then(() => {
          setStatus("Cropping — the page refreshes when the variants land…");
          window.MediaDerive.awaitDeriveThenReload(ref);
        })
        .catch((e) => {
          applyBtn.disabled = false;
          setStatus("Crop failed: " + e);
        });
    });
    if (resetBtn) {
      resetBtn.addEventListener("click", () => {
        postCrop(ref, null)
          .then(() => {
            setStatus(
              "Crop cleared — the page refreshes when the variants land…",
            );
            window.MediaDerive.awaitDeriveThenReload(ref);
          })
          .catch((e) => setStatus("Reset failed: " + e));
      });
    }
  }

  document.addEventListener("DOMContentLoaded", () => {
    const root = document.getElementById("crop-tool");
    if (root) mount(root);
  });
})();
