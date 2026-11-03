/* owl 下拉框补丁：nai key / base url 改名 + 追加 NEW API 登录账号块
 * 由 landing.html 里的 <script defer src="/_owl/ai-panel.js?v=N"> 载入。
 * 数据来自 https://gate.davidzhao.top/ai/api/account（验证当前 owl 登录身份和成员资格）。
 */
(function () {
  'use strict';
  if (window.__owlAiPanelVersion >= 17) return;
  window.__owlAiPanelVersion = 17;
  window.__owlAiPanel = true;

  var account = null;
  var pending = false;

  function credentialRow(label, value) {
    var wrap = document.createElement('div');
    wrap.style.cssText = 'display:grid;grid-template-columns:64px minmax(0,1fr) auto;align-items:center;column-gap:8px;margin-top:6px';
    var name = document.createElement('span');
    name.className = 'field__label';
    name.textContent = label;
    name.style.cssText = 'margin:0;white-space:nowrap;min-width:0';
    wrap.appendChild(name);
    var valueNode = document.createElement('code');
    valueNode.textContent = value;
    valueNode.style.cssText = 'display:block;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;font-size:12px;padding:2px 6px;border-radius:6px;background:rgba(127,127,127,.14);font-family:ui-monospace,SFMono-Regular,Menlo,monospace';
    wrap.appendChild(valueNode);
    var button = document.createElement('button');
    button.type = 'button';
    button.textContent = '复制';
    button.style.cssText = 'justify-self:end;font-size:12px;line-height:1;padding:4px 8px;border-radius:6px;cursor:pointer;border:1px solid rgba(127,127,127,.35);background:transparent;color:inherit';
    button.addEventListener('click', function (e) {
      e.preventDefault();
      e.stopPropagation();
      if (navigator.clipboard && window.isSecureContext) navigator.clipboard.writeText(value);
    });
    wrap.appendChild(button);
    return wrap;
  }

  function loadAccount(host) {
    if (account) return renderAccount(host);
    if (pending) return;
    pending = true;
    fetch('/ai/api/account', { credentials: 'include', cache: 'no-store' })
      .then(function (r) { if (!r.ok) throw new Error('account unavailable'); return r.json(); })
      .then(function (data) { account = data; renderAccount(host); })
      .catch(function () {})
      .then(function () { pending = false; });
  }

  function renderAccount(host) {
    if (!account || !host || host.querySelector('[data-owl-newapi]')) return;
    var slot = document.createElement('div');
    slot.setAttribute('data-owl-newapi', '');
    slot.style.cssText = 'margin-top:10px;padding-top:10px;border-top:1px solid rgba(127,127,127,.25)';
    var heading = document.createElement('a');
    heading.href = 'https://newapi.davidzhao.top';
    heading.target = '_blank';
    heading.rel = 'noopener';
    heading.textContent = 'NEW API ↗';
    heading.style.cssText = 'display:inline-block;font-size:13px;font-weight:600;text-decoration:underline;color:inherit';
    slot.appendChild(heading);
    slot.appendChild(credentialRow('用户名', account.username));
    slot.appendChild(credentialRow('密码', account.password));
    host.appendChild(slot);
  }

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

    var host = box ? box.parentNode : accountHeader;
    loadAccount(host);

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
