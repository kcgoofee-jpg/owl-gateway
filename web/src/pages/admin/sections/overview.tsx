/** 运营入口：只汇总已有接口，不将某个单项状态当作整个服务可用性的结论。 */

import { fullTime } from "../../../lib/format";
import { Card } from "../../../ui";
import { useAdmin } from "../data";
import { SectionHead } from "./_kit";
import type { Settings } from "./limits/core";
import type { LogsResp } from "./logs/types";
import type { MembersResp } from "./members/types";
import { Sect } from "./system/forms";
import type { UpstreamPage } from "./upstream/core";

type OpsStatus = { maintenance: { enabled: boolean; mode?: string } };
export default function Overview() {
  const ops = useAdmin<OpsStatus>("/admin/api/ops");
  const settings = useAdmin<Settings>("/admin/api/settings");
  const members = useAdmin<MembersResp>("/admin/api/members?filter=attention");
  const upstream = useAdmin<UpstreamPage>("/admin/api/upstream");
  const errors = useAdmin<LogsResp>("/admin/api/logs?status=error&per_page=20&page=1");
  return (
    <div class="xa-page">
      <SectionHead title="总览" desc="运营状态与待处理事项；点击各项进入对应页面。" />
      <Sect>
        <p class="small muted">后台数据按手动刷新读取；不同接口读取时间可能不同，读取失败时不会当作正常。</p>
        <Card title="维护状态">
          <p>
            {ops.error
              ? "未知：读取失败"
              : !ops.data
                ? "读取中…"
                : ops.data.maintenance.enabled
                  ? `维护已开启（${ops.data.maintenance.mode ?? "模式未知"}）`
                  : "维护未开启"}
          </p>
          <a href="/admin/system">管理维护模式 →</a>
        </Card>
        <Card title="全站 V5">
          <p>
            {settings.error
              ? "未知：读取失败"
              : !settings.data
                ? "读取中…"
                : settings.data.v5_paused
                  ? "V5 已暂停"
                  : `V5 未暂停；全站每日额度 ${settings.data.budgets.daily_v5 === 0 ? "停用" : settings.data.budgets.daily_v5}`}
          </p>
          <a href="/admin/rules/budget">查看全站预算和运行保护 →</a>
        </Card>
        <Card title="待处理成员">
          <p>
            {members.error
              ? "未知：读取失败"
              : !members.data
                ? "读取中…"
                : `需要处理 ${members.data.attention ?? "未知"} 位`}
          </p>
          <a href="/admin/members">查看成员及处理事项 →</a>
        </Card>
        <Card title="上游与待核对请求">
          <p>
            {upstream.error
              ? "未知：读取失败"
              : !upstream.data
                ? "读取中…"
                : `官方账号 ${upstream.data.accounts.length} 个；结果未知待核对 ${upstream.data.settlements.length} 条`}
          </p>
          <p class="small muted">账号数量不等于可用容量；拼车和捐赠上游请查看运行总览。</p>
          <a href="/admin/upstream/overview">查看上游与待核对请求 →</a>
        </Card>
        <Card title="最近异常">
          {errors.error ? (
            <p>未知：请求日志读取失败。</p>
          ) : !errors.data ? (
            <p>读取中…</p>
          ) : errors.data.logs.length === 0 ? (
            <p>没有查询到失败记录。</p>
          ) : (
            <ul>
              {errors.data.logs.slice(0, 3).map((row) => (
                <li key={row.id}>
                  {fullTime(row.ts)} · {row.issue_label || "请求失败"} ·{" "}
                  <a href={`/admin/logs/usage?rid=${encodeURIComponent(row.rid)}`}>查看请求</a>
                </li>
              ))}
            </ul>
          )}
          <a href="/admin/logs/usage">查看请求日志 →</a>
        </Card>
      </Sect>
    </div>
  );
}
