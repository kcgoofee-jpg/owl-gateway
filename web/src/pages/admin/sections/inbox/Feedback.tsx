/** 成员反馈：Discord /反馈 弹窗收集的一段话。一条一行：时间 · 成员 · 内容，可按成员或内容搜索、导出 CSV。
 *  流量低：收在问卷页底部的一个折叠区里，默认收起；标题带上总数（来自 /admin/api/feedback）。 */
import { Download } from "lucide-preact";
import { useState } from "preact/hooks";
import { fullTime } from "../../../../lib/format";
import { Button, Skeleton } from "../../../../ui";
import { useAdmin } from "../../Admin";
import { LoadError, TableWrap, useShowMore } from "../_kit";
import type { FeedbackItem, FeedbackResp } from "../logs/types";
import { downloadFile, Empty, NARROW, useDebounced, useMedia } from "../members/shared";
import { filterFeedback, splitTail } from "./feedbackStats";

/** 内容：成员写的话（保留换行）；旧版选择题的记录是一行摘要，用弱化色；什么都没有显示「—」。
 *  末尾 4 个字绑在一起不折行：不管成员写多长，最后一行都不会只剩一两个字（排版硬规则，见 e2e/layout.spec.ts）。 */
function Text(props: { it: FeedbackItem }) {
  const { it } = props;
  if (!it.text) return <span class="muted">—</span>;
  const [head, tail] = splitTail(it.text, 4);
  return (
    <div class={it.legacy ? "lg-fbtext muted" : "lg-fbtext"}>
      {head}
      {tail && <span class="lg-fbtail">{tail}</span>}
    </div>
  );
}

function Who(props: { it: FeedbackItem }) {
  const { it } = props;
  const label = it.key_name || it.username || it.discord_id || "—";
  return it.key_id ? (
    <a href={`/admin/logs/usage?key=${it.key_id}`} title="查看这位成员的请求日志">
      {label}
    </a>
  ) : (
    label
  );
}

export function Feedback() {
  const narrow = useMedia(NARROW);
  const { data, error } = useAdmin<FeedbackResp>("/admin/api/feedback");
  const [search, setSearch] = useState("");
  const dq = useDebounced(search, 200);
  const [exporting, setExporting] = useState(false);

  const items = data?.items ?? [];
  const rows = filterFeedback(items, dq);
  const { shown, more } = useShowMore(rows);

  return (
    <details class="card inbox-fb">
      <summary>文字反馈{data ? `（${items.length} 条）` : ""}</summary>
      <div class="stack">
        {error && !data ? (
          <LoadError error={error} />
        ) : !data ? (
          <Skeleton h="240px" />
        ) : (
          <>
            <div class="xa-toolbar">
              <label class="field-inline">
                <span class="field-inline__k">搜索</span>
                <input
                  class="input"
                  type="search"
                  placeholder="成员或内容"
                  value={search}
                  onInput={(e) => setSearch(e.currentTarget.value)}
                />
              </label>
              <span class="spacer" />
              <Button
                disabled={exporting}
                title="下载全部反馈（CSV）"
                onClick={async () => {
                  setExporting(true);
                  await downloadFile("/admin/api/feedback/export", "owl-feedback.csv");
                  setExporting(false);
                }}
              >
                <Download size={15} aria-hidden="true" />
                导出 CSV
              </Button>
            </div>

            <div class="card lg-listcard">
              {rows.length === 0 ? (
                <Empty title={items.length ? "未找到符合条件的反馈" : "还没有反馈"}>
                  {items.length ? "换个关键字试试。" : "成员在 Discord 里用 /反馈 写几句话后会出现在这里。"}
                </Empty>
              ) : narrow ? (
                <ul class="lg-cards" aria-label="成员反馈">
                  {shown.map((it) => (
                    <li key={it.id} class="lg-card">
                      <div class="lg-card__top">
                        <b>
                          <Who it={it} />
                        </b>
                        <span class="small muted">{fullTime(it.ts)}</span>
                      </div>
                      <Text it={it} />
                    </li>
                  ))}
                </ul>
              ) : (
                <TableWrap label="成员反馈">
                  <table class="table lg-table">
                    <caption class="sr-only">成员反馈</caption>
                    <thead>
                      <tr>
                        <th scope="col">时间</th>
                        <th scope="col">成员</th>
                        <th scope="col">内容</th>
                      </tr>
                    </thead>
                    <tbody>
                      {shown.map((it) => (
                        <tr key={it.id}>
                          <td class="nowrap">{fullTime(it.ts)}</td>
                          <td>
                            <Who it={it} />
                          </td>
                          <td class="lg-fbcell">
                            <Text it={it} />
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </TableWrap>
              )}
              {more}
            </div>
          </>
        )}
      </div>
    </details>
  );
}
