/** 跨来源运行视图；展示接口实际返回的状态，不推断某种请求一定能被派发。 */
import { fullTime } from "../../../../lib/format";
import { Card, Notice } from "../../../../ui";
import { useAdmin } from "../../data";
import { LoadError, Loading } from "../_kit";
import { Sect } from "../system/forms";
import {
  type ShareAccount,
  type SharePage,
  tokenState,
  type UpstreamAccount,
  type UpstreamPage,
} from "./core";
import { Settlements } from "./settlements";

type Row = { source: string; account: UpstreamAccount | ShareAccount; href: string; official: boolean };

export function UpstreamOverview() {
  const official = useAdmin<UpstreamPage>("/admin/api/upstream");
  const shared = useAdmin<SharePage>("/admin/api/share-upstreams");
  const rows: Row[] = [
    ...(official.data?.accounts ?? []).map((account) => ({
      source: "官方",
      account,
      href: "/admin/upstream/official",
      official: true,
    })),
    ...(shared.data?.accounts ?? []).map((account) => ({
      source: account.donated ? "捐赠" : "拼车",
      account,
      href: account.donated ? "/admin/upstream/donated" : "/admin/upstream/share",
      official: false,
    })),
  ];
  return (
    <Sect>
      <p class="small muted">
        汇总官方、拼车和捐赠来源。状态是网关最近读数；实际派发还受请求类型、成员规则、全站预算和排队保护影响。查看详情可修改对应上游。
      </p>
      {official.data && (
        <p class="small muted">
          官方账号汇总时间：{fullTime(official.data.generated_at)}；拼车及捐赠接口未提供汇总时间。
        </p>
      )}
      {((official.loading && !official.data) || (shared.loading && !shared.data)) && <Loading rows={1} />}
      {official.error && <LoadError error={`官方账号读取失败：${official.error}`} />}
      {shared.error && <LoadError error={`拼车与捐赠读取失败：${shared.error}`} />}
      {rows.length === 0 && official.data && shared.data && <Notice>当前没有上游账号。</Notice>}
      {official.data && <Settlements items={official.data.settlements} csrf={official.data.csrf_token} />}
      {rows.map(({ source, account: a, href, official: isOfficial }) => {
        const status = tokenState(a);
        const free = a.v5_daily_limit === 0 ? "停用" : `${a.v5_used} / ${a.v5_daily_limit}`;
        const share = isOfficial ? null : (a as ShareAccount);
        return (
          <Card key={`${source}-${a.token_id}`} title={`${source} #${a.position}`}>
            <p>
              {status.text} · 基础状态：{a.usable ? "启用且未处于冷却" : "不可用（停用、失效或冷却）"}
            </p>
            <p>
              免费 V5：{free}
              {a.v5_held != null && a.v5_held > 0 ? `（另有 ${a.v5_held} 个在途或未知预留）` : ""}；Anlas：
              {a.allow_anlas ? "允许" : "关闭"}
            </p>
            {share && (
              <p>
                每日 Anlas 上限：
                {share.daily_anlas === 0 ? "不限" : `${share.anlas_used_today} / ${share.daily_anlas}`}；RPM：
                {share.rpm === 0 ? "不限" : share.rpm}；并发：{share.concurrency}
              </p>
            )}
            <a href={href}>查看{source}配置与详情 →</a>
            {" · "}
            <a href={`/admin/logs/usage?upstream=${encodeURIComponent(a.token_id)}`}>查看此上游请求 →</a>
          </Card>
        );
      })}
    </Sect>
  );
}
