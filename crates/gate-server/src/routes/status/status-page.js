// 公开状态页的脚本（CSP 是 script-src 'self'，所以是同源外链，没有内联脚本）。
// - 主题：放在 <head> 里同步执行，先按上次的选择（或系统偏好）设好 data-theme，首屏不闪；存储键与首页 / 后台共用 owl-theme。
//   切换按钮在 DOM 就绪后再接线，切换时图标转 90 度。
// - 90 天色条按天从左往右长出来（Web Animations，不写 style 属性）。
// 系统开了「减少动态效果」时两种动画都不播。
// 跨页转场被浏览器跳过时它的 Promise 会被拒绝；接住，免得控制台报 Uncaught AbortError（与首页 theme-init.js 相同）。
(function () {
  function quiet(e) {
    var t = e && e.viewTransition;
    if (!t) return;
    [t.ready, t.finished, t.updateCallbackDone].forEach(function (p) {
      if (p && p.catch) p.catch(function () {});
    });
  }
  addEventListener("pageswap", quiet);
  addEventListener("pagereveal", quiet);
})();
(function () {
  var KEY = "owl-theme";
  var root = document.documentElement;
  var mq = window.matchMedia ? window.matchMedia("(prefers-color-scheme: dark)") : null;
  var reduce = window.matchMedia ? window.matchMedia("(prefers-reduced-motion: reduce)") : null;

  function still() {
    return !!(reduce && reduce.matches);
  }

  function saved() {
    try {
      var t = localStorage.getItem(KEY);
      return t === "light" || t === "dark" ? t : null;
    } catch (e) {
      return null;
    }
  }

  function system() {
    return mq && mq.matches ? "dark" : "light";
  }

  function apply(t) {
    root.setAttribute("data-theme", t);
    var b = document.getElementById("themeBtn");
    if (!b) return;
    var label = t === "dark" ? "切换到浅色主题" : "切换到深色主题";
    b.setAttribute("aria-label", label);
    b.setAttribute("title", label);
  }

  function spin(b) {
    if (still() || !b.animate) return;
    var icon = b.querySelector(root.getAttribute("data-theme") === "dark" ? ".ic-sun" : ".ic-moon");
    if (icon && icon.animate) {
      icon.animate(
        [
          { transform: "rotate(-90deg)", opacity: 0 },
          { transform: "rotate(0deg)", opacity: 1 },
        ],
        { duration: 300, easing: "ease-out" },
      );
    }
  }

  // 每个组件的色条：一天一根，从左往右依次从底边长起来
  function grow() {
    if (still()) return;
    var groups = document.querySelectorAll(".sp-bars svg");
    for (var k = 0; k < groups.length; k++) {
      var rects = groups[k].querySelectorAll("rect");
      for (var i = 0; i < rects.length; i++) {
        if (!rects[i].animate) return;
        rects[i].animate([{ transform: "scaleY(0)" }, { transform: "scaleY(1)" }], {
          duration: 300,
          delay: 150 + k * 120 + i * 6,
          easing: "ease-out",
          fill: "backwards",
        });
      }
    }
  }

  function wire() {
    apply(root.getAttribute("data-theme") || saved() || system());
    var b = document.getElementById("themeBtn");
    if (b) {
      b.addEventListener("click", function () {
        var t = root.getAttribute("data-theme") === "dark" ? "light" : "dark";
        try {
          localStorage.setItem(KEY, t);
        } catch (e) {}
        apply(t);
        spin(b);
      });
    }
    grow();
  }

  apply(saved() || system());
  if (mq && mq.addEventListener) {
    mq.addEventListener("change", function (e) {
      if (saved()) return;
      apply(e.matches ? "dark" : "light");
    });
  }
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", wire);
  else wire();
})();
