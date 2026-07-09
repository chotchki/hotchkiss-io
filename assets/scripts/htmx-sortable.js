htmx.onLoad(function (content) {
  const sortables = content.querySelectorAll(".sortable");
  for (let i = 0; i < sortables.length; i++) {
    const sortable = sortables[i];
    const sortableInstance = new Sortable(sortable, {
      animation: 150,
      ghostClass: "blue-background-class",

      // Make the `.htmx-indicator` unsortable
      filter: ".htmx-indicator",
      onMove: function (evt) {
        return evt.related.className.indexOf("htmx-indicator") === -1;
      },

      // Disable sorting on the `end` event
      onEnd: function () {
        this.option("disabled", true);
      },
    });

    // Re-enable sorting once the request finishes. Use `afterRequest` (not
    // `afterSwap`) so it also fires for hx-swap="none" reorder posts that
    // return no body.
    sortable.addEventListener("htmx:afterRequest", function () {
      sortableInstance.option("disabled", false);
    });
  }
});
