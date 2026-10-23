/**
 * 带子页的分区：每个子页是一条路由 <base>/<tab>，筛选条件放在查询串里，可以直接收藏或分享链接。
 * <base> 本身或不认识的子路径 replace 到第一个子页（不留历史）。
 */
import type { ComponentChildren, ComponentType } from "preact";
import { useEffect } from "preact/hooks";
import { useLocation } from "preact-iso";
import { cx } from "../../../ui";
import { SectionHead } from "./_kit";
import "./logs.css";

export type Tab = { id: string; label: string; view: ComponentType; badge?: ComponentChildren };

/** 地址是否在这个分区之下（<base> 或 <base>/…） */
export const inSection = (base: string, path: string): boolean =>
  path === base || path.startsWith(`${base}/`);

export function TabPage(props: { title: string; base: string; label: string; tabs: Tab[] }) {
  const { path, route } = useLocation();
  const seg = path.slice(props.base.length + 1).split("/")[0] ?? "";
  const tab = props.tabs.find((t) => t.id === seg);
  const first = props.tabs[0]?.id ?? "";
  // 只在真的还在这个分区下才跳：各页是懒加载的，点别的栏目时新页面还没加载完，旧页会带着「已经是新地址」的 path
  // 多渲染一阵——这时 tab 是空的，不加这个判断就会把人又跳回来（「点别的栏目跳转失败」）
  useEffect(() => {
    if (!tab && inSection(props.base, path)) route(`${props.base}/${first}`, true);
  }, [tab, path, route, props.base, first]);
  const View = tab?.view;
  return (
    <div class="xa-page">
      <SectionHead title={props.title} />
      <nav class="lg-tabs" aria-label={props.label}>
        {props.tabs.map((t) => (
          <a
            key={t.id}
            href={`${props.base}/${t.id}`}
            class={cx("lg-tab", t.id === tab?.id && "is-active")}
            aria-current={t.id === tab?.id ? "page" : undefined}
          >
            {t.label}
            {t.badge}
          </a>
        ))}
      </nav>
      {View && <View key={tab.id} />}
    </div>
  );
}
