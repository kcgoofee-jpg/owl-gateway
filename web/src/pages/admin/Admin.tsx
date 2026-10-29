/**
 * 后台外壳：登录门、侧栏导航、路由、全局刷新节拍。
 *
 * 分区组件放在 ./sections/<id>.tsx，默认导出一个无参组件；数据用 useAdmin(url) 取，
 * 它跟着全局「刷新节拍」自动重取（SSE 推送事件时，或 SSE 不可用时页面可见每 15 秒一次）。
 * 站长规定：后台不放刷新按钮。
 */
import { signal } from "@preact/signals";
import {
  Activity,
  Gauge,
  ListTree,
  LogOut,
  Menu as MenuIcon,
  Moon,
  Server,
  SlidersHorizontal,
  Sun,
  Users,
} from "lucide-preact";
import type { ComponentChildren, ComponentType } from "preact";
import { useEffect, useState } from "preact/hooks";
import { LocationProvider, lazy, Route, Router, useLocation } from "preact-iso";
import { ApiError, getJSON, sendJSON } from "../../lib/api";
import { effectiveTheme, themeChoice, toggleTheme } from "../../lib/theme";
import { Button, cx, Field, Skeleton, toast } from "../../ui";
import "./admin.css";
import { SitepaidBanner } from "./SitepaidHints";
import { edits } from "./sections/live";

/* ---------------- 分区注册表 ---------------- */
export type SectionId = "members" | "logs" | "upstream" | "onboarding" | "limits" | "system";

type Section = {
  id: SectionId;
  label: string;
  group: "" | "日常" | "监控" | "设置";
  path: string;
  icon: ComponentType<{ size?: number; "aria-hidden"?: boolean }>;
  view: ComponentType;
};

export const SECTIONS: Section[] = [
  {
    id: "members",
    label: "成员",
    group: "日常",
    path: "/admin/members",
    icon: Users,
    view: lazy(() => import("./sections/members")),
  },
  {
    id: "logs",
    label: "日志",
    group: "日常",
    path: "/admin/logs",
    icon: ListTree,
    view: lazy(() => import("./sections/logs")),
  },
  {
    id: "upstream",
    label: "上游",
    group: "监控",
    path: "/admin/upstream",
    icon: Activity,
    view: lazy(() => import("./sections/upstream")),
  },
  {
    id: "onboarding",
    label: "成员规则",
    group: "设置",
    path: "/admin/onboarding",
    icon: Server,
    view: lazy(() => import("./sections/onboarding")),
  },
  {
    id: "limits",
    label: "运行保护",
    group: "设置",
    path: "/admin/limits",
    icon: SlidersHorizontal,
    view: lazy(() => import("./sections/limits")),
  },
  {
    id: "system",
    label: "系统",
    group: "设置",
    path: "/admin/system",
    icon: Gauge,
    view: lazy(() => import("./sections/system")),
  },
];

/* ---------------- 全局刷新节拍 ---------------- */
/** 每次 +1 都会让所有 useAdmin 重新取数。由 SSE 事件或可见时 15 秒定时器驱动。 */
export const tick = signal(0);
/** 用户正在编辑表单 / 打开对话框时暂停自动刷新，避免把输入冲掉。 */
export const busy = signal(0);

function startTicker(): () => void {
  let es: EventSource | null = null;
  let timer: ReturnType<typeof setInterval> | undefined;
  // 暂停期间（编辑 / 对话框）错过的节拍：恢复时补一次，不用等下一条推送
  let missed = false;
  const bump = () => {
    if (document.hidden || busy.value > 0) {
      missed = true;
      return;
    }
    missed = false;
    tick.value++;
  };
  const stopBusyWatch = busy.subscribe((n) => {
    if (n === 0 && missed && !document.hidden) bump();
  });
  const fallback = () => {
    if (!timer) timer = setInterval(bump, 15_000);
  };
  try {
    es = new EventSource("/admin/api/events");
    es.onmessage = bump;
    // 晚于 3 秒才连上：停掉退回的定时器，不要两边一起刷
    es.onopen = () => {
      clearInterval(timer);
      timer = undefined;
    };
    es.onerror = () => {
      if (es?.readyState === EventSource.CLOSED) fallback();
    };
    // 旧版后端没有 SSE：3 秒内没连上就退回定时刷新
    setTimeout(() => {
      if (!es || es.readyState !== EventSource.OPEN) fallback();
    }, 3000);
  } catch {
    fallback();
  }
  const onVis = () => !document.hidden && bump();
  document.addEventListener("visibilitychange", onVis);
  return () => {
    es?.close();
    clearInterval(timer);
    stopBusyWatch();
    document.removeEventListener("visibilitychange", onVis);
  };
}

