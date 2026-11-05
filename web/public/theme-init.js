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

