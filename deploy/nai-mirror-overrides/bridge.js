/* owl ⇄ NovelAI 生图页桥：把官网前端的 API 调用改到本站网关。
   只做三件事：改 base、带 Key、隐藏账号横幅。 */
(function () {
  window.__BRIDGE_VERSION__ = "2026-11-06-share-metadata";
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
    var mo = new MutationObserver(function () { hideBanners(); renameDiscord(); });
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
     改成把画布上这张原图（webp/png 原始字节，官方元数据都在）交给网关，由机器人发到 Discord。 ---- */

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

  function toast(text, bad) {
    var el = document.createElement("div");
    el.textContent = text;
    el.setAttribute("data-owl-toast", "1");
    el.style.cssText = [
      "position:fixed", "left:50%", "bottom:96px", "transform:translateX(-50%)",
      "max-width:min(560px,90vw)", "padding:12px 18px", "border-radius:10px", "z-index:2147483000",
      "font:600 14px/1.5 system-ui,-apple-system,'Segoe UI',sans-serif", "color:#fff",
      "background:" + (bad ? "#7a2c2c" : "#2f2a56"),
      "border:1px solid " + (bad ? "#d16060" : "#5b52a8"),
      "box-shadow:0 8px 28px rgba(0,0,0,.45)", "text-align:center", "pointer-events:none",
    ].join(";");
    document.body.appendChild(el);
    setTimeout(function () { el.remove(); }, bad ? 6000 : 4000);
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

  // null = cancel, false = image only, true = original bytes and generation metadata.
  function confirmShare() {
    return new Promise(function (resolve) {
      var previous = document.activeElement;
      var dialog = document.createElement("dialog");
      dialog.className = "owl-share-dialog";
      dialog.setAttribute("aria-labelledby", "owl-share-title");
      dialog.setAttribute("aria-describedby", "owl-share-warning owl-share-help");
      dialog.style.cssText = "width:min(480px,calc(100vw - 40px));box-sizing:border-box;padding:24px;border:1px solid #494844;border-radius:14px;background:#191918;color:#eeeeec;font:14px/1.65 system-ui,-apple-system,'Segoe UI',sans-serif;box-shadow:0 20px 70px #0009;";
      var style = document.createElement("style");
      style.textContent = ".owl-share-dialog::backdrop{background:rgba(0,0,0,.65)}.owl-share-dialog button:focus-visible,.owl-share-dialog input:focus-visible{outline:2px solid #b1a9ff;outline-offset:3px}";
      var title = document.createElement("h2");
      title.id = "owl-share-title";
      title.textContent = "确认分享";
      title.style.cssText = "margin:0 0 12px;font-size:18px;";
      var warning = document.createElement("p");
      warning.id = "owl-share-warning";
      warning.textContent = "该分享转发至 owl 服务器进行保存备份，请确认是否有潜在二传违规风险！";
      warning.style.cssText = "margin:0 0 20px;";
      var label = document.createElement("label");
      label.style.cssText = "display:flex;align-items:center;gap:10px;cursor:pointer;";
      var checkbox = document.createElement("input");
      checkbox.type = "checkbox";
      checkbox.checked = false;
      checkbox.style.cssText = "width:18px;height:18px;flex:none;accent-color:#5b5bd6;";
      label.appendChild(checkbox);
      label.appendChild(document.createTextNode("包含生成元数据（提示词、种子等）"));
      var help = document.createElement("p");
      help.id = "owl-share-help";
      help.textContent = "默认仅分享去除生成参数的图片（PNG）。勾选后将发送原图和生成参数，其他人可能据此复现图片。";
      help.style.cssText = "margin:10px 0 24px;color:#b5b3ad;font-size:13px;";
      var actions = document.createElement("div");
      actions.style.cssText = "display:flex;justify-content:flex-end;gap:10px;";
      var cancel = document.createElement("button");
      cancel.type = "button";
      cancel.textContent = "取消";
      cancel.style.cssText = "padding:9px 18px;border:1px solid #494844;border-radius:8px;background:#222221;color:#eeeeec;font:inherit;cursor:pointer;";
      var submit = document.createElement("button");
      submit.type = "button";
      submit.textContent = "确认转发";
      submit.style.cssText = "padding:9px 18px;border:1px solid #5b5bd6;border-radius:8px;background:#5b5bd6;color:#fff;font:inherit;cursor:pointer;";
      var settled = false;
      function finish(value) {
        if (settled) return;
        settled = true;
        dialog.remove();
        if (previous && previous.isConnected && previous.focus) previous.focus();
        resolve(value);
      }
      cancel.addEventListener("click", function () { finish(null); });
      submit.addEventListener("click", function () { finish(checkbox.checked); });
      dialog.addEventListener("cancel", function (ev) { ev.preventDefault(); finish(null); });
      dialog.addEventListener("close", function () { finish(null); });
      actions.appendChild(cancel);
      actions.appendChild(submit);
      [style, title, warning, label, help, actions].forEach(function (el) { dialog.appendChild(el); });
      document.body.appendChild(dialog);
      try { dialog.showModal(); cancel.focus(); }
      catch (e) { finish(null); toast("当前浏览器无法打开分享确认，请更新浏览器后再试。", true); }
    });
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
    confirmShare().then(function (includeMetadata) {
      if (includeMetadata === null) return;
      var meta = includeMetadata ? metaFor(img) : {};
      toast("正在提交到 Discord…");
      var src = img.currentSrc || img.src;
      var reused = BLOBS.get(src);
      return (reused ? Promise.resolve(reused) : fetch(src).then(function (r) { return r.blob(); }))
        .then(function (blob) { return includeMetadata ? blob : imageWithoutMetadata(blob); })
        .then(function (blob) {
          var fd = new FormData();
          var ext = /png/.test(blob.type) ? "png" : /jpe?g/.test(blob.type) ? "jpg" : "webp";
          fd.append("image", blob, "owl-explore." + ext);
          if (includeMetadata) {
            fd.append("prompt", meta.prompt || "");
            fd.append("size", meta.size || "");
            fd.append("seed", meta.seed || "");
            fd.append("model", meta.model || "");
          }
          return fetch("/owl/explore", { method: "POST", body: fd, credentials: "same-origin" });
        })
        .then(function (r) { return r.json().catch(function () { return {}; }).then(function (j) { return { ok: r.ok, j: j }; }); })
        .then(function (res) {
          if (res.ok) toast("已收到，正在发到图乐园频道 ✅");
          else toast((res.j && (res.j.detail || res.j.message)) || "提交失败，稍后再试。", true);
        });
    })
      .catch(function (e) { toast("提交失败：" + e, true); })
      .then(function () { link.removeAttribute("data-owl-busy"); });
  }
  document.addEventListener("click", uploadToExplore, true);
  document.addEventListener("click", openOwlDiscord, true);
})();