/** 同一拍里同一个地址只取一次（总览、角标、日志页常常同时要 /errors、/status、/jobs）。 */
const inflight = new Map<string, { t: number; p: Promise<unknown> }>();
export function getShared<T>(url: string, t = tick.value): Promise<T> {
  const hit = inflight.get(url);
  if (hit && hit.t === t) return hit.p as Promise<T>;
  const p = getJSON<T>(url);
  inflight.set(url, { t, p });
  // 失败的不留：下一次（同一拍里别的组件）重新取
  p.catch(() => inflight.get(url)?.p === p && inflight.delete(url));
  return p;
}

/** 取后台数据：url 为 null 时不取。返回 {data, error, loading}；之后跟着全局刷新节拍自动重取。 */
export function useAdmin<T>(url: string | null): {
  data: T | null;
  error: string | null;
  loading: boolean;
} {
  const [state, setState] = useState<{ data: T | null; error: string | null; loading: boolean }>({
    data: null,
    error: null,
    loading: !!url,
  });
  const t = tick.value;
  useEffect(() => {
    if (!url) return;
    let alive = true;
    getShared<T>(url, t)
      .then((data) => alive && setState({ data, error: null, loading: false }))
      .catch((e: unknown) => {
        if (!alive) return;
        if (e instanceof ApiError && e.status === 401) authed.value = false;
        setState((s) => ({ ...s, error: e instanceof Error ? e.message : String(e), loading: false }));
      });
    return () => {
      alive = false;
    };
  }, [url, t]);
  return state;
}

/** 后台写操作：成功 toast，失败 toast 错误；返回结果或 null。成功后触发一次全局刷新。 */
export async function act<T = unknown>(
  method: "POST" | "PUT" | "PATCH" | "DELETE",
  url: string,
  body?: unknown,
  okText = "已保存",
): Promise<T | null> {
  try {
    const r = await sendJSON<T>(method, url, body);
    if (okText) toast(okText);
    edits.value++;
    tick.value++;
    return r;
  } catch (e) {
    if (e instanceof ApiError && e.status === 401) authed.value = false;
    toast(e instanceof Error ? e.message : String(e), { error: true });
    return null;
  }
}

/* ---------------- 登录 ---------------- */
const authed = signal<boolean | null>(null);

function Login() {
  const [pw, setPw] = useState("");
  const [err, setErr] = useState("");
  const [pending, setPending] = useState(false);
  return (
    <main class="admin-login">
      <form
        class="card admin-login__card"
        onSubmit={async (e) => {
          e.preventDefault();
          setPending(true);
          setErr("");
          try {
            await sendJSON("POST", "/admin/api/login", { password: pw });
            authed.value = true;
          } catch (x) {
            setErr(x instanceof Error ? x.message : String(x));
          } finally {
            setPending(false);
          }
        }}
      >
        <h1 class="admin-login__title">
          <span aria-hidden="true">🦉</span> 猫头鹰公益站 · 后台
        </h1>
        {/* 隐藏的用户名字段：让密码管理器能把密码和本站关联保存 */}
        <input type="text" name="username" autoComplete="username" value="admin" hidden readOnly />
        <Field label="后台密码">
          <input
            class="input"
            type="password"
            name="password"
            autoComplete="current-password"
            required
            value={pw}
            onInput={(e) => setPw((e.target as HTMLInputElement).value)}
          />
        </Field>
        {err && (
          <p class="admin-login__err" role="alert">
            {err}
          </p>
        )}
        <Button type="submit" variant="primary" size="lg" disabled={pending || !pw}>
          {pending ? "登录中…" : "登录"}
        </Button>
      </form>
    </main>
  );
}

/**
 * 鉴权探针失败（断网 / 5xx / 超时）：留在后台壳里给重试，**不要**当成「没登录」回登录页。
 * 只有 401 才是真的掉登录（见 Admin() 的 catch）。
 */
function AdminDataError(props: { error: string | null; onRetry: () => void }) {
  return (
    <div class="admin-fail" role="alert">
      <h1>后台数据读取失败</h1>
      <p>网络或服务端暂时没有响应，登录状态没有变化。恢复后点「重试」即可，不用重新登录。</p>
      {props.error && <p class="small muted">{props.error}</p>}
      <div>
        <Button variant="primary" onClick={props.onRetry}>
          重试
        </Button>
      </div>
    </div>
  );
}

/** 未知的 /admin/* 路径：保留侧栏导航，不再静默渲染成员页。 */
function NotFound() {
  const { path, route } = useLocation();
  return (
    <div class="admin-fail">
      <h1>页面不存在</h1>
      <p>
        没有 <code>{path}</code> 这个后台分区。
      </p>
      <p class="small muted">地址可能写错了，或者这个页面已经被删掉。旧地址会跳到新位置。</p>
      <div>
        <Button variant="primary" onClick={() => route("/admin/members")}>
          回到成员页
        </Button>
      </div>
    </div>
  );
}

/* ---------------- 外壳 ---------------- */
const drawerOpen = signal(false);

