/*
 * Audiobook player (Phase DD) — enhances the native <audio> element the
 * /media embed emits (span.audio-embed > audio[data-ref]) with:
 *   - a chapter list (data-chapters JSON) with tap-to-seek + live highlight,
 *   - ±30s skip buttons and a playback-rate cycle button,
 *   - MediaSession metadata + lock-screen controls; artwork loads via a
 *     CREDENTIALED fetch → blob URL so a GATED cover still shows on the lock
 *     screen (WebKit's out-of-page artwork fetch may not carry cookies),
 *   - localStorage resume (key audio-pos:<ref>) applied at loadedmetadata AND
 *     re-asserted on first play (iOS silently drops a currentTime set before
 *     metadata). Never autoplays on page load,
 *   - playlist auto-advance (Phase DG): a page's audio embeds in document
 *     order ARE the series order — on `ended` the next volume plays (chained
 *     from an active playback, not a page-load autoplay), MediaSession
 *     nexttrack/prevtrack skip between volumes from the lock screen, and
 *     starting one player pauses the others. A single-embed page is a
 *     playlist of one — none of this fires.
 *
 * Embeds arrive via an HTMX swap, so we scan on load AND after settles,
 * guarded per-element by data-enhanced. Degrades to the bare native controls
 * without this script. Server position sync is Phase DF; localStorage stays
 * the per-device fallback.
 */
