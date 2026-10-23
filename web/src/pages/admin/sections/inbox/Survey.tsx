/** 成员问卷（/survey 网页版）：汇总 + 明细。一人一份，可覆盖。 */
import { fullTime } from "../../../../lib/format";
import { Skeleton } from "../../../../ui";
import { useAdmin } from "../../Admin";
import { LoadError, Stat, TableWrap, useShowMore } from "../_kit";
import type { SurveyResp, SurveyRow } from "../logs/types";
import { Empty, NARROW, useMedia } from "../members/shared";
import { surveyStats } from "./feedbackStats";

function Improve(props: { r: SurveyRow }) {
  const { r } = props;
  return (
    <>
      {r.improve || "—"}
      {r.improve_other && <div class="small muted">补充：{r.improve_other}</div>}
    </>
  );
}

export function Survey() {
  const narrow = useMedia(NARROW);
  const { data, error } = useAdmin<SurveyResp>("/admin/api/survey");
  const rows = data?.rows ?? [];
  const { shown, more } = useShowMore(rows);
  if (error && !data) return <LoadError error={error} />;
  if (!data) return <Skeleton h="240px" />;
  const s = surveyStats(rows);
  const who = (r: SurveyRow) => r.name || r.discord_id || "—";

  return (
    <div class="stack">
      {rows.length > 0 && (
        <div class="stats">
          <Stat label="已填" value={`${s.total} 人`} hint="在册成员里的一份答卷" />
          <Stat
            label="最常用客户端"
            value={<span class="lg-statv">{s.topClient ? s.topClient[0] : "暂无"}</span>}
            hint={s.topClient ? `${s.topClient[1]} 人` : ""}
          />
          <Stat
            label="Top 改进诉求"
            value={<span class="lg-statv">{s.topImprove ? s.topImprove[0] : "暂无"}</span>}
            hint={s.topImprove ? `${s.topImprove[1]} 人提到` : ""}
          />
        </div>
      )}
      <div class="card lg-listcard">
        {rows.length === 0 ? (
          <Empty title="还没有成员填过问卷">
            成员登录后，在首页页脚的「使用体验问卷」里填写（/survey）。
          </Empty>
        ) : narrow ? (
          <ul class="lg-cards" aria-label="成员问卷">
            {shown.map((r, i) => (
              <li key={`${r.ts}-${i}`} class="lg-card">
                <div class="lg-card__top">
                  <b>{who(r)}</b>
                  <span class="small muted">{fullTime(r.ts)}</span>
                </div>
                <div class="small">
                  <span class="muted">客户端：</span>
                  {r.client || "—"}
                </div>
                <div class="small">
                  <span class="muted">希望改进：</span>
                  <Improve r={r} />
                </div>
              </li>
            ))}
          </ul>
        ) : (
          <TableWrap label="成员问卷">
            <table class="table lg-table">
              <caption class="sr-only">成员问卷</caption>
              <thead>
                <tr>
                  <th scope="col">时间</th>
                  <th scope="col">成员</th>
                  <th scope="col">客户端</th>
                  <th scope="col">希望改进</th>
                </tr>
              </thead>
              <tbody>
                {shown.map((r, i) => (
                  <tr key={`${r.ts}-${i}`}>
                    <td class="nowrap">{fullTime(r.ts)}</td>
                    <td>{who(r)}</td>
                    <td>{r.client || "—"}</td>
                    <td class="lg-ans">
                      <Improve r={r} />
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </TableWrap>
        )}
        {more}
      </div>
    </div>
  );
}
