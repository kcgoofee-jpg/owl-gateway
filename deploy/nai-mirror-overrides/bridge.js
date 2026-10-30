/* owl ⇄ NovelAI 生图页桥：把官网前端的 API 调用改到本站网关。
   只做三件事：改 base、带 Key、隐藏账号横幅。 */
(function () {
  window.__BRIDGE_VERSION__ = "2026-10-30-d";
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
    return of.call(window, mapped, init).then(function (r) {
      window.__bridgeLog.push(mapped + " -> " + r.status);
      return r;
    }).catch(function (e) {
      window.__bridgeLog.push(mapped + " -> ERR " + e);
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
  function watchBanners() {
    hideBanners();
    var mo = new MutationObserver(function () { hideBanners(); });
    mo.observe(document.documentElement, { childList: true, subtree: true, characterData: true });
  }
  if (document.body) watchBanners();
  else document.addEventListener("DOMContentLoaded", watchBanners);

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

  function uploadToExplore(ev) {
    var link = ev.target && ev.target.closest ? ev.target.closest('a[href="/explore/register"]') : null;
    if (!link) return;
    ev.preventDefault();
    ev.stopPropagation();
    if (link.getAttribute("data-owl-busy") === "1") return;
    var img = currentImage();
    if (!img) { toast("没找到当前这张图，先出图再上传。", true); return; }
    var meta = metaFor(img);
    link.setAttribute("data-owl-busy", "1");
    toast("正在提交到 Discord…");
    var src = img.currentSrc || img.src;
    var reused = BLOBS.get(src);
    (reused ? Promise.resolve(reused) : fetch(src).then(function (r) { return r.blob(); }))
      .then(function (blob) {
        var fd = new FormData();
        var ext = /png/.test(blob.type) ? "png" : "webp";
        fd.append("image", blob, "owl-explore." + ext);
        fd.append("prompt", meta.prompt || "");
        fd.append("size", meta.size || "");
        fd.append("seed", meta.seed || "");
        fd.append("model", meta.model || "");
        return fetch("/owl/explore", { method: "POST", body: fd, credentials: "same-origin" });
      })
      .then(function (r) { return r.json().catch(function () { return {}; }).then(function (j) { return { ok: r.ok, j: j }; }); })
      .then(function (res) {
        if (res.ok) toast("已发到 Discord 图乐园频道 ✅");
        else toast((res.j && (res.j.detail || res.j.message)) || "提交失败，稍后再试。", true);
      })
      .catch(function (e) { toast("提交失败：" + e, true); })
      .then(function () { link.removeAttribute("data-owl-busy"); });
  }
  document.addEventListener("click", uploadToExplore, true);
})();
