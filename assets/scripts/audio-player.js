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
 *   - cross-page auto-advance (Phase DY): DV/DW nested each volume onto its OWN
 *     page (one embed), so the same-page chain above has no next — when the audio
 *     ends with the page VISIBLE, navigate to the next sibling (server-rendered
 *     #autoplay-next[data-href]) with ?play=1 and the destination auto-plays.
 *     Foreground-only: a hidden/locked page can't navigate + start gesture-less
 *     audio on iOS (accepted — the listener taps Next on return).
 *   - LOCKED-screen advance (DG.1 plan B, phone-proven necessary): iOS mutes
 *     a backgrounded element that never had a user gesture — `next.play()`
 *     advanced SILENTLY until unlock. So when `ended` fires with the page
 *     hidden, the JUST-FINISHED element (which owns the live audio session)
 *     ADOPTS the next track: swaps its src, saves under the next book's
 *     resume key, presents the next book's lock-screen metadata. On
 *     visibilitychange→visible it hands position back to the real per-book
 *     player (one tap to resume if iOS blocks the gestureless play there).
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

  // Cross-page auto-advance (Phase DY): with DV/DW the volumes are one-per-page, so a
  // volume page carries a SINGLE embed — the same-page chain (neighborAudio) has no
  // next. When the audio ends with the page VISIBLE, navigate to the next sibling
  // (the server renders #autoplay-next[data-href] when one exists) carrying ?play=1 so
  // the destination starts playing. Foreground-only BY DESIGN: a hidden/locked page
  // can't navigate + start gesture-less audio on iOS (the accepted limitation — the
  // listener taps Next on return). Best-effort: a browser autoplay block on arrival
  // leaves the next volume cued at its position (one tap).
  function crossPageAdvance() {
    const hook = document.getElementById("autoplay-next");
    const href = hook && hook.dataset.href;
    if (!href) return;
    location.href = href + (href.indexOf("?") >= 0 ? "&" : "?") + "play=1";
  }

  // Consume a ?play=1 left by a cross-page advance: play the first embed, then strip
  // the flag (so a manual refresh doesn't re-autoplay). No audio yet (embed still
  // settling via HTMX) → leave the flag and retry on the next settle. The play rides
  // the element's own resume/rate hooks; a browser block just leaves it cued.
  function maybeAutoplayFromNav() {
    const params = new URLSearchParams(location.search);
    if (params.get("play") !== "1") return;
    const first = document.querySelector(".audio-embed audio[data-ref]");
    if (!first) return;
    params.delete("play");
    const qs = params.toString();
    history.replaceState(
      null,
      "",
      location.pathname + (qs ? "?" + qs : "") + location.hash,
    );
    first.play().catch(() => {
      /* gesture-less play blocked — cued at position, one tap resumes */
    });
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
    // Locked-screen adoption state: when set, THIS element is playing the
    // NEXT book's stream on behalf of {ref, title, artwork, el} — saves and
    // lock-screen metadata follow the adopted track, not this element's own.
    let adopted = null;
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
      if (adopted) return; // adopted stream seeks via its own one-shot below
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
        // Under adoption the position belongs to the ADOPTED book.
        const key = adopted ? POS_PREFIX + adopted.ref : posKey;
        localStorage.setItem(key, String(audio.currentTime));
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
    // Adoption (locked-screen advance): swap THIS element's stream to the
    // next book. `src` overrides the <source> children, and the hand-back
    // reverts it (removeAttribute + load re-selects the children). Play
    // starts immediately to keep the audio session hot; the resume seek +
    // rate re-assert ride a one-shot loadedmetadata (guarded against a
    // second adoption racing in before it fires).
    const adoptTrack = (next) => {
      const srcEl = next.querySelector("source");
      const src = next.currentSrc || (srcEl ? srcEl.src : "");
      if (!src) return;
      // Park the OUTGOING book's position under its own key before the
      // adopted ref takes over save() — the periodic save is up to 5s stale.
      save();
      adopted = {
        ref: next.dataset.ref,
        title: next.dataset.title || "Audio",
        artwork: next.dataset.artwork || "",
        el: next,
      };
      const t = adopted;
      audio.src = src;
      audio.load();
      audio.addEventListener(
        "loadedmetadata",
        () => {
          if (adopted !== t) return;
          audio.playbackRate = RATES[rateIdx];
          const pos = parseFloat(
            localStorage.getItem(POS_PREFIX + t.ref) || "0",
          );
          if (pos > 3 && !(audio.duration && pos > audio.duration - 5))
            seekTo(pos);
        },
        { once: true },
      );
      audio.play().catch(() => {
        /* even the blessed element got blocked — one tap resumes */
      });
    };
    // Auto-advance: with the page VISIBLE, chain the real next element (its
    // own hooks apply resume + rate — phone-proven). With the page HIDDEN
    // (locked phone / backgrounded PWA), iOS keeps a gesture-less element
    // MUTED until unlock — phone-proven too — so the finished element,
    // which owns the live audio session, adopts the next stream instead.
    audio.addEventListener("ended", () => {
      const base = adopted ? adopted.el : audio;
      const next = neighborAudio(base, 1);
      if (!next) {
        // No same-page neighbor (the one-per-page volume case) — cross-page advance
        // when visible (Phase DY). Hidden/locked can't autoplay across a nav on iOS.
        if (!document.hidden) crossPageAdvance();
        return;
      }
      if (document.hidden) {
        adoptTrack(next);
      } else {
        next.play().catch(() => {
          /* blocked — the listener taps play; no worse than pre-DG */
        });
      }
    });
    // Hand-back: the moment the page is visible again, move playback to the
    // real per-book player at the carried position so the UI (chapters,
    // highlight, its own controls) matches what's playing. If iOS blocks the
    // gesture-less play here, the player sits paused at the RIGHT spot.
    document.addEventListener("visibilitychange", () => {
      if (document.visibilityState !== "visible" || !adopted) return;
      const t = adopted;
      const pos = audio.currentTime;
      const wasPlaying = !audio.paused;
      // Pause FIRST: the pause-triggered save must file under the adopted
      // book's key; clearing `adopted` before it would corrupt the host
      // book's resume position with the adopted book's time.
      audio.pause();
      adopted = null;
      audio.removeAttribute("src");
      audio.load();
      try {
        localStorage.setItem(POS_PREFIX + t.ref, String(pos));
      } catch {
        /* resume falls to the element's own saves */
      }
      t.el.currentTime = pos;
      if (wasPlaying) {
        t.el.play().catch(() => {
          /* one tap resumes — position is already right */
        });
      }
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
        if (adopted) return; // another book's times — highlight would lie
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
      // Blob URLs cached per RESOLVED track ref (own or adopted) — created
      // once each, never rebuilt, so a long locked-screen series marathon
      // accumulates one small blob per book, not per claim.
      const artCache = {};
      const claimSession = () => {
        const ms = navigator.mediaSession;
        // Under adoption the lock screen presents the ADOPTED book.
        const track = adopted
          ? { ref: adopted.ref, title: adopted.title, art: adopted.artwork }
          : { ref, title, art: audio.dataset.artwork };
        const setMeta = () => {
          try {
            const meta = { title: track.title, artist: "hotchkiss.io" };
            if (artCache[track.ref])
              meta.artwork = [{ src: artCache[track.ref], sizes: "512x512" }];
            ms.metadata = new MediaMetadata(meta);
          } catch {
            /* MediaMetadata unsupported — controls still work */
          }
        };
        setMeta();
        if (track.art && !(track.ref in artCache)) {
          artCache[track.ref] = null; // fetch once even if it fails
          // Credentialed in-page fetch → blob URL: the lock screen's own
          // fetch of a GATED cover may go out cookieless and 404; a blob is
          // local and needs no credentials.
          fetch(track.art, { credentials: "same-origin" })
            .then((r) =>
              r.ok
                ? r.blob()
                : Promise.reject(new Error("artwork " + r.status)),
            )
            .then((b) => {
              artCache[track.ref] = URL.createObjectURL(b);
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
        // Neighbors resolve from the ADOPTED element under adoption, so a
        // lock-screen "next" during book N+1 goes to N+2, not back to N+1.
        const jump = (delta) => () => {
          const base = adopted ? adopted.el : audio;
          const target = neighborAudio(base, delta);
          if (!target) return;
          if (document.hidden) {
            // Locked: a fresh element would play muted (the phone-proven
            // iOS behavior) — adopt in EITHER direction instead.
            adoptTrack(target);
            return;
          }
          audio.pause();
          target.play().catch(() => {});
        };
        const base = adopted ? adopted.el : audio;
        setHandler("nexttrack", neighborAudio(base, 1) ? jump(1) : null);
        setHandler("previoustrack", neighborAudio(base, -1) ? jump(-1) : null);
      };
      audio.addEventListener("play", claimSession);
    }
  }

  function scan() {
    document
      .querySelectorAll(".audio-embed audio[data-ref]")
      .forEach((a) => enhance(a));
  }

  // Enhance present embeds, then consume any cross-page ?play=1 (Phase DY) once the
  // audio has settled in.
  function onSettle() {
    scan();
    maybeAutoplayFromNav();
  }
  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", onSettle);
  } else {
    onSettle();
  }
  // Embeds arrive via HTMX swaps — enhance + autoplay-check as they settle.
  document.addEventListener("htmx:afterSettle", onSettle);
})();
