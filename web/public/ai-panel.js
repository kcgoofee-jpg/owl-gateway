/* owl 下拉框补丁：nai key / base url 改名 + 追加 NEW API 登录账号块
 * 由 landing.html 里的 <script defer src="/_owl/ai-panel.js?v=N"> 载入。
 * 数据来自 https://gate.davidzhao.top/ai/api/account（验证当前 owl 登录身份和成员资格）。
 */
(function () {
  'use strict';
  if (window.__owlAiPanelVersion >= 16) return;
  window.__owlAiPanelVersion = 16;
  window.__owlAiPanel = true;

  // 把「我的 Key」「接入地址」两个标签改名（nai 侧）
  function patch() {
    var box = document.querySelector('.lp-account__key');
    var accountHeader = document.querySelector('.menu:has(.lp-avatar) .menu__header');
    if (!box && !accountHeader) return false;

    // Remove the block inserted by older cached copies of this script.
    var oldNewApi = document.querySelectorAll('[data-owl-newapi]');
    for (var j = 0; j < oldNewApi.length; j++) oldNewApi[j].remove();

    // 1) 标签改名
    var labels = box ? box.querySelectorAll('.field__label') : [];
    for (var i = 0; i < labels.length; i++) {
      var t = labels[i].textContent.trim();
      if (t === '我的 Key' || t === '我的Key') labels[i].textContent = 'nai key';
      else if (t === '接入地址') labels[i].textContent = 'base url';
    }

    // 2) 去掉「已登录」
    var walker = document.createTreeWalker((box ? box.parentNode : accountHeader), NodeFilter.SHOW_TEXT, null);
    var n;
    while ((n = walker.nextNode())) {
      if (n.nodeValue && n.nodeValue.indexOf('已登录') !== -1) {
        n.nodeValue = n.nodeValue.replace(/\s*已登录\s*/g, '');
      }
    }

    return true;
  }

  function tick() {
    if (!document.querySelector('.menu:has(.lp-avatar) .menu__header')) return;
    patch();
  }

  var scheduled = false;
  function schedule() {
    if (scheduled) return;
    scheduled = true;
    requestAnimationFrame(function () {
      scheduled = false;
      tick();
    });
  }

  function boot() {
    // owl 下拉框是懒渲染的：菜单第一次打开时才出现 .lp-account__key
    new MutationObserver(schedule).observe(document.documentElement, {
      childList: true,
      subtree: true,
    });
    // React 重渲染会还原 textContent，低频兜底重打
    setInterval(tick, 1500);
    // 打开菜单就预取账号，减少等待
    document.addEventListener('click', function (e) {
      var t = e.target;
      if (t && t.closest && t.closest('[class*="account"],[class*="user"],[class*="avatar"],[class*="profile"]')) {
        setTimeout(tick, 60);
        setTimeout(tick, 400);
      }
    }, true);
    schedule();
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', boot);
  } else {
    boot();
  }
})();
