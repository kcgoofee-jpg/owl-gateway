/**
 * 后台外壳：登录门、侧栏导航、路由、全局刷新节拍。
 *
 * 分区组件放在 ./sections/<id>.tsx，默认导出一个无参组件；数据用 useAdmin(url) 取，
 * 跨分区缓存秒显，只在手动刷新 / 写操作后重取。
 * 站长 2026-11-14 改手动刷新：顶栏放刷新按钮，去掉 SSE / 15s 自动轮询（原「后台不放刷新按钮」规定作废）。
 */
import { signal } from "@preact/signals";
import {
  Activity,
  ListTree,
  LogOut,
  Menu as MenuIcon,
  RefreshCw,
  SlidersHorizontal,
  Users,
} from "lucide-preact";
import type { ComponentChildren, ComponentType } from "preact";
import { Suspense } from "preact/compat";
import { useEffect, useState } from "preact/hooks";
import { LocationProvider, lazy, Route, Router, useLocation } from "preact-iso";
import { ApiError, getJSON, sendJSON } from "../../lib/api";
import { Button, cx, Field, Skeleton, toast } from "../../ui";
import "./admin.css";
import { authed, endSession, refresh } from "./data";

/* ---------------- 分区注册表 ---------------- */
export type SectionId = "members" | "logs" | "upstream" | "settings";

type Section = {
  id: SectionId;
  label: string;
  path: string;
  icon: ComponentType<{ size?: number; "aria-hidden"?: boolean }>;
  view: ComponentType;
};

/** 只有四个分区：日常看的两个 + 需要改的两个。设置不再拆成三页。 */
export const SECTIONS: Section[] = [
  {
    id: "members",
    label: "成员",
    path: "/admin/members",
    icon: Users,
    view: lazy(() => import("./sections/members")),
  },
  {
    id: "upstream",
    label: "上游",
    path: "/admin/upstream",
    icon: Activity,
    view: lazy(() => import("./sections/upstream")),
  },
  {
    id: "logs",
    label: "日志",
    path: "/admin/logs",
    icon: ListTree,
    view: lazy(() => import("./sections/logs")),
  },
  {
    id: "settings",
    label: "设置",
    path: "/admin/settings",
    icon: SlidersHorizontal,
    view: lazy(() => import("./sections/settings")),
  },
];

/** 旧地址（成员规则 / 运行保护 / 系统已并入设置）：进来就换到新地址，不留历史。 */
const LEGACY_PATH = /^\/admin\/(onboarding|limits|system)(\/|$)/;

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

/** 未知的 /admin/*：旧分区地址换到设置页，其余给 404。 */
function Fallback() {
  const { path, route } = useLocation();
  const legacy = LEGACY_PATH.test(path);
  useEffect(() => {
    if (legacy) route("/admin/settings", true);
  }, [legacy, route]);
  if (legacy) return <SectionLoading />;
  return <NotFound />;
}

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

/** 分区 chunk 加载中的骨架（有侧栏在，宽度对齐内容区即可）。 */
function SectionLoading() {
  return (
    <div class="xa-page" aria-busy="true">
      <Skeleton w="160px" h="28px" />
      <Skeleton h="56px" />
      <Skeleton h="56px" />
      <Skeleton h="56px" />
    </div>
  );
}

function Nav() {
  const { path } = useLocation();
  const active = (s: Section) =>
    path.startsWith(s.path) || (s.id === "members" && (path === "/admin" || path === "/admin/"));
  return (
    <nav class="admin-nav" aria-label="后台导航">
      {SECTIONS.map((s) => {
        const Icon = s.icon;
        return (
          <a
            key={s.id}
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
        );
      })}
    </nav>
  );
}

function Shell(props: { children: ComponentChildren }) {
  return (
    <div class={cx("admin", drawerOpen.value && "drawer-open")}>
      <aside class="admin-side">
        <div class="admin-brand">
          <span aria-hidden="true">🦉</span> 猫头鹰公益站
        </div>
        <Nav />
        <div class="admin-side__foot">
          <Button
            variant="ghost"
            onClick={async () => {
              try {
                await sendJSON("POST", "/admin/api/logout");
                endSession();
              } catch (e) {
                if (e instanceof ApiError && e.status === 401) endSession();
                else toast(e instanceof Error ? e.message : "退出失败", { error: true });
              }
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
          <Button
            variant="ghost"
            icon
            aria-label="刷新"
            title="刷新当前页数据（后台不自动刷新；表单里没保存的修改会保留）"
            onClick={() => {
              refresh();
              toast("已刷新");
            }}
          >
            <RefreshCw size={18} aria-hidden="true" />
          </Button>
        </header>
        <main class="admin-content">{props.children}</main>
      </div>
    </div>
  );
}

export function Admin() {
  // 不再先等 /admin/api/me 再渲染：多一个往返就是多 0.5～1 秒白屏（弱网下最明显）。
  // 直接渲染分区，任何接口回 401 时 data.ts 会 endSession() 退回登录页。
  useEffect(() => {
    getJSON("/admin/api/me")
      .then(() => {
        authed.value = true;
      })
      .catch((e: unknown) => {
        // 只有 401 才是真的没登录；断网 / 5xx 不能清掉已登录态
        if (e instanceof ApiError && e.status === 401) endSession();
      });
  }, []);

  if (authed.value === false) return <Login />;
  const MembersView = SECTIONS[0]?.view as ComponentType;
  return (
    <LocationProvider>
      <Shell>
        {/* 懒加载分区切换时显示目标骨架，而不是让旧分区继续冒充新页面
            （旧版在 chunk 挂起期间保留旧内容：侧栏已选「日志」，正文还是「成员」）。 */}
        <Suspense fallback={<SectionLoading />}>
          <Router>
            {SECTIONS.flatMap((s) => [
              // preact-iso 的 `/*` 至少要多一段路径：分区根路径单独注册一条
              <Route key={s.id} path={s.path} component={s.view} />,
              <Route key={`${s.id}-sub`} path={`${s.path}/*`} component={s.view} />,
            ])}
            {/* 总览页 10-23 删除：/admin 直接是成员页 */}
            <Route key="root" path="/admin" component={MembersView} />
            <Route key="root-slash" path="/admin/" component={MembersView} />
            {/* 不认识的 /admin/*：旧地址换到设置页，其余 404 */}
            <Route default component={Fallback} />
          </Router>
        </Suspense>
      </Shell>
    </LocationProvider>
  );
}
