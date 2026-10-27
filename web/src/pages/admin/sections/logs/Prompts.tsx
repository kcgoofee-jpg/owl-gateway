/** 提示词及生成参数：按成员查看最近 30 天的记录。 */
import { useState } from "preact/hooks";
import { fullTime } from "../../../../lib/format";
import { Button, copyText, SearchMultiSelect, Skeleton } from "../../../../ui";
import { useAdmin } from "../../Admin";
import { LoadError } from "../_kit";
import { memberKeyOptions } from "../members/logic";
import { Empty, Pager, usePageData } from "../members/shared";
import type { Member } from "../members/types";
import { allPromptText, negativeText, paramsLine, parseExtra, positiveText, statusText } from "./promptData";
import { intParam, useQueryState } from "./query";
import type { PromptsResp } from "./types";

const SIZES = [24, 48, 96];
export function Prompts() {
  const { query, set } = useQueryState();
  const page = intParam(query.page, 1);
  const per = intParam(query.per, 24, SIZES);
  const selected = /^\d+(,\d+)*(;\d+(,\d+)*)*$/.test(query.key ?? "") ? (query.key as string).split(";") : [];
  const members = useAdmin<{ members: Member[] }>("/admin/api/members");
  const qs = new URLSearchParams({ page: String(page), per_page: String(per), full: "1" });
  if (selected.length) qs.set("key_id", selected.join(","));
  const { data, error } = usePageData<PromptsResp>(`/admin/api/prompts?${qs}`);
  const [copied, setCopied] = useState<number | null>(null);
  return (
    <div class="stack">
      <p class="small muted">提示词与生成参数保留 30 天，不保存生成图片。</p>
      <SearchMultiSelect
        label="成员（可多选）"
        values={selected}
        allLabel="全部成员"
        options={memberKeyOptions(members.data?.members ?? [])}
        onChange={(vs) => set({ key: vs.join(";") || null, page: null })}
      />
      {selected.length > 0 && (
        <div class="xa-chips">
          {selected.map((value) => (
            <span class="xa-chip" key={value}>
              {memberKeyOptions(members.data?.members ?? []).find((m) => m.value === value)?.label ?? value}
              <button
                type="button"
                aria-label="取消选择成员"
                onClick={() =>
                  set({ key: selected.filter((v) => v !== value).join(";") || null, page: null })
                }
              >
                ×
              </button>
            </span>
          ))}
        </div>
      )}
      {error && !data ? (
        <LoadError error={error} />
      ) : !data ? (
        <Skeleton />
      ) : data.items.length === 0 ? (
        <Empty title="暂无提示词记录" />
      ) : (
        <>
          <ul class="lg-alist" aria-label="提示词记录">
            {data.items.map((item) => {
              const extra = parseExtra(item.extra);
              const text = allPromptText(item, extra);
              return (
                <li key={item.id}>
                  <details class="prompt-record">
                    <summary>
                      {item.key_name || "已删除的 Key"} · {fullTime(item.ts)} · {item.model} ·{" "}
                      {statusText(item.status)}
                    </summary>
                    <p class="small muted">{paramsLine(extra)}</p>
                    <div class="lg-blk">
                      <span class="small muted">正面及角色提示词</span>
                      <pre class="lg-blk__t">{positiveText(item, extra) || "无"}</pre>
                    </div>
                    <div class="lg-blk">
                      <span class="small muted">负面及角色提示词</span>
                      <pre class="lg-blk__t">{negativeText(item, extra) || "无"}</pre>
                    </div>
                    <div class="lg-blk">
                      <span class="small muted">生成参数与角色坐标</span>
                      <pre class="lg-blk__t">{JSON.stringify(extra, null, 2)}</pre>
                    </div>
                    <Button
                      size="sm"
                      disabled={!text}
                      onClick={async () => {
                        if (await copyText(text)) setCopied(item.id);
                      }}
                    >
                      {copied === item.id ? "已复制" : "复制提示词"}
                    </Button>
                  </details>
                </li>
              );
            })}
          </ul>
          <Pager
            page={data.page}
            pages={data.pages}
            total={data.total}
            perPage={data.per_page}
            sizes={SIZES}
            onPage={(n) => set({ page: String(n) })}
            onSize={(n) => set({ per: String(n), page: null })}
          />
        </>
      )}
    </div>
  );
}
