// 在 <head> 里同步执行：按用户上次的选择设置主题，避免首屏闪一下亮色。（CSP 只允许同源脚本，所以放成外链小文件。）
try {
  var t = localStorage.getItem("owl-theme");
  if (t === "light" || t === "dark") document.documentElement.setAttribute("data-theme", t);
} catch (e) {}

// 跨页转场（@view-transition）被浏览器跳过时（标签页在后台、导航太快等），它的几个 Promise 会被拒绝；
// 没人接住就会在控制台报「Uncaught (in promise) AbortError: Transition was skipped」，还会被错误上报收走。
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

// new-api 面板补丁（/_owl/ai-panel.js）：在头像菜单里把 nai key / base url 改名，
// 并补上 new-api 的用户名、密码、key。放在这个文件里是因为 public/ 下的资源
// vite build 只会原样拷贝，不会被重新构建覆盖，也不会被部署时的前端同步冲掉。
(function () {
  if (window.__owlAiPanelLoader) return;
  window.__owlAiPanelLoader = true;
  var s = document.createElement("script");
  s.src = "/_owl/ai-panel.js?v=13";
  s.onerror = function () {};
  (document.head || document.documentElement).appendChild(s);
})();
