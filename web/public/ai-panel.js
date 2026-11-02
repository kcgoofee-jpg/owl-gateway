/* owl 下拉框补丁：nai key / base url 改名 + 追加 NEW API 账号块
 * 由 landing.html 里的 <script defer src="/_owl/ai-panel.js?v=N"> 载入。
 * 数据来自 https://gate.davidzhao.top/ai/api/account（验证当前 owl 登录身份和成员资格）。
 */
(function () {
  'use strict';
  if (window.__owlAiPanelVersion >= 13) return;
  window.__owlAiPanelVersion = 13;
  window.__owlAiPanel = true;

  var API = '/ai/api/account';
  var fetchedAt = 0;
  var acct = null;      // 已拿到的账号数据
  var failed = false;   // 拉取失败（未登录 / 500）
  var inflight = null;

  function getAccount() {
    if ((acct && Date.now() - fetchedAt < 30000) || failed) return Promise.resolve(acct);
    if (inflight) return inflight;
    inflight = fetch(API, { credentials: 'include', cache: 'no-store' })
      .then(function (r) {
        if (!r.ok) throw new Error('HTTP ' + r.status);
        return r.json();
      })
      .then(function (j) {
        acct = j;
        fetchedAt = Date.now();
        return j;
      })
      .catch(function () {
        failed = true;
        // 允许下一次打开菜单时重试
        setTimeout(function () { failed = false; }, 15000);
        return null;
      })
      .then(function (j) { inflight = null; return j; });
    return inflight;
  }

  function copy(text, btn) {
    var done = function () {
      var old = btn.textContent;
      btn.textContent = '已复制';
      setTimeout(function () { btn.textContent = old; }, 1200);
    };
    if (navigator.clipboard && window.isSecureContext) {
      navigator.clipboard.writeText(text).then(done, function () { fallback(); });
    } else {
      fallback();
    }
    function fallback() {
      var ta = document.createElement('textarea');
      ta.value = text;
      ta.style.cssText = 'position:fixed;top:-1000px;opacity:0';
      document.body.appendChild(ta);
      ta.select();
      try { document.execCommand('copy'); done(); } catch (e) {}
      document.body.removeChild(ta);
    }
  }

  // 一行：label + 值 + 复制按钮
  function row(label, value, opts) {
    opts = opts || {};
    var wrap = document.createElement('div');
    wrap.style.cssText =
      'display:grid;grid-template-columns:64px minmax(0,1fr) auto;' +
      'align-items:center;column-gap:8px;margin-top:6px';

    var lab = document.createElement('span');
    lab.className = 'field__label';
    lab.textContent = label;
    lab.style.cssText = 'margin:0;white-space:nowrap;min-width:0';
    wrap.appendChild(lab);

    var val = document.createElement('code');
    val.textContent = value;
    val.title = value;
    val.style.cssText =
      'display:block;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;' +
      'font-size:12px;padding:2px 6px;border-radius:6px;background:rgba(127,127,127,.14)' +
      (opts.mono === false ? '' : ';font-family:ui-monospace,SFMono-Regular,Menlo,monospace');
    wrap.appendChild(val);

    var btn = document.createElement('button');
    btn.type = 'button';
    btn.textContent = '复制';
    btn.style.cssText =
      'justify-self:end;font-size:12px;line-height:1;padding:4px 8px;border-radius:6px;cursor:pointer;' +
      'border:1px solid rgba(127,127,127,.35);background:transparent;color:inherit';
    btn.addEventListener('click', function (e) {
      e.preventDefault();
      e.stopPropagation();
      copy(value, btn);
    });
    wrap.appendChild(btn);

    return wrap;
  }

  // 把「我的 Key」「接入地址」两个标签改名（nai 侧），并向容器里补 NEW API 块
  function patch() {
    var box = document.querySelector('.lp-account__key');
    if (!box) return false;

    // 1) 标签改名
    var labels = box.querySelectorAll('.field__label');
    for (var i = 0; i < labels.length; i++) {
      var t = labels[i].textContent.trim();
      if (t === '我的 Key' || t === '我的Key') labels[i].textContent = 'nai key';
      else if (t === '接入地址') labels[i].textContent = 'base url';
    }

    // 2) 去掉「已登录」
    var walker = document.createTreeWalker(box.parentNode || box, NodeFilter.SHOW_TEXT, null);
    var n;
    while ((n = walker.nextNode())) {
      if (n.nodeValue && n.nodeValue.indexOf('已登录') !== -1) {
        n.nodeValue = n.nodeValue.replace(/\s*已登录\s*/g, '');
      }
    }

    // 3) NEW API 块
    var host = box.parentNode;
    if (!host) return false;
    var slot = host.querySelector('[data-owl-newapi]');
    if (!slot) {
      slot = document.createElement('div');
      slot.setAttribute('data-owl-newapi', '');
      slot.style.cssText = 'margin-top:10px;padding-top:10px;border-top:1px solid rgba(127,127,127,.25)';

      var head = document.createElement('a');
      head.href = 'https://newapi.davidzhao.top';
      head.target = '_blank';
      head.rel = 'noopener';
      head.textContent = 'NEW API ↗';
      head.style.cssText =
        'display:inline-block;font-size:13px;font-weight:600;text-decoration:underline;color:inherit';
      slot.appendChild(head);

      var body = document.createElement('div');
      body.setAttribute('data-owl-newapi-body', '');
      body.style.cssText = 'margin-top:4px';
      body.textContent = '读取账号中…';
      body.style.cssText += ';font-size:12px;opacity:.7';
      slot.appendChild(body);

      host.appendChild(slot);
    }

    return true;
  }

  function fill() {
    var slot = document.querySelector('[data-owl-newapi]');
    if (!slot) return;
    var body = slot.querySelector('[data-owl-newapi-body]');
    if (!body) return;

    getAccount().then(function (a) {
      if (!a) {
        body.textContent = '请登录 owl 并确认已领取成员 Key；若账号不可用，请联系管理员。';
        body.style.opacity = '.7';
        body.setAttribute('data-filled', '1');
        return;
      }
      var state = a.username + '|' + a.balance_usd + '|' + a.checked_in_today;
      if (body.getAttribute('data-filled') === state) return;
      body.innerHTML = '';
      body.style.opacity = '1';
      body.appendChild(row('用户名', a.username));
      body.appendChild(row('密码', a.password));
      if (a.key) body.appendChild(row('api key', a.key));
      if (a.base) body.appendChild(row('base url', a.base, { mono: false }));
      var info = document.createElement('div');
      info.style.cssText = 'margin-top:8px;font-size:12px;line-height:1.6';
      info.textContent = '余额：$' + Number(a.balance_usd || 0).toFixed(4) + ' · ' + (a.checked_in_today ? '今日已签到' : '今日未签到');
      body.appendChild(info);
      var rule = document.createElement('div');
      rule.style.cssText = 'font-size:12px;line-height:1.6';
      rule.textContent = a.checkin_reward || '每日签到随机 $2–$5，每个账户每天限领一次；余额可累计';
      body.appendChild(rule);
      var signin = document.createElement('a');
      signin.href = 'https://newapi.davidzhao.top/profile';
      signin.target = '_blank';
      signin.rel = 'noopener';
      signin.textContent = '前往 New API 签到（需用上面的账号登录）';
      signin.style.cssText = 'display:block;margin-top:4px;font-size:12px;text-decoration:underline';
      body.appendChild(signin);
      body.setAttribute('data-filled', state);
    });
  }

  function tick() {
    if (!document.querySelector('.lp-account__key')) return;
    if (patch()) fill();
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
        getAccount();
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
