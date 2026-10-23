import { signal } from "@preact/signals";

/** 当前生效的主题；跟随系统时为 null。只用一个存储键 owl-theme（首页 / 后台 / 状态页共用）。 */
export const themeChoice = signal<"light" | "dark" | null>(read());

function read(): "light" | "dark" | null {
  try {
    const t = localStorage.getItem("owl-theme");
    return t === "light" || t === "dark" ? t : null;
  } catch {
    return null;
  }
}

export function effectiveTheme(): "light" | "dark" {
  return themeChoice.value ?? (matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light");
}

export function toggleTheme(): void {
  const next = effectiveTheme() === "dark" ? "light" : "dark";
  themeChoice.value = next;
  document.documentElement.setAttribute("data-theme", next);
  try {
    localStorage.setItem("owl-theme", next);
  } catch {
    /* 隐私模式：只在本页生效 */
  }
}
