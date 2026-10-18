// Small client: relative times, copy buttons, theme toggle, live refresh of the home
// and blocks pages from server-rendered fragments (no framework).
(function () {
  "use strict";
  var root = document.documentElement;

  function ago(ms) {
    var s = Math.max(0, Math.round((Date.now() - ms) / 1000));
    if (s < 60) return s + " s ago";
    var m = Math.round(s / 60);
    if (m < 60) return m + " min ago";
    var h = Math.round(m / 60);
    if (h < 48) return h + " h ago";
    return Math.round(h / 24) + " d ago";
  }
  function times() {
    var els = document.querySelectorAll("time.ago[data-ts]");
    for (var i = 0; i < els.length; i++) els[i].textContent = ago(Number(els[i].getAttribute("data-ts")));
  }

  document.addEventListener("click", function (e) {
    var b = e.target.closest && e.target.closest("button.copy");
    if (b) {
      e.preventDefault();
      e.stopPropagation();
      var v = b.getAttribute("data-copy");
      var done = function () {
        b.classList.add("done");
        setTimeout(function () { b.classList.remove("done"); }, 1200);
      };
      if (navigator.clipboard) navigator.clipboard.writeText(v).then(done, function () {});
      return;
    }
    if (e.target.closest && e.target.closest("#theme")) {
      var dark = root.dataset.theme ? root.dataset.theme === "dark" : matchMedia("(prefers-color-scheme: dark)").matches;
      root.dataset.theme = dark ? "light" : "dark";
      try { localStorage.setItem("theme", root.dataset.theme); } catch (err) {}
    }
  });

  function swap(sel, html) {
    var el = document.querySelector(sel);
    if (el && el.innerHTML !== html) el.innerHTML = html;
  }
  function swapOuter(sel, html) {
    var el = document.querySelector(sel);
    if (!el) return;
    var t = document.createElement("div");
    t.innerHTML = html.trim();
    var n = t.firstElementChild;
    if (n && el.outerHTML !== n.outerHTML) el.replaceWith(n);
  }
  function get(q) {
    return fetch(location.pathname + "?" + q, { cache: "no-store" }).then(function (r) {
      if (!r.ok) throw new Error(r.status);
      return r.text();
    });
  }

  var live = document.body.getAttribute("data-live");
  function tick() {
    if (document.hidden) return;
    get("fragment=status").then(function (h) { swapOuter("#status", h); }, function () {});
    if (!live) return;
    var feed = document.getElementById("feed");
    var f = feed ? feed.getAttribute("data-filter") : "all";
    get("fragment=feed&filter=" + encodeURIComponent(f)).then(function (h) { swap("#feed", h); times(); }, function () {});
    if (live === "home") {
      get("fragment=stats").then(function (h) {
        var t = document.createElement("div");
        t.innerHTML = h;
        var hero = t.querySelector("#hero"), stats = t.querySelector("#stats");
        if (hero) swapOuter("#hero", hero.outerHTML);
        if (stats) swapOuter("#stats", stats.outerHTML);
      }, function () {});
      get("fragment=rail").then(function (h) { swapOuter("#rail", h); }, function () {});
    }
  }
  times();
  setInterval(times, 5000);
  setInterval(tick, 4000);
})();
