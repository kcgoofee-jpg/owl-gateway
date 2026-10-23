/** 请求日志「客户端」列：把 User-Agent 缩成一眼能看懂的「系统 · 浏览器 / 工具」；认不出的（脚本、SDK）直接截一段原文；没记录（保留期外）是「—」。 */
export function uaLabel(ua: string): string {
  const s = ua.trim();
  if (!s) return "—";
  // 不是浏览器（开头不是 Mozilla/）：脚本、客户端、SDK 自报的第一个「名字/版本」就是答案
  if (!/^Mozilla\//.test(s)) {
    const first = s.split(/\s/)[0] ?? s;
    return first.length > 24 ? `${first.slice(0, 24)}…` : first;
  }
  const os = /Android ([\d.]+)/.exec(s)
    ? `Android ${/Android ([\d.]+)/.exec(s)?.[1]?.split(".")[0]}`
    : /iPhone|iPad|iPod/.test(s)
      ? "iOS"
      : /Windows NT/.test(s)
        ? "Windows"
        : /Mac OS X|Macintosh/.test(s)
          ? "macOS"
          : /Linux|X11/.test(s)
            ? "Linux"
            : "";
  const browser = /Edg(?:e|A|iOS)?\//.test(s)
    ? "Edge"
    : /OPR\/|Opera/.test(s)
      ? "Opera"
      : /Firefox\/|FxiOS/.test(s)
        ? "Firefox"
        : /Chrome\/|CriOS/.test(s)
          ? "Chrome"
          : /Safari\//.test(s)
            ? "Safari"
            : "";
  if (os && browser) return `${os} · ${browser}`;
  if (browser) return browser;
  if (os) return os;
  return s.length > 24 ? `${s.slice(0, 24)}…` : s;
}
