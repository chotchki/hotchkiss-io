(function () {
  "use strict";

  // Renumber the visible position inputs (DZ.5) to their new 1-based DOM order after a
  // reorder — a drag or a typed jump leaves the OTHER rows' numbers stale otherwise
  // (the POST is hx-swap="none", no reload).
  function renumber(form) {
    const rows = form.querySelectorAll("[data-reorder-row]");
    for (let i = 0; i < rows.length; i++) {
      const inp = rows[i].querySelector("[data-reorder-position]");
      if (inp) inp.value = String(i + 1);
    }
  }

  // Numeric-position reorder (DZ.5) — for a long list where dragging is impractical (a
  // 271-volume series). A [data-reorder-position] input: on change, move its row to that
  // 1-based position, renumber, then fire the form's `end` (the SAME reorder POST the
  // drag fires). Bound ONCE at document level (htmx.onLoad runs per swap — a listener
  // added there would stack).
  document.addEventListener("change", function (evt) {
    const input =
      evt.target && evt.target.closest
        ? evt.target.closest("[data-reorder-position]")
        : null;
    if (!input) return;
    const form = input.closest(".sortable");
    const row = input.closest("[data-reorder-row]");
    if (!form || !row) return;
    const rows = Array.prototype.slice.call(
      form.querySelectorAll("[data-reorder-row]"),
    );
    let pos = parseInt(input.value, 10);
    if (Number.isNaN(pos)) {
      renumber(form); // restore the shown number
      return;
    }
    if (pos < 1) pos = 1;
    if (pos > rows.length) pos = rows.length;
    const from = rows.indexOf(row);
    if (pos - 1 === from) {
      renumber(form);
      return;
    }
    const ref = rows[pos - 1];
    if (pos - 1 < from) {
      form.insertBefore(row, ref);
    } else {
      ref.parentNode.insertBefore(row, ref.nextSibling);
    }
    renumber(form);
    if (window.htmx) htmx.trigger(form, "end");
  });

  htmx.onLoad(function (content) {
    const sortables = content.querySelectorAll(".sortable");
    for (let i = 0; i < sortables.length; i++) {
      const sortable = sortables[i];
      const opts = {
        animation: 150,
        ghostClass: "blue-background-class",

        // Make the `.htmx-indicator` unsortable
        filter: ".htmx-indicator",
        onMove: function (evt) {
          return evt.related.className.indexOf("htmx-indicator") === -1;
        },

        // Disable sorting on the `end` event, then renumber the position inputs.
        onEnd: function () {
          this.option("disabled", true);
          renumber(this.el);
        },
      };
      // Restrict the drag to a grip handle WHEN present (a row with an editable number
      // input needs the input to stay clickable). Lists without a handle (the
      // /admin/pages nav-order list) keep whole-row dragging — unchanged.
      if (sortable.querySelector("[data-reorder-handle]")) {
        opts.handle = "[data-reorder-handle]";
      }
      const sortableInstance = new Sortable(sortable, opts);

      // Re-enable sorting once the request finishes. Use `afterRequest` (not
      // `afterSwap`) so it also fires for hx-swap="none" reorder posts that
      // return no body.
      sortable.addEventListener("htmx:afterRequest", function () {
        sortableInstance.option("disabled", false);
      });
    }
  });
})();
