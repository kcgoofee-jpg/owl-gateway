import { LogOut, Moon, Sun } from "lucide-preact";
import { useEffect, useRef, useState } from "preact/hooks";
import { ApiError, sendJSON } from "../../lib/api";
import { animate, reduceMotion } from "../../lib/motion";
import { effectiveTheme, themeChoice, toggleTheme } from "../../lib/theme";
import { CopyField, Menu, toast } from "../../ui";
import { UsageRules } from "./UsageRules";
import "../../ui/chrome.css";
import { useLoginHref } from "../../lib/deviceid";

/** 明暗切换：无边框的圆形图标按钮。图标表示「点了会变成什么」，读屏也是同样的说法；切换时图标转 90 度。 */
export function ThemeButton() {
  // 跟随系统时，系统主题变了也要重绘
  const [, bump] = useState(0);
  const ref = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    const mq = matchMedia("(prefers-color-scheme: dark)");
    const on = () => bump((n) => n + 1);
    mq.addEventListener("change", on);
    return () => mq.removeEventListener("change", on);
  }, []);
  void themeChoice.value;
  const dark = effectiveTheme() === "dark";
  const label = dark ? "切换到浅色主题" : "切换到深色主题";
  const onClick = () => {
    toggleTheme();
    // 新图标渲染出来之后再转
    requestAnimationFrame(() => {
      const svg = ref.current?.querySelector("svg");
      if (svg && !reduceMotion())
        animate(svg, { rotate: [-90, 0], opacity: [0, 1] }, { duration: 0.3, ease: "easeOut" });
    });
  };
  return (
    <button ref={ref} type="button" class="icon-btn" aria-label={label} title={label} onClick={onClick}>
      {dark ? <Sun size={18} aria-hidden="true" /> : <Moon size={18} aria-hidden="true" />}
    </button>
  );
}

async function logout() {
  try {
    await sendJSON("POST", "/logout");
    location.reload();
  } catch (e) {
    toast(e instanceof ApiError ? e.message : "退出失败，请稍后再试", { error: true });
  }
}

/** Discord 头像地址（头像是哈希，可能带动图前缀 a_）；拿不到就用名字首字。 */
function avatarUrl(id?: string, hash?: string | null): string {
  return id && hash && /^(a_)?[0-9a-f]{32}$/.test(hash)
    ? `https://cdn.discordapp.com/avatars/${id}/${hash}.png?size=64`
    : "";
}

/** 右上角账户入口：游客是一个「登录」按钮；登录后是头像，点开是名字 + 退出。 */
function Account(props: { user: TopbarUser }) {
  const [broken, setBroken] = useState(false);
  const { name } = props.user;
  const src = broken ? "" : avatarUrl(props.user.id, props.user.avatar);
  const initial = (name.match(/[\p{Script=Han}A-Za-z0-9]/u)?.[0] ?? "?").toUpperCase();
  return (
    <Menu
      button={(p) => (
        <button type="button" class="lp-avatar" aria-label={`账户：${name}`} title={name} {...p}>
          {src ? (
            <img
              src={src}
              alt=""
              width={32}
              height={32}
              referrerpolicy="no-referrer"
              onError={() => setBroken(true)}
            />
          ) : (
            <span aria-hidden="true">{initial}</span>
          )}
        </button>
      )}
      header={
        <>
          已登录
          <strong>{name}</strong>
          {props.user.key && (
            <div class="lp-account__key">
              <span class="field__label">我的 Key</span>
              <CopyField value={props.user.key} label="Key" />
              <span class="field__label">接入地址</span>
              <CopyField value={location.origin} label="接入地址" />
            </div>
          )}
        </>
      }
      items={[
        {
          label: (
            <>
              <LogOut size={14} aria-hidden="true" /> 退出登录
            </>
          ),
          onSelect: logout,
        },
      ]}
    />
  );
}

export type TopbarUser = {
  name: string;
  id?: string | undefined;
  avatar?: string | null | undefined;
  key?: string | undefined;
};

/**
 * 全站顶栏：Logo（两只眼睛 + 站名，就是回首页的链接）、游客登录、主题切换、头像菜单。
 * 首页、问卷共用；独立状态页（网关渲染）照同样的结构和类名画一份。
 */
export function Topbar(props: {
  /** 已登录成员；游客不传 */
  user?: TopbarUser | undefined;
  /** 顶栏右侧是否显示游客登录入口 */
  loginOpen: boolean;
  /** 已登录且有 Key 时，顶栏放「生图工作台」按钮，点进去就是 /workbench。 */
  workbench?: boolean;
  /** Homepage brand links directly to the inspiration gallery. */
  brandHref?: string;
}) {
  const loginHref = useLoginHref();
  return (
    <header class="site-top">
      <div class="wrap site-top__in">
        <a class="brand" href={props.brandHref ?? "/"}>
          <span class="brand__eyes" aria-hidden="true">
            <i />
            <i />
          </span>
          <span class="brand__name">猫头鹰公益站</span>
        </a>
        <div class="site-top__right">
          {props.workbench && (
            <a id="workbench-entry" class="btn btn--primary btn--sm site-top__wb" href="/workbench">
              生图工作台
              <span class="site-top__beta">Beta</span>
            </a>
          )}
          {!props.user && props.loginOpen && (
            <a class="btn btn--primary" href={loginHref} aria-label="用 Discord 登录">
              登录
            </a>
          )}
          <ThemeButton />
          {props.user && <Account user={props.user} />}
        </div>
      </div>
    </header>
  );
}

export function SiteFooter(props: { note?: string }) {
  return (
    <footer class="lp-foot">
      <p>
        猫头鹰公益站 · 非商业公益服务，不保证可用性
        {props.note ? ` · ${props.note}` : ""}
      </p>
      <p>本站不保存提示词或生成图片；请求结果与额度记录用于排障和防滥用。</p>
      <details class="lp-use-terms">
        <summary>使用规则</summary>
        <p>禁止以下行为：</p>
        <UsageRules />
      </details>
    </footer>
  );
}
