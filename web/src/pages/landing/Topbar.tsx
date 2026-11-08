import { LogOut } from "lucide-preact";
import { useEffect, useState } from "preact/hooks";
import { ApiError, sendJSON } from "../../lib/api";
import { CopyField, Menu, toast } from "../../ui";
import "../../ui/chrome.css";
import { useLoginHref } from "../../lib/deviceid";

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

type NewApiAccount = { username: string; password: string };

export function NewApiCredentials() {
  const [account, setAccount] = useState<NewApiAccount | null>(null);
  useEffect(() => {
    const controller = new AbortController();
    fetch("/ai/api/account", { credentials: "include", cache: "no-store", signal: controller.signal })
      .then((response) => {
        if (!response.ok) throw new Error("Account unavailable");
        return response.json() as Promise<NewApiAccount>;
      })
      .then(setAccount)
      .catch(() => {});
    return () => controller.abort();
  }, []);
  if (!account) return null;
  return (
    <div class="lp-account__newapi" data-owl-newapi="native">
      <a href="https://newapi.davidzhao.top/" target="_blank" rel="noopener noreferrer">
        NEW API ↗
      </a>
      <span class="field__label">用户名</span>
      <CopyField value={account.username} label="New API 用户名" />
      <span class="field__label">密码</span>
      <CopyField value={account.password} label="New API 密码" />
    </div>
  );
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
          <strong>{name}</strong>
          {props.user.key && (
            <div class="lp-account__key">
              <span class="field__label">nai key</span>
              <CopyField value={props.user.key} label="Key" />
              <span class="field__label">base url</span>
              <CopyField value={location.origin} label="接入地址" />
            </div>
          )}
          <NewApiCredentials />
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
 * 全站顶栏：Logo（两只眼睛 + 站名，就是回首页的链接）、游客登录和头像菜单。
 */
export function Topbar(props: {
  /** 已登录成员；游客不传 */
  user?: TopbarUser | undefined;
  /** 顶栏右侧是否显示游客登录入口 */
  loginOpen: boolean;
  /** 已登录且有 Key 时，顶栏放「生图工作台」按钮，点进去就是 /image。 */
  workbench?: boolean;
}) {
  const loginHref = useLoginHref();
  return (
    <header class="site-top">
      <div class="wrap site-top__in">
        <a class="brand" href="/">
          <span class="brand__mark-anchor" aria-hidden="true">
            <img class="brand__mark" src="/_owl/owl-eyes.svg" alt="" width="32" height="32" />
            <i class="brand__landing-eye" />
            <i class="brand__landing-eye" />
          </span>
          <span class="brand__name">猫头鹰公益站</span>
        </a>
        <div class="site-top__right">
          <a class="site-top__poker" href="/poker" aria-label="德州扑克牌桌" title="德州扑克">
            <svg viewBox="0 0 24 24" aria-hidden="true">
              <path d="M12 2C9.6 5.6 4 9 4 13.2A5 5 0 0 0 12 18c.2 1.6-.6 2.9-1.6 4h3.2C12.6 20.9 11.8 19.6 12 18a5 5 0 0 0 8-4.8C20 9 14.4 5.6 12 2Z" />
            </svg>
          </a>
          {props.workbench && (
            <a id="workbench-entry" class="btn btn--primary btn--sm site-top__wb" href="/image">
              生图工作台
              <span class="site-top__beta">Beta</span>
            </a>
          )}
          {!props.user && props.loginOpen && (
            <a class="btn btn--primary" href={loginHref} aria-label="用 Discord 登录">
              登录
            </a>
          )}
          {props.user && <Account user={props.user} />}
        </div>
      </div>
    </header>
  );
}
