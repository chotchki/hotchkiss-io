// Analytics traffic line chart (CQ.7) — ported from recon-gen's renderLineChart.
// Reads the numeric JSON island (#ts-data) the server emits and draws a two-series
// (total views + unique visitors) overlay into #ts-chart with vendored d3@7. Re-runs
// on htmx:afterSwap because the control-model rework swaps the whole #analytics-content
// wrapper (a fresh island) on every range/audience toggle. No island / no d3 → no-op
// (the page degrades to its server-rendered tables). Loaded deferred + admin-only.
(function () {
  "use strict";

  function render() {
    var container = document.getElementById("ts-chart");
    var island = document.getElementById("ts-data");
    if (!container || !island || typeof d3 === "undefined") return;

    var data;
    try {
      data = JSON.parse(island.textContent);
    } catch (e) {
      return;
    }

    container.innerHTML = "";
    if (!data || data.empty || !data.days || data.days.length === 0) {
      container.innerHTML =
        '<p style="color:#6b7280;padding:1rem;font-size:0.875rem">No traffic in this range yet.</p>';
      return;
    }

    var days = data.days;
    var series = [
      { name: "Total views", values: data.total, color: "#14213d" },
      { name: "Unique visitors", values: data.unique, color: "#e8833a" },
    ];
    // Greylist tolls/day overlay (CY.2) — always challenged=1, independent of the audience
    // filter. Drawn ONLY when the window actually walled someone, so a quiet site keeps the
    // clean two-line chart instead of a flat-zero third line.
    if (data.challenged && data.challenged.some(function (v) { return v > 0; })) {
      series.push({ name: "Tolls served", values: data.challenged, color: "#b91c1c" });
    }

    var width = Math.max(container.clientWidth || 720, 320);
    var height = 240;
    var margin = { top: 24, right: 16, bottom: 28, left: 44 };
    var innerW = width - margin.left - margin.right;
    var innerH = height - margin.top - margin.bottom;

    var x = d3.scalePoint().domain(days).range([0, innerW]);
    var yMax = d3.max(series, function (s) { return d3.max(s.values); }) || 1;
    var y = d3.scaleLinear().domain([0, yMax]).nice().range([innerH, 0]);

    var svg = d3
      .select(container)
      .append("svg")
      .attr("viewBox", "0 0 " + width + " " + height)
      .attr("preserveAspectRatio", "xMidYMid meet")
      .attr("role", "img")
      .attr("aria-label", "traffic per day")
      .style("width", "100%")
      .style("height", "auto")
      .style("font-family", "sans-serif");

    var g = svg
      .append("g")
      .attr("transform", "translate(" + margin.left + "," + margin.top + ")");

    g.append("g")
      .call(d3.axisLeft(y).ticks(4).tickSizeOuter(0))
      .attr("font-size", "10")
      .attr("color", "#6b7280");

    var xTicks = days.length > 1 ? [days[0], days[days.length - 1]] : [days[0]];
    g.append("g")
      .attr("transform", "translate(0," + innerH + ")")
      .call(d3.axisBottom(x).tickValues(xTicks).tickSizeOuter(0))
      .attr("font-size", "10")
      .attr("color", "#6b7280");

    var line = d3
      .line()
      .x(function (_, i) { return x(days[i]); })
      .y(function (v) { return y(v); });

    series.forEach(function (s) {
      g.append("path")
        .datum(s.values)
        .attr("class", "linechart-line")
        .attr("fill", "none")
        .attr("stroke", s.color)
        .attr("stroke-width", 2)
        .attr("d", line);

      // Per-point dots carry a native <title> tooltip (day + series + value).
      g.append("g")
        .selectAll("circle")
        .data(s.values)
        .join("circle")
        .attr("cx", function (_, i) { return x(days[i]); })
        .attr("cy", function (v) { return y(v); })
        .attr("r", 2.5)
        .attr("fill", s.color)
        .append("title")
        .text(function (v, i) { return days[i] + " — " + s.name + ": " + v; });
    });

    var legend = g.append("g").attr("font-size", "10").attr("fill", "#14213d");
    series.forEach(function (s, i) {
      var lg = legend.append("g").attr("transform", "translate(" + i * 120 + ",-12)");
      lg.append("rect").attr("width", 10).attr("height", 10).attr("y", -9).attr("fill", s.color);
      lg.append("text").attr("x", 14).text(s.name);
    });
  }

  document.addEventListener("DOMContentLoaded", render);
  // htmx events bubble to document; re-render when the analytics wrapper swaps in.
  document.addEventListener("htmx:afterSwap", function (e) {
    if (e.target && e.target.id === "analytics-content") render();
  });
})();