(function () {
  "use strict";

  const POS_PREFIX = "audio-pos:";
  const RATE_KEY = "audio-rate"; // global, not per-book — speed is a listener trait
  const SAVE_EVERY_MS = 5000;
  const SKIP_SECONDS = 30;
  const RATES = [1, 1.25, 1.5, 1.6, 2];

  function fmtTime(totalSeconds) {
    const s = Math.max(0, Math.floor(totalSeconds));
    const h = Math.floor(s / 3600);
    const m = Math.floor((s % 3600) / 60);
    const sec = String(s % 60).padStart(2, "0");
    return h > 0
      ? h + ":" + String(m).padStart(2, "0") + ":" + sec
      : m + ":" + sec;
  }

  // Playlist neighbors resolve against the LIVE DOM at call time, not at
  // enhance time — embeds arrive via independent HTMX swaps in any order,
  // and document order is the series order the author wrote.
  function neighborAudio(audio, delta) {
    const all = Array.from(
      document.querySelectorAll(".audio-embed audio[data-ref]"),
    );
    const i = all.indexOf(audio);
    return i < 0 ? null : all[i + delta] || null;
  }

  function button(label, title, onClick) {
    const b = document.createElement("button");
    b.type = "button";
    b.textContent = label;
    b.title = title;
    b.className =
      "px-3 py-1.5 rounded text-sm bg-navy text-div-grey hover:bg-navy/90";
    b.addEventListener("click", onClick);
    return b;
  }

  function enhance(audio) {
    if (audio.dataset.enhanced) return;
    audio.dataset.enhanced = "1";
    const ref = audio.dataset.ref;
    const title = audio.dataset.title || "Audio";

    // ---- resume (per-device localStorage; server sync is Phase DF) ----
    const posKey = POS_PREFIX + ref;
    const savedPos = parseFloat(localStorage.getItem(posKey) || "0");
    let resumed = false;
    // Distinguish OUR seeks from the user's: a user who scrubs the timeline
    // before pressing play (e.g. deliberately back to 0:00 to restart) must
    // never be yanked back by the first-play re-assert below.
    let programmaticSeek = false;
    let userSeeked = false;
    const seekTo = (t) => {
      programmaticSeek = true;
      audio.currentTime = t;
    };
    audio.addEventListener("seeking", () => {
      if (programmaticSeek) {
        programmaticSeek = false;
        return;
      }
      userSeeked = true;
    });
    const applyResume = () => {
      if (resumed || !(savedPos > 3)) return; // near-zero → just start over
      if (audio.duration && savedPos > audio.duration - 5) return; // finished
      seekTo(savedPos);
      resumed = true;
    };
    audio.addEventListener("loadedmetadata", applyResume);
    // iOS can drop a seek issued before metadata is truly ready — re-assert
    // ONCE on the first play (unless the user has scrubbed themselves), then
    // never fight the user's own seeking again.
    audio.addEventListener("play", function onFirstPlay() {
      audio.removeEventListener("play", onFirstPlay);
      audio.playbackRate = RATES[rateIdx]; // iOS may reset rate when the stream loads
      if (userSeeked) return;
      if (!resumed) applyResume();
      else if (savedPos > 3 && audio.currentTime < 1) seekTo(savedPos);
    });
    let lastSave = 0;
    const save = () => {
      try {
        localStorage.setItem(posKey, String(audio.currentTime));
      } catch {
        /* storage full/blocked — resume just won't persist */
      }
    };
    audio.addEventListener("timeupdate", () => {
      const now = Date.now();
      if (now - lastSave > SAVE_EVERY_MS) {
        lastSave = now;
        save();
      }
    });
    audio.addEventListener("pause", save);

    // ---- playlist (Phase DG) ----
    // Exclusive playback: two volumes narrating over each other is never
    // what anyone wants.
    audio.addEventListener("play", () => {
      document
        .querySelectorAll(".audio-embed audio[data-ref]")
        .forEach((other) => {
          if (other !== audio && !other.paused) other.pause();
        });
    });
    // Auto-advance: chain the next volume from an ENDED playback — the same
    // active audio session, which is what lets iOS allow it (a page-load
    // autoplay would be blocked, and we never do that). The next player's own
    // first-play hooks apply its saved position + the global rate. If iOS
    // still blocks with the screen locked, the src-swap-on-one-element
    // fallback is the DG.1 plan B — validate on the real phone first.
    audio.addEventListener("ended", () => {
      const next = neighborAudio(audio, 1);
      if (!next) return;
      next.play().catch(() => {
        /* blocked — the listener taps play; no worse than pre-DG */
      });
    });

    const skip = (delta) => {
      seekTo(Math.max(0, audio.currentTime + delta));
    };

    // ---- controls row: -30s / +30s / rate cycle ----
    const controls = document.createElement("span");
    controls.className = "flex flex-row flex-wrap items-center gap-2";
    controls.appendChild(
      button("−30s", "Back 30 seconds", () => skip(-SKIP_SECONDS)),
    );
    controls.appendChild(
      button("+30s", "Forward 30 seconds", () => skip(SKIP_SECONDS)),
    );
    // Speed persists across books/devices' sessions (a 1.6× listener is a
    // 1.6× listener); an unknown stored value (older RATES list) falls to 1×.
    let rateIdx = RATES.indexOf(
      parseFloat(localStorage.getItem(RATE_KEY) || "1"),
    );
    if (rateIdx < 0) rateIdx = 0;
    audio.playbackRate = RATES[rateIdx];
    const rateBtn = button(RATES[rateIdx] + "×", "Playback speed", () => {
      rateIdx = (rateIdx + 1) % RATES.length;
      audio.playbackRate = RATES[rateIdx];
      rateBtn.textContent = RATES[rateIdx] + "×";
      try {
        localStorage.setItem(RATE_KEY, String(RATES[rateIdx]));
      } catch {
        /* storage full/blocked — speed just won't persist */
      }
    });
    controls.appendChild(rateBtn);
    audio.insertAdjacentElement("afterend", controls);

    // ---- chapters (tap-to-seek, live highlight) ----
    let chapters = [];
    try {
      chapters = JSON.parse(audio.dataset.chapters || "[]");
    } catch {
      chapters = [];
    }
    const rows = [];
    if (chapters.length) {
      // Collapsed by default — a 13h audiobook's ~50 chapters would otherwise
      // dominate the page. `hidden` beats <details>: the embed lives inside a
      // <p>, where flow content like <details> is invalid (the STL <span> rule).
      const list = document.createElement("span");
      list.className =
        "hidden flex-col w-full mt-1 border border-navy/20 rounded divide-y divide-navy/10";
      const chapBtn = button(
        "Chapters (" + chapters.length + ") ▸",
        "Show chapters",
        () => {
          const open = list.classList.toggle("hidden") === false;
          list.classList.toggle("flex", open);
          chapBtn.textContent =
            "Chapters (" + chapters.length + ") " + (open ? "▾" : "▸");
        },
      );
      controls.appendChild(chapBtn);
      chapters.forEach((ch, i) => {
        const startS = (ch.start_ms || 0) / 1000;
        const row = document.createElement("button");
        row.type = "button";
        row.className =
          "chapter-row flex flex-row justify-between px-3 py-1.5 text-sm text-left text-navy hover:bg-navy/5";
        const name = document.createElement("span");
        name.textContent = ch.title || "Chapter " + (i + 1);
        const time = document.createElement("span");
        time.className = "text-navy/50";
        time.textContent = fmtTime(startS);
        row.appendChild(name);
        row.appendChild(time);
        row.addEventListener("click", () => {
          seekTo(startS);
          audio.play();
        });
        list.appendChild(row);
        rows.push({ row, startS });
      });
      controls.insertAdjacentElement("afterend", list);
      audio.addEventListener("timeupdate", () => {
        let current = -1;
        for (let i = 0; i < rows.length; i++) {
          if (audio.currentTime >= rows[i].startS) current = i;
        }
        rows.forEach(({ row }, i) => {
          row.classList.toggle("bg-navy/10", i === current);
        });
      });
    }

    // ---- MediaSession: lock-screen metadata + transport controls ----
    // navigator.mediaSession is ONE GLOBAL per document — claimed on each
    // element's `play` event, so the PLAYING book owns the lock screen even
    // with several embeds on a page (enhance-time binding made the last one
    // in DOM order win). Artwork fetches lazily on first claim: a page of N
    // gated covers doesn't fetch N images, and the blob URL is cached
    // per-element (created once, never rebuilt → nothing accumulates).
    if ("mediaSession" in navigator) {
      let artworkUrl = null;
      let artworkTried = false;
      const claimSession = () => {
        const ms = navigator.mediaSession;
        const setMeta = () => {
          try {
            const meta = { title, artist: "hotchkiss.io" };
            if (artworkUrl)
              meta.artwork = [{ src: artworkUrl, sizes: "512x512" }];
            ms.metadata = new MediaMetadata(meta);
          } catch {
            /* MediaMetadata unsupported — controls still work */
          }
        };
        setMeta();
        const art = audio.dataset.artwork;
        if (art && !artworkTried) {
          artworkTried = true;
          // Credentialed in-page fetch → blob URL: the lock screen's own
          // fetch of a GATED cover may go out cookieless and 404; a blob is
          // local and needs no credentials.
          fetch(art, { credentials: "same-origin" })
            .then((r) =>
              r.ok
                ? r.blob()
                : Promise.reject(new Error("artwork " + r.status)),
            )
            .then((b) => {
              artworkUrl = URL.createObjectURL(b);
              setMeta();
            })
            .catch(() => {
              /* no artwork — metadata alone is fine */
            });
        }
        const setHandler = (action, fn) => {
          try {
            ms.setActionHandler(action, fn);
          } catch {
            /* action unsupported on this platform */
          }
        };
        setHandler("play", () => audio.play());
        setHandler("pause", () => audio.pause());
        setHandler("seekbackward", (d) =>
          skip(-((d && d.seekOffset) || SKIP_SECONDS)),
        );
        setHandler("seekforward", (d) =>
          skip((d && d.seekOffset) || SKIP_SECONDS),
        );
        setHandler("seekto", (d) => {
          if (d && typeof d.seekTime === "number") seekTo(d.seekTime);
        });
        // Volume skip (Phase DG): the lock screen jumps between BOOKS; the
        // ±30s seeks above stay the in-volume controls. A null handler at
        // the ends hides the button — a single-book page shows no next/prev.
        // Re-resolved on every claim (each `play`), so late-settling embeds
        // are picked up by the time anyone can press the button.
        const jump = (delta) => () => {
          const target = neighborAudio(audio, delta);
          if (!target) return;
          audio.pause();
          target.play().catch(() => {});
        };
        setHandler("nexttrack", neighborAudio(audio, 1) ? jump(1) : null);
        setHandler("previoustrack", neighborAudio(audio, -1) ? jump(-1) : null);
      };
      audio.addEventListener("play", claimSession);
    }
  }

  function scan() {
    document
      .querySelectorAll(".audio-embed audio[data-ref]")
      .forEach((a) => enhance(a));
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", scan);
  } else {
    scan();
  }
  // Embeds arrive via HTMX swaps — enhance them as they settle.
  document.addEventListener("htmx:afterSettle", scan);
})();
