/** 工作台设置：对照官网 User Settings（图像生成 / 界面 / 主题 / 账号）。 */
import { CloseIcon } from "./Icons";
import { THEMES, type WbPrefs, type WbTheme } from "./prefs";

export type SettingsTab = "image" | "interface" | "theme" | "account";

const TITLES: Record<SettingsTab, string> = {
  image: "图像生成",
  interface: "界面",
  theme: "主题",
  account: "账号",
};

export function SettingsModal(props: {
  open: boolean;
  tab: SettingsTab;
  setTab: (t: SettingsTab) => void;
  prefs: WbPrefs;
  setPrefs: (p: WbPrefs) => void;
  quota: string | null;
  who: string;
  onClose: () => void;
}) {
  if (!props.open) return null;
  const set = (p: Partial<WbPrefs>) => props.setPrefs({ ...props.prefs, ...p });
  return (
    <>
      <button type="button" class="nv-set-bg" aria-label="关闭设置" onClick={props.onClose} />
      <div class="nv-set" role="dialog" aria-label="用户设置" aria-modal="true">
        <aside class="nv-set__nav">
          <p class="nv-set__kicker">用户设置</p>
          {(["image", "interface", "theme", "account"] as const).map((id) => (
            <button key={id} type="button" aria-pressed={props.tab === id} onClick={() => props.setTab(id)}>
              {TITLES[id]}
            </button>
          ))}
          <span class="nv-set__grow" />
          <button
            type="button"
            class="nv-nav__out"
            onClick={() => {
              void fetch("/logout", { method: "POST" }).finally(() => {
                location.href = "/";
              });
            }}
          >
            退出
          </button>
        </aside>
        <div class="nv-set__body">
          <header>
            <h2>{TITLES[props.tab]}</h2>
            <button type="button" class="nv-iconbtn" aria-label="关闭设置" onClick={props.onClose}>
              <CloseIcon />
            </button>
          </header>
          {props.tab === "theme" && (
            <div class="nv-themes">
              <p>默认是官网那套深色。Sand 和 Slate 仍按 naitheme，品牌只留猫头鹰眼睛。</p>
              {THEMES.map((t) => (
                <button
                  key={t.id}
                  type="button"
                  class={props.prefs.theme === t.id ? "nv-theme is-on" : "nv-theme"}
                  aria-pressed={props.prefs.theme === t.id}
                  onClick={() => set({ theme: t.id as WbTheme })}
                >
                  <span class="nv-theme__swatch">
                    {t.swatch.map((c) => (
                      <i key={c} style={{ background: c }} />
                    ))}
                  </span>
                  <b>{t.label}</b>
                  <span>{t.blurb}</span>
                </button>
              ))}
            </div>
          )}
          {props.tab === "image" && (
            <div class="nv-set__grid">
              <p>
                <b>图像设置</b>
              </p>
              <label class="nv-switch">
                <span>
                  <b>流式出图</b>
                  <i>网关还没有中间帧。开了也不会推流。</i>
                </span>
                <input type="checkbox" checked={false} disabled aria-label="流式出图" />
              </label>
              <label class="nv-switch">
                <span>
                  <b>透明背景</b>
                  <i>使用模型的透明背景生成标记。</i>
                </span>
                <input
                  type="checkbox"
                  checked={props.prefs.transparentBg}
                  onChange={(e) => set({ transparentBg: e.currentTarget.checked })}
                />
              </label>
              <div class="nv-switch">
                <span>
                  <b>Alpha 模式</b>
                  <i>只影响透明背景图片的 Alpha 通道格式。</i>
                </span>
                <fieldset class="nv-seg" aria-label="透明背景 Alpha 模式">
                  <button
                    type="button"
                    aria-pressed={props.prefs.straightAlpha}
                    onClick={() => set({ straightAlpha: true })}
                  >
                    Straight
                  </button>
                  <button
                    type="button"
                    aria-pressed={!props.prefs.straightAlpha}
                    onClick={() => set({ straightAlpha: false })}
                  >
                    Premultiplied
                  </button>
                </fieldset>
              </div>
              <label class="nv-switch">
                <span>
                  <b>生成后自动下载</b>
                  <i>生成完成后图片会自动下载。</i>
                </span>
                <input
                  type="checkbox"
                  checked={props.prefs.autoDownload}
                  onChange={(e) => set({ autoDownload: e.currentTarget.checked })}
                />
              </label>
              <label class="nv-switch">
                <span>
                  <b>记住出图历史</b>
                  <i>出图历史会保存在本地。默认关闭。</i>
                </span>
                <input
                  type="checkbox"
                  checked={props.prefs.persistHistory}
                  onChange={(e) => set({ persistHistory: e.currentTarget.checked })}
                />
              </label>
              <label class="nv-switch">
                <span>
                  <b>隐藏本次画廊</b>
                  <i>右侧缩略图栏收起来。</i>
                </span>
                <input
                  type="checkbox"
                  checked={props.prefs.hideHistory}
                  onChange={(e) => set({ hideHistory: e.currentTarget.checked })}
                />
              </label>
              <label class="nv-switch">
                <span>
                  <b>减弱动效</b>
                  <i>输出视图移动时不做动画。</i>
                </span>
                <input
                  type="checkbox"
                  checked={props.prefs.reducedMotion}
                  onChange={(e) => set({ reducedMotion: e.currentTarget.checked })}
                />
              </label>
              <div>
                <p>
                  <b>成图格式</b>
                </p>
                <fieldset class="nv-seg">
                  <legend class="nv-sr">成图格式</legend>
                  <button type="button" aria-pressed>
                    PNG
                  </button>
                  <button type="button" aria-pressed={false} disabled title="网关目前只回 PNG">
                    WebP（无损）
                  </button>
                </fieldset>
                <p class="nai-note">流式预览和 WebP 要等网关。</p>
              </div>
            </div>
          )}
          {props.tab === "interface" && (
            <div class="nv-set__grid">
              <p>
                <b>界面语言</b>
              </p>
              <p class="nv-lang">中文</p>
              <p class="nai-note">工作台固定中文。主题在「主题」页切换。</p>
            </div>
          )}
          {props.tab === "account" && (
            <div class="nv-set__grid">
              <p>
                <b>当前档位</b>
              </p>
              <p>{props.who}</p>
              <p>{props.quota ?? "额度会显示在顶栏。"}</p>
              <p class="nai-note">登录仍走 Discord。Key 在首页头像菜单。</p>
            </div>
          )}
        </div>
      </div>
    </>
  );
}