function Nav() {
  const { path } = useLocation();
  const active = (s: Section) =>
    path.startsWith(s.path) || (s.id === "members" && (path === "/admin" || path === "/admin/"));
  let lastGroup = "";
  return (
    <nav class="admin-nav" aria-label="后台导航">
      {SECTIONS.map((s) => {
        const header = s.group && s.group !== lastGroup ? s.group : "";
        lastGroup = s.group;
        const Icon = s.icon;
        return (
          <div key={s.id}>
            {header && <div class="admin-nav__group">{header}</div>}
            <a
              href={s.path}
              class={cx("admin-nav__item", active(s) && "is-active")}
              aria-current={active(s) ? "page" : undefined}
              onClick={() => {
                drawerOpen.value = false;
              }}
            >
              <Icon size={17} aria-hidden={true} />
              <span>{s.label}</span>
            </a>
          </div>
        );
      })}
    </nav>
  );
}

function Shell(props: { children: ComponentChildren; dataLive?: boolean }) {
  const dataLive = props.dataLive ?? true;
  // 鉴权探针还没成功时不要开轮询 / SSE：避免后台壳在错误态里继续打接口
  useEffect(() => {
    if (!dataLive) return;
    return startTicker();
  }, [dataLive]);
  const dark = themeChoice.value ? themeChoice.value === "dark" : effectiveTheme() === "dark";
  const ThemeIcon = dark ? Sun : Moon;
  return (
    <div class={cx("admin", drawerOpen.value && "drawer-open")}>
      <aside class="admin-side">
        <div class="admin-brand">
          <span aria-hidden="true">🦉</span> 猫头鹰公益站
        </div>
        <Nav />
        <div class="admin-side__foot">
          <Button variant="ghost" onClick={toggleTheme}>
            <ThemeIcon size={16} aria-hidden="true" />
            {dark ? "浅色" : "深色"}
          </Button>
          <Button
            variant="ghost"
            onClick={async () => {
              await sendJSON("POST", "/admin/api/logout").catch(() => null);
              authed.value = false;
            }}
          >
            <LogOut size={16} aria-hidden="true" />
            退出
          </Button>
        </div>
      </aside>
      <button
        type="button"
        class="admin-scrim"
        aria-label="关闭菜单"
        tabIndex={-1}
        onClick={() => {
          drawerOpen.value = false;
        }}
      />
      <div class="admin-main">
        <header class="admin-top">
          <Button
            variant="ghost"
            icon
            aria-label="打开菜单"
            onClick={() => {
              drawerOpen.value = true;
            }}
          >
            <MenuIcon size={18} aria-hidden="true" />
          </Button>
          <span class="admin-brand">🦉 猫头鹰公益站</span>
          <span class="spacer" />
        </header>
        <main class="admin-content">
          {dataLive && <SitepaidBanner />}
          {props.children}
        </main>
      </div>
    </div>
  );
}

export function Admin() {
  const [attempt, setAttempt] = useState(0);
  const [authError, setAuthError] = useState<string | null>(null);
  const [probing, setProbing] = useState(true);

  useEffect(() => {
    let alive = true;
    setProbing(true);
    setAuthError(null);
    getJSON("/admin/api/me")
      .then(() => {
        if (alive) authed.value = true;
      })
      .catch((e: unknown) => {
        if (!alive) return;
        // 只有 401 才是真的没登录；断网 / 5xx 不能清掉已登录态
        if (e instanceof ApiError && e.status === 401) authed.value = false;
        else setAuthError(e instanceof Error ? e.message : String(e));
      })
      .finally(() => {
        if (alive) setProbing(false);
      });
    return () => {
      alive = false;
    };
  }, [attempt]);

  const retry = () => {
    setAuthError(null);
    setProbing(true);
    setAttempt((n) => n + 1);
  };

  if (authed.value === false) return <Login />;
  if (authed.value !== true) {
    return (
      <LocationProvider>
        <Shell dataLive={false}>
          {probing ? (
            <main class="admin-login">
              <Skeleton w="240px" h="24px" />
            </main>
          ) : (
            <AdminDataError error={authError} onRetry={retry} />
          )}
        </Shell>
      </LocationProvider>
    );
  }
  const MembersView = SECTIONS[0]?.view as ComponentType;
  return (
    <LocationProvider>
      <Shell>
        <Router>
          {SECTIONS.flatMap((s) => [
            // preact-iso 的 `/*` 至少要多一段路径：分区根路径单独注册一条
            <Route key={s.id} path={s.path} component={s.view} />,
            <Route key={`${s.id}-sub`} path={`${s.path}/*`} component={s.view} />,
          ])}
          {/* 总览页 10-23 删除：/admin 直接是成员页 */}
          <Route key="root" path="/admin" component={MembersView} />
          <Route key="root-slash" path="/admin/" component={MembersView} />
          {/* 不认识的 /admin/*：404 视图，不再静默渲染成员页 */}
          <Route default component={NotFound} />
        </Router>
      </Shell>
    </LocationProvider>
  );
}
