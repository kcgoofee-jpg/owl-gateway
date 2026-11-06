/* owl ⇄ NovelAI 生图页桥：把官网前端的 API 调用改到本站网关。
   只做三件事：改 base、带 Key、隐藏账号横幅。 */
(function () {
  window.__BRIDGE_VERSION__ = "2026-11-06-share-settings-v4";
  var CFG = window.__NAI_BRIDGE__ || {};
  var GATE = CFG.gate || "";
  var KEY = CFG.key || "";
  var HOSTS = ["image.novelai.net", "api.novelai.net", "text.novelai.net", "explore.novelai.net"];

  function map(url) {
    try {
      var u = new URL(url, location.href);
      if (HOSTS.indexOf(u.host) >= 0) return GATE + u.pathname + u.search;
    } catch (e) {}
    return url;
  }

  // 同源且是本站接口的请求也要带 Key（桥接改写过或前端直接用相对路径都一样）
  function needsAuth(url) {
    try {
      var u = new URL(url, location.href);
      if (u.origin !== location.origin) return false;
      return /^\/(ai|user|v\d)\//.test(u.pathname);
    } catch (e) {
      return false;
    }
  }

  // 官网前端会从 auth_token 里解 JWT 取账号 id；本站的 Key 不是 JWT，这里包一层
  function asJwt(token) {
    if (/^[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+$/.test(token)) return token;
    var payload = btoa(JSON.stringify({ id: "owl-" + token.slice(-8), iat: Math.floor(Date.now() / 1000) }));
    return "owl." + payload + ".owl";
  }

  /* ---- 排队反馈（2026-11-01）：本站队列满时出图请求会挂 60–120 秒，官网前端在这段时间
     不显示任何东西，用户以为卡死就刷新页面——刷新等于断开，前面白等。这里补一条计时提示，
     并在 429 / 断连时换成一句人话（等多久看 Retry-After）。只加显示，不动请求。 ---- */
  var QW = { n: 0, since: 0, timer: null, el: null };
  var QW_PAT = /\/ai\/generate-image(?:-stream)?(?:\?|$)/;

  function qwBox() {
    if (QW.el && QW.el.isConnected) return QW.el;
    var el = document.createElement("div");
    el.setAttribute("data-owl-queue", "1");
    el.style.cssText = [
      "position:fixed", "left:50%", "bottom:32px", "transform:translateX(-50%)",
      "padding:10px 16px", "border-radius:999px", "z-index:2147482999",
      "font:600 13px/1.4 system-ui,-apple-system,'Segoe UI',sans-serif",
      "color:#fff", "background:#2f2a56", "border:1px solid #5b52a8",
      "box-shadow:0 6px 22px rgba(0,0,0,.4)", "pointer-events:none", "white-space:nowrap",
    ].join(";");
    (document.body || document.documentElement).appendChild(el);
    QW.el = el;
    return el;
  }

  function qwStart() {
    QW.n++;
    if (QW.timer) return true;
    QW.since = Date.now();
    var el = qwBox();
    function tick() {
      var s = Math.round((Date.now() - QW.since) / 1000);
      el.textContent = s < 3
        ? "正在出图…"
        : "排队中 " + s + " 秒 · 别刷新，刷新就白等了";
    }
    tick();
    QW.timer = setInterval(tick, 1000);
    return true;
  }

  function qwStop() {
    QW.n = Math.max(0, QW.n - 1);
    if (QW.n > 0) return;
    if (QW.timer) { clearInterval(QW.timer); QW.timer = null; }
    if (QW.el) { QW.el.remove(); QW.el = null; }
  }

  function qwFail(r, netErr) {
    var wait = 15;
    try {
      var ra = r && r.headers && r.headers.get("retry-after");
      if (ra && /^\d+$/.test(ra) && +ra > 0 && +ra <= 300) wait = +ra;
    } catch (e) {}
    var text = netErr
      ? "连接断了 · 等 10 秒再点一次（这次没扣费）"
      : "这轮没排上 · 约 " + wait + " 秒后重试（没扣费）";
    var el = qwBox();
    el.style.background = "#7a2c2c";
    el.style.borderColor = "#d16060";
    el.textContent = text;
    setTimeout(function () {
      if (el.isConnected) el.remove();
      if (QW.el === el) QW.el = null;
    }, 6000);
  }

  var of = window.fetch;
  window.fetch = function (input, init) {
    var url = typeof input === "string" ? input : (input && input.url) || "";
    var mapped = map(url);
    if (mapped === url && !(KEY && needsAuth(url))) return of.apply(window, arguments);
    init = init || {};
    var headers = new Headers(init.headers || (input && input.headers) || {});
    if (KEY) headers.set("authorization", "Bearer " + KEY);
    // 让网关的请求日志能区分「本站生图页」和第三方客户端
    headers.set("x-owl-client", "workbench");
    init.headers = headers;
    // 同源请求必须带 Cookie：维护模式按网页会话判定站长，丢 Cookie 会被当成访客挡掉
    init.credentials = "same-origin";
    window.__bridgeLog = window.__bridgeLog || [];
    var watching = QW_PAT.test(mapped) ? qwStart() : false;
    return of.call(window, mapped, init).then(function (r) {
      window.__bridgeLog.push(mapped + " -> " + r.status);
      if (watching) { qwStop(); if (r.status === 429) qwFail(r); }
      return r;
    }).catch(function (e) {
      window.__bridgeLog.push(mapped + " -> ERR " + e);
      if (watching) { qwStop(); qwFail(null, true); }
      throw e;
    });
  };

  var XO = XMLHttpRequest.prototype.open;
  var XS = XMLHttpRequest.prototype.setRequestHeader;
  XMLHttpRequest.prototype.open = function (m, u) {
    var mapped = map(u);
    this.__bridgeAuth = mapped !== u;
    // 走本站接口的 XHR 也带来源标记：服务端的自动备份按 UA 前缀认「网页工作台」。
    this.__bridgeOwn = mapped !== u || (KEY && needsAuth(u));
    this.__bridgeHeaders = [];
    return XO.call(this, m, mapped, ...Array.prototype.slice.call(arguments, 2));
  };
  XMLHttpRequest.prototype.setRequestHeader = function (k, v) {
    if (this.__bridgeAuth && String(k).toLowerCase() === "authorization") return;
    if (this.__bridgeAuth) this.__bridgeHeaders.push([k, v]);
    return XS.call(this, k, v);
  };
  var OS = XMLHttpRequest.prototype.send;
  XMLHttpRequest.prototype.send = function (body) {
    if (this.__bridgeOwn) {
      try {
        XS.call(this, "x-owl-client", "workbench");
      } catch (e) {}
    }
    if (this.__bridgeAuth && KEY) {
      try {
        XS.call(this, "authorization", "Bearer " + KEY);
      } catch (e) {}
    }
    return OS.call(this, body);
  };

  window.__BRIDGE_SESSION_STEP__ = "reached";
  // 登录态：官网前端读 sessionStorage.session（优先）或 localStorage.session，形如 {auth_token, encryption_key}
  try {
    if (KEY) {
      var ek = localStorage.getItem("owl-ek");
      if (!ek) {
        var rawBytes = new Uint8Array(32);
        crypto.getRandomValues(rawBytes);
        ek = btoa(String.fromCharCode.apply(null, rawBytes));
        localStorage.setItem("owl-ek", ek);
      }
      var want = JSON.stringify({ auth_token: asJwt(KEY), encryption_key: ek });
      var cur = localStorage.getItem("session");
      var okLocal = false;
      try { okLocal = JSON.parse(cur || "null")?.auth_token === asJwt(KEY); } catch (e) {}
      if (!okLocal) localStorage.setItem("session", want);
      var curS = sessionStorage.getItem("session");
      var okSession = false;
      try { okSession = JSON.parse(curS || "null")?.auth_token === asJwt(KEY); } catch (e) {}
      if (!okSession) sessionStorage.setItem("session", want);
    }
  } catch (e) {}

  // 账号横幅（注册/续费/试用）一律不显示：官网在未登录或欠费时会挂一条顶部条带，
  // 这里按文案找到它并连同占位一起收起（纯 CSS 选不到文本，只能用观察器）。
  var HIDE_TEXTS = [
    "Create an account and start for free",
    "Your subscription renewal payment has failed",
    "Confirm your email",
    "We’ve updated our Terms of Service",
    "We've updated our Terms of Service",
  ];
  function hideBanners() {
    var walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
    var hits = [];
    while (walker.nextNode()) {
      var t = (walker.currentNode.nodeValue || "").trim();
      for (var i = 0; i < HIDE_TEXTS.length; i++) {
        if (t.indexOf(HIDE_TEXTS[i]) >= 0) { hits.push(walker.currentNode.parentElement); break; }
      }
    }
    for (var j = 0; j < hits.length; j++) {
      var el = hits[j];
      // 往上找到「整条横幅」：第一个铺满宽度、且不包含主要内容的容器
      for (var k = 0; k < 4 && el && el !== document.body; k++) {
        var r = el.getBoundingClientRect();
        if (r.width > window.innerWidth * 0.6 && r.height > 16 && r.height < 120) {
          el.style.setProperty("display", "none", "important");
          el.setAttribute("data-owl-hidden", "account-banner");
          break;
        }
        el = el.parentElement;
      }
    }
  }
  /* ---- 菜单里的「NovelAI Discord」改成我们自己的服务器（官方点击是 window.open("https://discord.gg/novelai")） ---- */
  var OWL_DISCORD = CFG.discord || "";
  var OLD_LABEL = "NovelAI Discord";
  var NEW_LABEL = "OWL Discord";

  function renameDiscord() {
    if (!document.body) return;
    var walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
    var hit = [];
    while (walker.nextNode()) {
      var t = walker.currentNode.nodeValue || "";
      if (t.indexOf(OLD_LABEL) >= 0) hit.push(walker.currentNode);
    }
    for (var i = 0; i < hit.length; i++) hit[i].nodeValue = hit[i].nodeValue.split(OLD_LABEL).join(NEW_LABEL);
  }

  function openOwlDiscord(ev) {
    if (!OWL_DISCORD) return;
    var el = ev.target;
    while (el && el !== document.body) {
      var text = (el.textContent || "").trim();
      if (text === NEW_LABEL || text === OLD_LABEL) {
        ev.preventDefault();
        ev.stopPropagation();
        window.open(OWL_DISCORD, "_blank", "noopener");
        return;
      }
      el = el.parentElement;
    }
  }

  function watchBanners() {
    hideBanners();
    renameDiscord();
    var mo = new MutationObserver(function () {
      hideBanners();
      renameDiscord();
    });
    mo.observe(document.documentElement, { childList: true, subtree: true, characterData: true });
  }
  if (document.body) watchBanners();
  else document.addEventListener("DOMContentLoaded", watchBanners);

  /* ---- 设备指纹 + 浏览器标记：官网页面不会调 /public/device-fp，
     于是日志里的「浏览器标记 / 浏览器标识」一直是空的，反滥用少掉最重要的一条线索。
     这里用和旧工作台完全相同的分项（ua/屏幕/时区/硬件/GPU/Canvas/WebGL/字体）上报，
     服务端据此算设备指纹，并种下长期 owl_bid Cookie（清 Cookie 也还有 localStorage 兜底）。 ---- */
  function sha16(text) {
    if (!text || !window.crypto || !crypto.subtle) return Promise.resolve("");
    return crypto.subtle.digest("SHA-256", new TextEncoder().encode(text)).then(function (buf) {
      return Array.prototype.map
        .call(new Uint8Array(buf).slice(0, 8), function (b) {
          return ("0" + b.toString(16)).slice(-2);
        })
        .join("");
    }).catch(function () { return ""; });
  }

  function webglRaw() {
    try {
      var c = document.createElement("canvas");
      var gl = c.getContext("webgl") || c.getContext("experimental-webgl");
      if (!gl) return "";
      var dbg = gl.getExtension("WEBGL_debug_renderer_info");
      var vendor = dbg ? gl.getParameter(dbg.UNMASKED_VENDOR_WEBGL) : "";
      var renderer = dbg ? gl.getParameter(dbg.UNMASKED_RENDERER_WEBGL) : "";
      var params = [
        gl.getParameter(gl.MAX_TEXTURE_SIZE),
        gl.getParameter(gl.MAX_RENDERBUFFER_SIZE),
        gl.getParameter(gl.MAX_VERTEX_ATTRIBS),
        (gl.getSupportedExtensions() || []).join(","),
      ].join("~");
      return vendor + "~" + renderer + "~" + params;
    } catch (e) { return ""; }
  }

  function canvasRaw() {
    try {
      var c = document.createElement("canvas");
      c.width = 240; c.height = 60;
      var ctx = c.getContext("2d");
      ctx.textBaseline = "top";
      ctx.font = "16px Arial";
      ctx.fillStyle = "#f60"; ctx.fillRect(0, 0, 100, 20);
      ctx.fillStyle = "#069"; ctx.fillText("猫头鹰 owl 0123456789", 2, 15);
      ctx.fillStyle = "rgba(102,204,0,0.7)"; ctx.fillText("猫头鹰 owl 0123456789", 4, 30);
      ctx.beginPath(); ctx.arc(200, 30, 18, 0, Math.PI * 1.5); ctx.fill();
      return c.toDataURL();
    } catch (e) { return ""; }
  }

  function readBid() {
    try {
      var v = localStorage.getItem("owl_bid") || "";
      return /^[0-9a-f]{32}$/.test(v) ? v : "";
    } catch (e) { return ""; }
  }

  function sendDeviceFp() {
    var nav = navigator;
    var glRaw = webglRaw();
    var glParts = glRaw.split("~");
    var data = {
      ua: nav.userAgent,
      lang: nav.language,
      langs: (nav.languages || []).join(","),
      screen: screen.width + "x" + screen.height + "x" + screen.colorDepth,
      tz: new Date().getTimezoneOffset(),
      hw: nav.hardwareConcurrency || "",
      mem: nav.deviceMemory || "",
      platform: nav.platform || "",
      touch: nav.maxTouchPoints || "",
      gpuVendor: glParts[0] || "",
      gpuRenderer: glParts[1] || "",
    };
    Promise.all([sha16(canvasRaw()), sha16(glRaw)]).then(function (h) {
      data.canvas = h[0];
      data.webgl = h[1];
      var body = { data: data };
      var bid = readBid();
      if (bid) body.bid = bid;
      return fetch("/public/device-fp", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(body),
        credentials: "same-origin",
      });
    }).then(function (r) {
      return r.ok ? r.json() : null;
    }).then(function (res) {
      if (res && typeof res.bid === "string" && /^[0-9a-f]{32}$/.test(res.bid)) {
        try { localStorage.setItem("owl_bid", res.bid); } catch (e) {}
      }
    }).catch(function () {});
  }
  try {
    if (window.requestIdleCallback) requestIdleCallback(sendDeviceFp);
    else setTimeout(sendDeviceFp, 800);
  } catch (e) {}

  /* ---- 上传到 Explore：官网那个按钮只是 <a href="/explore/register">，我们镜像里没有 Explore 站。
     改成把画布上这张原图（webp/png 原始字节，官方元数据都在）交给网关，由机器人发到 Discord。
     分享弹窗用工作台自己的主题（NovelAI Dark token，见下），不再套首页的紫灰配色；
     自己写 class（owl-*），不依赖打包产物里的 sc-* 哈希类或 Turbopack 私有模块。 ---- */

  // NovelAI Dark：官网 image.html 里 @font-face 自托管了 Source Sans Pro / Eczar，弹窗直接继承。
  var T = {
    bg0: "#0E0F21", // 弹窗面板
    bg1: "#13152C", // 页面底色
    bg2: "#191B31", // 输入框 / 次级表面
    bg3: "#22253F", // 悬停 / 抬升
    border: "#283059",
    text: "#FFFFFF",
    dim: "rgba(225,228,255,.78)",
    heading: "#F5F3C2",
    accent: "#EC55A7", // 复选框等交互强调色（官网 Dark 主题最常用的交互色）
    warning: "#FF7878",
    font: "'Source Sans Pro','Noto Sans SC',-apple-system,'Segoe UI',sans-serif",
    headingFont: "Eczar,'Source Sans Pro',serif",
  };

  // 弹窗公共样式只注入一次；选择器都带 owl- 前缀，不碰官网样式。
  function ensureDialogStyles() {
    if (document.getElementById("owl-share-styles")) return;
    var style = document.createElement("style");
    style.id = "owl-share-styles";
    style.textContent =
      ".owl-share-dialog{box-sizing:border-box;width:min(480px,calc(100vw - 40px));padding:24px;border:1px solid " +
      T.border + ";border-radius:14px;background:" + T.bg0 + ";color:" + T.text +
      ";font:14px/1.65 " + T.font + ";box-shadow:0 20px 70px rgba(0,0,0,.6)}" +
      ".owl-share-dialog::backdrop{background:rgba(6,7,20,.72)}" +
      ".owl-share-dialog h2{margin:0 0 12px;font:600 19px/1.4 " + T.headingFont + ";color:" + T.heading + "}" +
      ".owl-share-dialog p{margin:0}" +
      ".owl-share-dialog .owl-help{color:" + T.dim + ";font-size:13px}" +
      ".owl-share-dialog .owl-warning{color:" + T.warning + "}" +
      ".owl-share-dialog label.owl-row{display:flex;align-items:center;gap:10px;cursor:pointer}" +
      ".owl-share-dialog input[type=checkbox]{width:17px;height:17px;flex:none;accent-color:" + T.accent + ";cursor:pointer}" +
      ".owl-share-dialog select,.owl-share-dialog input[type=text]{display:block;box-sizing:border-box;width:100%;margin:8px 0 0;padding:9px 10px;border:1px solid " +
      T.border + ";border-radius:8px;background:" + T.bg2 + ";color:" + T.text + ";font:inherit}" +
      ".owl-share-dialog select:hover{border-color:" + T.bg3 + "}" +
      ".owl-share-dialog button{font:600 14px/1.2 " + T.font + ";padding:10px 14px;border-radius:8px;cursor:pointer}" +
      ".owl-btn-primary{background:" + T.heading + ";color:" + T.bg0 + ";border:1px solid " + T.heading + "}" +
      ".owl-btn-primary:disabled{opacity:.55;cursor:default}" +
      ".owl-btn-ghost{background:transparent;color:" + T.text + ";border:1px solid " + T.border + "}" +
      ".owl-btn-ghost:hover{background:" + T.bg2 + "}" +
      ".owl-btn-mini{padding:5px 9px;border-radius:6px;font-size:13px}" +
      ".owl-share-dialog button:focus-visible,.owl-share-dialog input:focus-visible,.owl-share-dialog select:focus-visible{outline:2px solid " +
      T.heading + ";outline-offset:2px}" +
      ".owl-share-dialog .owl-cap{font:600 13px/1.4 " + T.font + ";color:" + T.dim + ";margin:0 0 8px}" +
      ".owl-share-dialog .owl-cap.owl-block{margin-top:22px;padding-top:16px;border-top:1px solid " + T.border + "}" +
      ".owl-share-dialog .owl-actions{display:flex;align-items:center;gap:10px;margin-top:24px}" +
      ".owl-share-dialog .owl-spacer{flex:1 1 auto}" +
      ".owl-share-dialog .owl-link{background:none;border:0;padding:2px;font:600 13px/1.2 " + T.font +
      ";color:rgba(225,228,255,.86);text-decoration:underline;text-underline-offset:3px;cursor:pointer}" +
      ".owl-share-dialog .owl-link:hover{color:" + T.text + "}";
    document.head.appendChild(style);
  }

  // 可复用的模态骨架：主题样式 + 焦点还原 + Esc/关闭兜底；内容由 build 填充，
  // build 里拿到 handle（finish 结束弹窗并 resolve）。resolve(null) 一律表示取消。
  function openOwlDialog(build) {
    return new Promise(function (resolve) {
      ensureDialogStyles();
      var previous = document.activeElement;
      var dialog = document.createElement("dialog");
      dialog.className = "owl-share-dialog";
      var settled = false;
      var cleanups = [];
      function finish(value) {
        if (settled) return;
        settled = true;
        for (var i = 0; i < cleanups.length; i++) { try { cleanups[i](); } catch (e) {} }
        dialog.remove();
        if (previous && previous.isConnected && previous.focus) previous.focus();
        resolve(value);
      }
      var handle = {
        dialog: dialog,
        finish: finish,
        onCleanup: function (fn) { cleanups.push(fn); },
      };
      build(handle);
      dialog.addEventListener("cancel", function (ev) { ev.preventDefault(); finish(null); });
      dialog.addEventListener("close", function () { finish(null); });
      document.body.appendChild(dialog);
      try {
        dialog.showModal();
      } catch (e) {
        finish(null);
        toast("当前浏览器无法打开分享弹窗，请更新浏览器后再试。", true);
      }
    });
  }

  function dialogButton(text, primary, mini) {
    var b = document.createElement("button");
    b.type = "button";
    b.textContent = text;
    b.className = (primary ? "owl-btn-primary" : "owl-btn-ghost") + (mini ? " owl-btn-mini" : "");
    return b;
  }

  function dialogCheckbox(labelText) {
    var label = document.createElement("label");
    label.className = "owl-row";
    var box = document.createElement("input");
    box.type = "checkbox";
    label.appendChild(box);
    label.appendChild(document.createTextNode(labelText));
    return { label: label, box: box };
  }

  /* ---- 记住的分享选择：存服务端（按登录成员隔离），本地不落盘。
     GET 返回 remembered（当前仍可用，可免弹窗直接分享）和 last（最近一次保存的原始选择，
     绑定失效后仍用于回填弹窗里「包含元数据」的勾选，尊重成员明确记住过的偏好）。 ---- */
  function fetchShareState() {
    return Promise.all([
      fetch("/public/me/share-preference", { credentials: "same-origin", cache: "no-store" })
        .then(function (r) { return r.ok ? r.json() : null; })
        .catch(function () { return null; }),
      fetch("/public/me/share-destination", { credentials: "same-origin", cache: "no-store" })
        .then(function (r) { return r.ok ? r.json() : null; })
        .catch(function () { return null; }),
    ]);
  }

  // 返回 true/false 而不是抛错：调用方要据此回滚勾选框。
  function savePreference(choice, quiet) {
    return fetch("/public/me/share-preference", {
      method: "PUT",
      credentials: "same-origin",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        destination: choice.destination,
        include_metadata: choice.includeMetadata,
        binding_id: choice.destination === "personal" ? choice.bindingId : null,
      }),
    }).then(function (r) {
      if (!r.ok) throw new Error("save failed");
      if (!quiet) toast("已记住，下次分享直接发送。");
      return true;
    }).catch(function () {
      toast("这次选择没能记住，下次会再询问。", true);
      return false;
    });
  }

  function clearPreference() {
    return fetch("/public/me/share-preference", { method: "DELETE", credentials: "same-origin" })
      .then(function (r) {
        if (!r.ok) throw new Error("clear failed");
        return true;
      })
      .catch(function () {
        toast("清除失败，请稍后再试。", true);
        return false;
      });
  }

  /* ---- 分享设置：分享弹窗的二级界面。入口在分享弹窗里，不再往官网工具栏塞按钮。 ---- */
  function buildShareSettingsPanel(onBack) {
    var root = document.createElement("div");
    var title = document.createElement("h2");
    title.id = "owl-share-settings-title";
    title.textContent = "分享设置";
    root.appendChild(title);

    var destLabel = document.createElement("label");
    destLabel.style.cssText = "display:block;font-size:13px;color:" + T.dim + ";";
    destLabel.textContent = "分享到";
    var select = document.createElement("select");
    var owlOption = document.createElement("option");
    owlOption.value = "owl";
    owlOption.textContent = "OWL 公共频道";
    select.appendChild(owlOption);
    destLabel.appendChild(select);

    var metaRow = dialogCheckbox("附带生成参数（提示词、种子）");
    metaRow.label.style.marginTop = "14px";
    var rememberRow = dialogCheckbox("记住分享选择");
    rememberRow.label.style.marginTop = "12px";

    var connCap = document.createElement("p");
    connCap.className = "owl-cap owl-block";
    connCap.textContent = "个人频道";
    var connLine = document.createElement("p");
    connLine.className = "owl-help";
    var connText = document.createElement("span");
    var channelLink = document.createElement("a");
    channelLink.textContent = "在 Discord 中查看";
    channelLink.target = "_blank";
    channelLink.rel = "noopener noreferrer";
    channelLink.style.cssText = "color:" + T.heading + ";display:none;margin-left:8px;";
    connLine.append(connText, channelLink);
    var connActions = document.createElement("div");
    connActions.style.cssText = "display:flex;gap:8px;margin-top:10px;flex-wrap:wrap;";
    var connect = dialogButton("连接个人频道", false, true);
    var change = dialogButton("更换频道", false, true);
    var disconnect = dialogButton("断开", false, true);
    change.style.display = "none";
    disconnect.style.display = "none";
    connActions.append(connect, change, disconnect);

    var backupCap = document.createElement("p");
    backupCap.className = "owl-cap owl-block";
    backupCap.textContent = "自动备份";
    var backupRow = dialogCheckbox("出图后自动转发到个人频道");

    var actions = document.createElement("div");
    actions.className = "owl-actions";
    var back = dialogButton("返回", false, true);
    actions.append(back);

    root.append(
      destLabel, metaRow.label, rememberRow.label,
      connCap, connLine, connActions,
      backupCap, backupRow.label,
      actions
    );

    var binding = null;

    function save(quiet) {
      return savePreference({
        destination: select.value,
        includeMetadata: metaRow.box.checked,
        bindingId: binding && binding.binding_id,
      }, quiet);
    }

    function refresh() {
      fetchShareState().then(function (s) {
        var pref = s && s[0], dest = s && s[1];
        binding = dest && dest.connected ? dest : null;
        var remembered = pref && pref.remembered;
        var last = pref && pref.last;
        var personalOption = select.querySelector("option[value=personal]");
        if (binding && !personalOption) {
          personalOption = document.createElement("option");
          personalOption.value = "personal";
          personalOption.textContent = "我的个人频道";
          select.appendChild(personalOption);
        } else if (!binding && personalOption) {
          personalOption.remove();
        }
        if (remembered) {
          select.value = remembered.destination;
          metaRow.box.checked = remembered.include_metadata;
        } else {
          select.value = binding ? "personal" : "owl";
          metaRow.box.checked = last ? last.include_metadata : true;
        }
        rememberRow.box.checked = !!remembered;
        connText.textContent = binding ? "已连接" : "未连接";
        channelLink.style.display = binding ? "inline" : "none";
        if (binding) channelLink.href = binding.channel_url;
        connect.style.display = binding ? "none" : "inline-block";
        change.style.display = binding ? "inline-block" : "none";
        disconnect.style.display = binding ? "inline-block" : "none";
        fetch("/public/me/backup-settings", { credentials: "same-origin", cache: "no-store" })
          .then(function (r) { return r.ok ? r.json() : null; })
          .catch(function () { return null; })
          .then(function (b) {
            backupRow.box.disabled = !binding;
            backupRow.box.checked = !!(b && b.enabled && binding);
            if (!binding) backupRow.box.checked = false;
          });
      });
    }

    rememberRow.box.addEventListener("change", function () {
      rememberRow.box.disabled = true;
      var want = rememberRow.box.checked;
      var done = want
        ? save()
        : clearPreference().then(function (ok) {
            if (ok) toast("已忘掉记住的选择，下次分享会再问一次。");
            return ok;
          });
      done.then(function (ok) {
        if (!ok) rememberRow.box.checked = !want;
        rememberRow.box.disabled = false;
        refresh();
      });
    });
    // 已记住时改目的地或参数：直接更新记住的选择，不用再点一次保存。
    select.addEventListener("change", function () {
      if (rememberRow.box.checked) save(true).then(refresh);
    });
    metaRow.box.addEventListener("change", function () {
      if (rememberRow.box.checked) save(true).then(refresh);
    });
    backupRow.box.addEventListener("change", function () {
      backupRow.box.disabled = true;
      fetch("/public/me/backup-settings", {
        method: "PUT",
        credentials: "same-origin",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ enabled: backupRow.box.checked }),
      })
        .then(function (r) { return r.json().then(function (j) { return { ok: r.ok, j: j }; }); })
        .then(function (res) {
          if (res.ok) toast(backupRow.box.checked ? "自动备份已开启。" : "自动备份已关闭。");
          else toast((res.j && (res.j.detail || res.j.message)) || "设置失败，请稍后再试。", true);
        })
        .catch(function () { toast("设置失败，请稍后再试。", true); })
        .then(function () { refresh(); });
    });
    connect.addEventListener("click", startPersonalChannelConnect);
    change.addEventListener("click", startPersonalChannelConnect);
    disconnect.addEventListener("click", function () {
      disconnect.disabled = true;
      fetch("/public/me/share-destination", { method: "DELETE", credentials: "same-origin" })
        .then(function (r) { if (!r.ok) throw new Error(); return r.json(); })
        .then(function (data) {
          toast(data.remote_revoked ? "个人频道已断开。" : "已从本站断开；可在 Discord 中删除对应的 Webhook。");
        })
        .catch(function () { toast("断开失败，请稍后再试。", true); })
        .then(function () { disconnect.disabled = false; refresh(); });
    });
    back.addEventListener("click", function () { onBack(); });
    refresh();
    return root;
  }

  // 记住过选择时点分享是直发的（没有一级弹窗），设置入口由成功 toast 提供：这里开一个独立弹窗，
  // 内容和分享弹窗里的二级视图是同一块面板。
  function openShareSettings() {
    if (document.querySelector(".owl-share-dialog")) return;
    openOwlDialog(function (h) {
      h.dialog.setAttribute("aria-labelledby", "owl-share-settings-title");
      h.dialog.appendChild(buildShareSettingsPanel(function () { h.finish(null); }));
    });
  }

  // 个人频道授权：先占一个窗口再发起 OAuth（和之前一致，弹窗拦截给出提示）。
  function startPersonalChannelConnect() {
    var popup = window.open("about:blank", "_blank");
    if (!popup) { toast("浏览器拦截了授权窗口，请允许本站打开弹窗后重试。", true); return; }
    popup.opener = null;
    fetch("/public/me/share-destination", { method: "POST", credentials: "same-origin" })
      .then(function (r) { if (!r.ok) throw new Error("无法开始 Discord 授权，请稍后重试。"); return r.json(); })
      .then(function (data) {
        if (!/^https:\/\/discord\.com\/oauth2\/authorize\?/.test(data.url || "")) throw new Error("授权地址无效，请重试。");
        popup.location.href = data.url;
      })
      .catch(function (e) { popup.close(); toast(e.message, true); });
  }

  // null = cancel; otherwise the confirmed destination and metadata choice.
  // 一级是目的地与参数，二级（分享设置）在同一个弹窗里切换，不叠第二个模态。
  function confirmShare(pref) {
    return openOwlDialog(function (h) {
      var dialog = h.dialog;
      var rememberedPref = pref && pref.remembered;
      var preferred = rememberedPref ? rememberedPref.destination : "";
      var metaPref = rememberedPref
        ? rememberedPref.include_metadata
        : pref && pref.last
          ? pref.last.include_metadata
          : true;

      dialog.setAttribute("aria-labelledby", "owl-share-title");
      dialog.setAttribute("aria-describedby", "owl-share-warning");
      var confirmView = document.createElement("div");
      var title = document.createElement("h2");
      title.id = "owl-share-title";
      title.textContent = "分享到 Discord";
      var warning = document.createElement("p");
      warning.id = "owl-share-warning";
      warning.className = "owl-warning";
      warning.textContent = "会转发到 owl 服务器留存备份，注意二传风险。";
      var destinationLabel = document.createElement("label");
      destinationLabel.style.cssText = "display:block;margin:18px 0 0;";
      destinationLabel.textContent = "转发到";
      var select = document.createElement("select");
      select.disabled = true;
      destinationLabel.appendChild(select);
      var connection = document.createElement("div");
      connection.style.cssText = "display:flex;flex-wrap:wrap;align-items:center;gap:8px;margin-top:8px;font-size:13px;";
      var status = document.createElement("span");
      status.setAttribute("role", "status");
      status.textContent = "正在读取分享目的地…";
      status.style.color = T.dim;
      var channelLink = document.createElement("a");
      channelLink.textContent = "在 Discord 中查看";
      channelLink.target = "_blank";
      channelLink.rel = "noopener noreferrer";
      channelLink.style.cssText = "color:" + T.heading + ";display:none;";
      var retry = dialogButton("重试", false, true);
      retry.style.display = "none";
      connection.append(status, channelLink, retry);

      var metaRow = dialogCheckbox("附带生成参数（提示词、种子）");
      metaRow.label.style.marginTop = "18px";
      metaRow.box.checked = metaPref;

      var rememberRow = dialogCheckbox("记住这次选择");
      rememberRow.label.style.marginTop = "14px";
      rememberRow.box.checked = !!rememberedPref;

      var actions = document.createElement("div");
      actions.className = "owl-actions";
      var settingsBtn = document.createElement("button");
      settingsBtn.type = "button";
      settingsBtn.className = "owl-link";
      settingsBtn.textContent = "分享设置";
      var spacer = document.createElement("span");
      spacer.className = "owl-spacer";
      var cancel = dialogButton("取消", false);
      // showModal 后焦点落进弹窗：Esc 走 cancel 事件，Tab 也不会跑到弹窗外的惰性区域。
      cancel.setAttribute("autofocus", "");
      var submit = dialogButton("分享", true);
      submit.disabled = true;
      actions.append(settingsBtn, spacer, cancel, submit);

      confirmView.append(
        title, warning, destinationLabel, connection,
        metaRow.label, rememberRow.label, actions
      );

      var settingsView = document.createElement("div");
      settingsView.style.display = "none";
      var settingsBuilt = false;

      function showSettings() {
        if (!settingsBuilt) {
          settingsView.appendChild(buildShareSettingsPanel(showConfirm));
          settingsBuilt = true;
        }
        confirmView.style.display = "none";
        settingsView.style.display = "";
        dialog.setAttribute("aria-labelledby", "owl-share-settings-title");
        dialog.removeAttribute("aria-describedby");
        var first = settingsView.querySelector("select, button");
        if (first) first.focus();
      }
      function showConfirm() {
        confirmView.style.display = "";
        settingsView.style.display = "none";
        dialog.setAttribute("aria-labelledby", "owl-share-title");
        dialog.setAttribute("aria-describedby", "owl-share-warning");
        settingsBtn.focus();
        // 二级里改过记住的选择：回到一级时按最新状态回填。
        fetchShareState().then(function (s) {
          var p = s && s[0];
          var rm = p && p.remembered;
          var last = p && p.last;
          metaRow.box.checked = rm ? rm.include_metadata : last ? last.include_metadata : true;
          rememberRow.box.checked = !!rm;
          preferred = rm ? rm.destination : preferred;
          refreshDestination(true);
        });
      }

      var binding = null, loading = false;
      function destinationChanged() {
        warning.textContent = select.value === "personal"
          ? "会发到你的个人 Discord 频道，注意二传风险。"
          : "会转发到 owl 服务器留存备份，注意二传风险。";
      }
      function refreshDestination(force) {
        if (loading) return;
        loading = true; submit.disabled = true;
        fetch("/public/me/share-destination", { credentials: "same-origin", cache: "no-store" })
          .then(function (r) { if (!r.ok) throw new Error("无法读取分享目的地，请确认登录状态后重试。"); return r.json(); })
          .then(function (data) {
            var previousChoice = select.value;
            var previousBinding = binding && binding.binding_id;
            binding = data.connected ? data : null;
            select.replaceChildren();
            var owl = document.createElement("option"); owl.value = "owl"; owl.textContent = "OWL 公共频道"; select.appendChild(owl);
            if (binding) {
              var personal = document.createElement("option"); personal.value = "personal"; personal.textContent = "我的个人频道"; select.appendChild(personal);
            }
            var keep = !force && previousBinding === (binding && binding.binding_id) && previousChoice;
            if (keep) select.value = previousChoice;
            else if (preferred && (preferred === "owl" || binding)) select.value = preferred;
            else select.value = binding ? "personal" : "owl";
            preferred = "";
            if (binding) channelLink.href = binding.channel_url;
            select.disabled = false; submit.disabled = false;
            status.textContent = binding ? "个人频道已连接" : "未连接 · 可在分享设置里连接";
            channelLink.style.display = binding ? "inline" : "none";
            retry.style.display = "none";
            if (previousChoice !== select.value || previousBinding !== (binding && binding.binding_id)) destinationChanged();
          })
          .catch(function (e) { status.textContent = e.message; retry.style.display = "inline-block"; })
          .then(function () { loading = false; });
      }
      select.addEventListener("change", destinationChanged);
      retry.addEventListener("click", function () { refreshDestination(); });
      settingsBtn.addEventListener("click", showSettings);
      var onWinFocus = function () { refreshDestination(); };
      window.addEventListener("focus", onWinFocus);
      h.onCleanup(function () { window.removeEventListener("focus", onWinFocus); });
      cancel.addEventListener("click", function () { h.finish(null); });
      submit.addEventListener("click", function () {
        if (submit.disabled) return;
        h.finish({
          includeMetadata: metaRow.box.checked,
          destination: select.value,
          bindingId: binding && binding.binding_id,
          remember: rememberRow.box.checked,
        });
      });
      dialog.append(confirmView, settingsView);
      refreshDestination();
    });
  }

  // 画布上的图是 blob:，官网自己 createObjectURL 出来的就是**原图字节**（含元数据）。
  // 记下来，上传时直接用，避免 fetch 之后被再次编码。
  var BLOBS = new Map();
  var OCO = URL.createObjectURL;
  URL.createObjectURL = function (blob) {
    var u = OCO.call(URL, blob);
    try {
      if (blob && /^image\//.test(blob.type || "")) {
        BLOBS.set(u, blob);
        if (BLOBS.size > 40) BLOBS.delete(BLOBS.keys().next().value);
      }
    } catch (e) {}
    return u;
  };

  function toast(text, bad, action) {
    var el = document.createElement("div");
    el.textContent = text;
    el.setAttribute("data-owl-toast", "1");
    el.style.cssText = [
      "position:fixed", "left:50%", "bottom:96px", "transform:translateX(-50%)",
      "max-width:min(560px,90vw)", "padding:12px 18px", "border-radius:10px", "z-index:2147483000",
      "font:600 14px/1.5 system-ui,-apple-system,'Segoe UI',sans-serif", "color:#fff",
      "background:" + (bad ? "#7a2c2c" : "#2f2a56"),
      "border:1px solid " + (bad ? "#d16060" : "#5b52a8"),
      "box-shadow:0 8px 28px rgba(0,0,0,.45)", "text-align:center",
      action ? "pointer-events:auto" : "pointer-events:none",
    ].join(";");
    if (action) {
      var btn = document.createElement("button");
      btn.type = "button";
      btn.textContent = action.label;
      btn.style.cssText = "margin-left:12px;padding:2px 0;border:0;background:none;font:inherit;color:" +
        T.heading + ";text-decoration:underline;text-underline-offset:3px;cursor:pointer";
      btn.addEventListener("click", function () { el.remove(); action.onClick(); });
      el.appendChild(btn);
    }
    document.body.appendChild(el);
    setTimeout(function () { el.remove(); }, bad ? 6000 : 5000);
  }

  // 画布里当前显示的那张图（最大的一张 img，blob:/data: 都算）
  function currentImage() {
    var best = null, bestArea = 0;
    var list = document.querySelectorAll("img");
    for (var i = 0; i < list.length; i++) {
      var img = list[i];
      var src = img.currentSrc || img.src || "";
      if (!/^(blob:|data:image\/)/.test(src)) continue;
      var r = img.getBoundingClientRect();
      if (r.width < 200 || r.height < 200) continue;
      var area = r.width * r.height;
      if (area > bestArea) { bestArea = area; best = img; }
    }
    return best;
  }

  function textOf(sel, minWidth) {
    var list = document.querySelectorAll(sel);
    for (var i = 0; i < list.length; i++) {
      var r = list[i].getBoundingClientRect();
      if (r.width >= (minWidth || 120)) return (list[i].innerText || "").trim();
    }
    return "";
  }

  function metaFor(img) {
    var body = document.body.innerText || "";
    // 尺寸以图片自身像素为准（页面上的文字是显示尺寸，可能被缩放）
    var size = "";
    if (img && img.naturalWidth) size = img.naturalWidth + " × " + img.naturalHeight;
    if (!size) {
      var m = body.match(/(\d{3,5})\s*[×x]\s*(\d{3,5})/);
      if (m) size = m[1] + " × " + m[2];
    }
    var seed = "";
    var s = body.match(/\b(\d{9,11})\b/);
    if (s) seed = s[1];
    var model = textOf(".ProseMirror, textarea", 0) ? "" : "";
    var modelRow = document.body.innerText.match(/Model\s*\n?\s*(V?[\w.\s-]{2,24})/);
    if (modelRow) model = modelRow[1].split("\n")[0].trim();
    return {
      prompt: textOf(".ProseMirror", 200).replace(/\u00a0/g, " "),
      size: size,
      seed: seed,
      model: model,
    };
  }

  // Re-encode decoded pixels, omitting PNG text / WebP EXIF / JPEG EXIF and comments.
  // NovelAI also stores generation metadata in alpha LSBs. Clear LSB payloads in
  // all channels, keeping near-opaque alpha fully opaque and preserving dimensions.
  // Pixel values may change by one level; never fall back to the original on error.
  function imageWithoutMetadata(blob) {
    return new Promise(function (resolve, reject) {
      var url = URL.createObjectURL(blob);
      var image = new Image();
      image.onload = function () {
        URL.revokeObjectURL(url);
        try {
          var w = image.naturalWidth, h = image.naturalHeight;
          if (!w || !h || w > 8192 || h > 8192 || w * h > 16777216) throw new Error("图片尺寸过大，无法清理元数据");
          var canvas = document.createElement("canvas");
          canvas.width = w; canvas.height = h;
          var ctx = canvas.getContext("2d");
          if (!ctx) throw new Error("浏览器无法处理图片");
          ctx.drawImage(image, 0, 0);
          var pixels = ctx.getImageData(0, 0, w, h);
          for (var i = 0; i < pixels.data.length; i += 4) {
            pixels.data[i] &= 254;
            pixels.data[i + 1] &= 254;
            pixels.data[i + 2] &= 254;
            var alpha = pixels.data[i + 3];
            pixels.data[i + 3] = alpha >= 254 ? 255 : alpha & 254;
          }
          ctx.putImageData(pixels, 0, 0);
          canvas.toBlob(function (clean) {
            canvas.width = canvas.height = 0;
            if (clean && clean.size > 0) resolve(clean);
            else reject(new Error("清理元数据失败，未转发"));
          }, "image/png");
        } catch (e) { reject(e); }
      };
      image.onerror = function () { URL.revokeObjectURL(url); reject(new Error("图片无法读取，未转发")); };
      image.src = url;
    });
  }

  function uploadToExplore(ev) {
    var link = ev.target && ev.target.closest ? ev.target.closest('a[href="/explore/register"]') : null;
    if (!link) return;
    ev.preventDefault();
    ev.stopPropagation();
    if (link.getAttribute("data-owl-busy") === "1" || document.querySelector(".owl-share-dialog")) return;
    var img = currentImage();
    if (!img) { toast("没找到当前这张图，先出图再上传。", true); return; }
    link.setAttribute("data-owl-busy", "1");
    // 记住过就直接按记住的选择发；没记住才弹确认。分享设置入口在一级弹窗里，
    // 直发这条路走完后用 5 秒的 toast 补一个入口，否则设置就没地方改了。
    fetchShareState().then(function (s) {
      // 读取的异步间隙里可能已经开了另一个弹窗（连点 / 前一个没关完）：这次点击作废。
      if (document.querySelector(".owl-share-dialog")) return;
      var remembered = s && s[0] && s[0].remembered;
      if (remembered) {
        return doUpload(img, {
          includeMetadata: remembered.include_metadata,
          destination: remembered.destination,
          bindingId: remembered.binding_id,
        }).then(function (res) {
          if (res.ok) {
            toast(remembered.destination === "personal" ? "已发送到你的个人频道 ✅" : "已收到，正在发到图乐园频道 ✅", false, {
              label: "分享设置",
              onClick: openShareSettings,
            });
          } else {
            toast((res.j && (res.j.detail || res.j.message)) || "提交失败，稍后再试。", true);
          }
        });
      }
      return confirmShare(s && s[0]).then(function (choice) {
        if (choice === null) return;
        if (choice.remember) savePreference(choice);
        toast("正在提交到 Discord…");
        return doUpload(img, choice).then(function (res) {
          if (res.ok) toast(choice.destination === "personal" ? "已发送到你的个人频道 ✅" : "已收到，正在发到图乐园频道 ✅");
          else toast((res.j && (res.j.detail || res.j.message)) || "提交失败，稍后再试。", true);
        });
      });
    })
      .catch(function (e) { toast("提交失败：" + e, true); })
      .then(function () { link.removeAttribute("data-owl-busy"); });
  }

  // 取画布字节 → 按选择清理元数据 → 组表单提交。metadata_state 告诉服务端我们做过的处理，
  // 卡片标签以服务端对附件字节的验证为准（见 explore.rs 的 attachment_label）。
  function doUpload(img, choice) {
    var includeMetadata = choice.includeMetadata;
    var meta = includeMetadata ? metaFor(img) : {};
    var src = img.currentSrc || img.src;
    var reused = BLOBS.get(src);
    return (reused ? Promise.resolve(reused) : fetch(src).then(function (r) { return r.blob(); }))
      .then(function (blob) { return includeMetadata ? blob : imageWithoutMetadata(blob); })
      .then(function (blob) {
        var fd = new FormData();
        var ext = /png/.test(blob.type) ? "png" : /jpe?g/.test(blob.type) ? "jpg" : "webp";
        fd.append("image", blob, "owl-explore." + ext);
        fd.append("destination", choice.destination);
        fd.append("metadata_state", includeMetadata ? "kept" : "stripped");
        if (choice.destination === "personal") fd.append("binding_id", choice.bindingId);
        if (includeMetadata) {
          fd.append("prompt", meta.prompt || "");
          fd.append("size", meta.size || "");
          fd.append("seed", meta.seed || "");
          fd.append("model", meta.model || "");
        }
        return fetch("/owl/explore", { method: "POST", body: fd, credentials: "same-origin" })
          .then(function (r) { return r.json().catch(function () { return {}; }).then(function (j) { return { ok: r.ok, j: j }; }); });
      });
  }
  document.addEventListener("click", uploadToExplore, true);
  document.addEventListener("click", openOwlDiscord, true);

})();
