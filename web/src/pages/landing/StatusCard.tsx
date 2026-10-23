import { useEffect, useRef, useState } from "preact/hooks";
import { cx, Skeleton } from "../../ui";
import { useStore } from "./ctx";
import { createEstimator, dailyMeters, dailyUsage, deriveStatus } from "./status";
import { StatRows } from "./Usage";

/**
 * 全站实时状态：一句话（能不能马上出图 + 原因）+ 全站今日用量。所有访客都看得到。
 * 游客登录入口统一放在顶栏。
 */
export function StatusCard(props: { class?: string }) {
  const { live, me, connection } = useStore();
  const estimator = useRef(createEstimator());
  // 排队倒计时每秒走一格（live 没有变化时也要动）
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = setInterval(() => {
      if (!document.hidden) setNow(Date.now());
    }, 1000);
    return () => clearInterval(t);
  }, []);

  const data = live.value;
  const who = me.value;
  const timing = who?.logged_in ? who.timing : null;
  const view = deriveStatus(data, {
    estimate: estimator.current,
    nowMs: now,
    avgDurS: timing?.avg_dur_s ?? null,
  });
  const meters = dailyMeters(data);
  const usage = dailyUsage(data);

  // 读屏只在「状态本身」变化时播报（不含每秒变化的倒计时）
  const announce = view?.announce ?? "";
  const [spoken, setSpoken] = useState("");
  useEffect(() => {
    setSpoken(announce);
  }, [announce]);

  return (
    <section class={cx("lp-status lp-rows", props.class)} aria-label="当前状态">
      <div class="lp-status__line">
        {view ? (
          <p class="lp-status__text">
            {/* 圆点放在标题里：标题折行时跟着第一个字走，不会单独留在上一行 */}
            <strong class="lp-status__head">
              <span class={cx("dot", "dot--pulse", "lp-dot", `lp-dot--${view.tone}`)} aria-hidden="true" />
              {view.headline}
            </strong>
            {view.detail && (
              <span class="lp-status__detail">
                <span class="lp-status__sep"> · </span>
                {view.detail}
              </span>
            )}
          </p>
        ) : (
          <span class="lp-status__wait" aria-busy="true">
            <Skeleton h="32px" />
          </span>
        )}
        {/* 读屏播报和断线提示放在第一行里：卡片的直接子元素就是四条行线，不能多出来 */}
        <p class="sr-only" role="status" aria-live="polite" aria-atomic="true">
          {spoken}
        </p>
        {connection.value === "down" && (
          <p class="muted small lp-status__down">{data ? "实时数据暂时读不到" : "正在重新连接…"}</p>
        )}
      </div>
      {/* 还没读到数据时不画仪表（否则会先闪一下「0」） */}
      {data ? (
        <StatRows
          total={meters.total}
          v5={meters.v5}
          generated={{ total: usage.total.used, v5: usage.v5.used }}
        />
      ) : (
        <Skeleton h="56px" />
      )}
    </section>
  );
}
