/**
 * 工作台偏好。默认对齐官网当前的 NovelAI Dark。
 * Sand / Slate 仍按 naitheme 四层底。品牌只留猫头鹰眼睛，不搬字标。
 */
export type WbTheme = "dark" | "sand" | "slate";

export type WbPrefs = {
  theme: WbTheme;
  autoDownload: boolean;
  persistHistory: boolean;
  hideHistory: boolean;
  reducedMotion: boolean;
  transparentBg: boolean;
};

const KEY = "owl-wb-prefs";

export const THEMES: Array<{ id: WbTheme; label: string; blurb: string; swatch: [string, string, string] }> =
  [
    {
      id: "dark",
      label: "Dark",
      blurb: "官网默认深色。标题用米色。",
      swatch: ["#191B31", "#13152C", "#F5F3C2"],
    },
    {
      id: "sand",
      label: "Sand",
      blurb: "羊皮纸浅色。",
      swatch: ["#DFCDA4", "#EBDAB4", "#AB0613"],
    },
    {
      id: "slate",
      label: "Slate",
      blurb: "夜空深色。标题用薄荷绿。",
      swatch: ["#02030B", "#1E2231", "#71FFC3"],
    },
  ];

export const DEFAULT_PREFS: WbPrefs = {
  theme: "dark",
  autoDownload: false,
  persistHistory: false,
  hideHistory: false,
  reducedMotion: false,
  transparentBg: false,
};

function storedTheme(parsed: Partial<WbPrefs> & { v?: number }): WbTheme {
  if (parsed.v === 2 && (parsed.theme === "dark" || parsed.theme === "sand" || parsed.theme === "slate")) {
    return parsed.theme;
  }
  if (parsed.theme === "slate") return "slate";
  return "dark";
}

export function loadPrefs(): WbPrefs {
  try {
    const raw = localStorage.getItem(KEY);
    if (!raw) return { ...DEFAULT_PREFS };
    const parsed = JSON.parse(raw) as Partial<WbPrefs> & { v?: number };
    return {
      theme: storedTheme(parsed),
      autoDownload: parsed.autoDownload === true,
      persistHistory: parsed.persistHistory === true,
      hideHistory: parsed.hideHistory === true,
      reducedMotion: parsed.reducedMotion === true,
      transparentBg: parsed.transparentBg === true,
    };
  } catch {
    return { ...DEFAULT_PREFS };
  }
}

export function savePrefs(prefs: WbPrefs): void {
  try {
    localStorage.setItem(KEY, JSON.stringify({ ...prefs, v: 2 }));
  } catch {
    /* 隐私模式写不进去就只在这次会话生效 */
  }
}
