// Enhance the analytics custom-range inputs with flatpickr (Phase CT). The inputs
// are plain text so no-JS still submits a usable value (native form GET, treated as
// UTC — the whole dashboard is UTC); flatpickr just makes picking decent. Runs on
// load AND after every htmx swap, since the CQ.7 control model replaces
// #analytics-content (and the inputs with it) on each toggle.
(function () {
  function initPickers(root) {
    if (typeof flatpickr === "undefined") return;
    (root || document)
      .querySelectorAll("input[data-flatpickr]")
      .forEach(function (el) {
        if (el._flatpickr) return; // already enhanced (idempotent across re-inits)
        flatpickr(el, {
          enableTime: true,
          time_24hr: true,
          dateFormat: "Y-m-d H:i", // matches the server's UTC parse (parse_range_dt)
          allowInput: true, // let the field be typed/cleared, not only clicked
        });
      });
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", function () {
      initPickers(document);
    });
  } else {
    initPickers(document);
  }
  // htmx swaps #analytics-content on every range/audience/paths toggle → the fresh
  // inputs need enhancing. Events bubble to document.
  document.addEventListener("htmx:afterSwap", function (e) {
    initPickers(e.target || document);
  });
})();
