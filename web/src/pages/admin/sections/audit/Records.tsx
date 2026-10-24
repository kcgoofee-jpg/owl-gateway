/** 生成记录：成员生成过的图（缩略图画廊 / 列表）+ 提示词与参数，点开大图查看，可导出某成员的全部原图。 */
import { ChevronLeft, ChevronRight, Download, ImageOff, X } from "lucide-preact";
import { useEffect, useRef, useState } from "preact/hooks";
import { fullTime, rel } from "../../../../lib/format";
import { Badge, Button, copyText, cx, Dialog, SearchMultiSelect, Skeleton } from "../../../../ui";
import { useAdmin } from "../../Admin";
import { LoadError } from "../_kit";
import { FreshBar, useFrozenList, useScrolledPast } from "../live";
import { kindLabel, shortModel } from "../logs/format";
import { intParam, useQueryState } from "../logs/query";
import type { AuditItem, AuditResp } from "../logs/types";
import { memberKeyOptions, memberKeyValue } from "../members/logic";
import { downloadFile, Empty, Pager, usePageData } from "../members/shared";
import type { Member } from "../members/types";
import {
  allPromptText,
  aspectOf,
  hasPicture,
  infoLine,
  negativeText,
  paramsLine,
  parseExtra,
  positiveText,
  statusText,
  statusTone,
} from "./auditData";

const SIZES = [24, 48, 96];
type View = "grid" | "list";
const VIEWS: Array<{ id: View; label: string }> = [
  { id: "grid", label: "画廊" },
  { id: "list", label: "列表" },
];

const thumbUrl = (id: number) => `/admin/api/audit/${id}/thumb`;

function AuditImage(props: { src: string; alt: string; lazy?: boolean }) {
  const [failed, setFailed] = useState(false);
  if (failed)
    return (
      <span class="muted small" role="img" aria-label="图片加载失败">
        图片暂不可用
      </span>
    );
  return (
    <img
      src={props.src}
      alt={props.alt}
      loading={props.lazy ? "lazy" : "eager"}
      decoding="async"
      onError={() => setFailed(true)}
    />
  );
}

/* ---------------- 复制按钮（框里的反馈写在按钮上，不依赖 toast） ---------------- */
function CopyBtn(props: { text: string; label: string }) {
  const [state, setState] = useState<"" | "ok" | "fail">("");
  return (
    <Button
      size="sm"
      onClick={async () => {
        const ok = await copyText(props.text);
        setState(ok ? "ok" : "fail");
        setTimeout(() => setState(""), 1800);
      }}
    >
      {state === "ok" ? "已复制" : state === "fail" ? "复制失败" : props.label}
    </Button>
  );
}

function PromptBlock(props: { title: string; text: string; copy?: boolean }) {
  if (!props.text) return null;
  return (
    <div class="lg-blk">
      <div class="lg-blk__h">
        <span class="small muted">{props.title}</span>
        {props.copy !== false && <CopyBtn text={props.text} label="复制" />}
      </div>
      <p class="lg-blk__t">{props.text}</p>
    </div>
  );
}

/* ---------------- 大图查看：图为主，下方一行参数 + 两个复制按钮（提示词原文不展示） ---------------- */
function Lightbox(props: {
  items: AuditItem[];
  index: number;
  onIndex: (i: number) => void;
  onClose: () => void;
}) {
  const it = props.items[props.index];
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.target as HTMLElement | null)?.closest("input, textarea, select")) return;
      if (e.key === "ArrowLeft" && props.index > 0) {
        e.preventDefault();
        props.onIndex(props.index - 1);
      } else if (e.key === "ArrowRight" && props.index < props.items.length - 1) {
        e.preventDefault();
        props.onIndex(props.index + 1);
      }
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [props.index, props.items.length, props.onIndex]);
  if (!it) return null;
  const ex = parseExtra(it.extra);
  const pos = positiveText(it, ex);
  const neg = negativeText(it, ex);
  return (
    <Dialog
      open
      wide
      title={`${it.key_name || "已删除的 Key"} 的生成记录`}
      onClose={props.onClose}
      footer={
        <>
          <span class="small muted">
            {props.index + 1} / {props.items.length}
          </span>
          <span class="spacer" />
          {(pos || neg) && <CopyBtn text={allPromptText(it, ex)} label="复制完整提示词" />}
          <Button
            icon
            aria-label="上一张"
            disabled={props.index <= 0}
            onClick={() => props.onIndex(props.index - 1)}
          >
            <ChevronLeft size={18} aria-hidden="true" />
          </Button>
          <Button
            icon
            aria-label="下一张"
            disabled={props.index >= props.items.length - 1}
            onClick={() => props.onIndex(props.index + 1)}
          >
            <ChevronRight size={18} aria-hidden="true" />
          </Button>
        </>
      }
    >
      <div class="lg-lb">
        <div class="lg-lb__pic">
          {hasPicture(it) ? (
            <AuditImage
              key={it.id}
              src={`/admin/api/audit/${it.id}/${it.has_image ? "image" : "thumb"}`}
              alt={`${it.key_name} 生成的图${it.has_image ? "" : "（原图已过期，这是缩略图）"}`}
            />
          ) : (
            <div class="lg-lb__none">
              <ImageOff size={28} aria-hidden="true" />
              <span>{it.status !== "ok" ? "生成未成功（无图）" : "未记录原图"}</span>
            </div>
          )}
        </div>
        <div class="lg-lb__cap">
          {it.status !== "ok" && <Badge tone={statusTone(it.status)}>{statusText(it.status)}</Badge>}
          <span class="small mono lg-lb__info">{infoLine(it, ex)}</span>
          <span class="spacer" />
        </div>
      </div>
    </Dialog>
  );
}

/* ---------------- 页面 ---------------- */
export function Audit() {
  const { query, set } = useQueryState();
  const page = intParam(query.page, 1);
  const per = intParam(query.per, 24, SIZES);
  const view: View = query.view === "list" ? "list" : "grid";
  // key 存的是「分号分隔的多个成员」，每个成员本身可能是「逗号分隔的多把 Key」：如 "1,2;5"
  const sel = /^\d+(,\d+)*(;\d+(,\d+)*)*$/.test(query.key ?? "") ? (query.key as string) : "";
  const selValues = sel ? sel.split(";") : []; // 每项是一个成员的 key_ids 串
  const flatIds = selValues.flatMap((v) => v.split(",")); // 展平成所有 key_id，喂给画廊和导出
  const keyParam = flatIds.join(",");
  const members = useAdmin<{ members: Member[] }>("/admin/api/members");
  const status = useAdmin<{ audit: { prompts: boolean; thumbs: boolean; retention_days: number } }>(
    "/admin/api/status",
  );

  const memberOptions = memberKeyOptions(members.data?.members ?? []);
  const selMembers = selValues.map((v) => ({
    value: v,
    label: memberOptions.find((o) => o.value === v)?.label ?? v,
  }));
  // 手动标签（如「疑似小号」）：点一下就选中带这个标签的全部成员。身份组标签（站长 / 邀请）不算
  const tagGroups = new Map<string, string[]>();
  for (const m of members.data?.members ?? []) {
    for (const t of m.tags ?? []) {
      if (t.tag === "站长" || t.tag === "邀请") continue;
      const list = tagGroups.get(t.tag) ?? [];
      list.push(memberKeyValue(m));
      tagGroups.set(t.tag, list);
    }
  }
  const sameSet = (a: string[], b: string[]) => a.length === b.length && a.every((x) => b.includes(x));

  const qs = new URLSearchParams({ page: String(page), per_page: String(per) });
  if (keyParam) qs.set("key_id", keyParam);
  const { data, error, pending } = usePageData<AuditResp>(`/admin/api/audit?${qs}`);
  // 大图跟着记录 id 走：列表更新时不会跳到别的图
  const [zoomId, setZoomId] = useState<number | null>(null);
  // null = 没在导出；-1 = 服务端还在打包（没开始回数据）；0–100 = 下载进度
  const [exporting, setExporting] = useState<number | null>(null);

  // 开着大图、往下看了、或不在第 1 页：冻住，新生成的记录攒着
  const top = useRef<HTMLDivElement>(null);
  const scrolled = useScrolledPast(top);
  const live = useFrozenList(data?.items, (it) => it.id, {
    frozen: scrolled || zoomId !== null || page > 1,
    key: qs.toString(),
    pending,
  });
  const items = live.rows ?? [];
  const zoom = zoomId === null ? -1 : items.findIndex((it) => it.id === zoomId);
  const setZoom = (i: number | null) => setZoomId(i === null ? null : (items[i]?.id ?? null));
  const au = status.data?.audit;
  const recording = !!(au && (au.prompts || au.thumbs));

  // 后端现在吃逗号多 Key，合并成一个 zip（多个成员时按账号分文件夹）——一键一个文件
  const exportAll = async () => {
    if (!keyParam) return;
    setExporting(-1);
    const name = selValues.length > 1 ? `owl-选中${selValues.length}人.zip` : `owl-audit-${keyParam}.zip`;
    await downloadFile(`/admin/api/audit/export?key_id=${keyParam}`, name, setExporting);
    setExporting(null);
  };

  return (
    <div class="stack">
      {tagGroups.size > 0 && (
        <div class="xa-chips">
          <span class="xa-chips__lead">标签</span>
          {[...tagGroups].map(([tag, vals]) => {
            const on = sameSet(vals, selValues);
            return (
              <Button
                key={tag}
                size="sm"
                variant={on ? "primary" : "default"}
                aria-pressed={on}
                onClick={() => set({ key: on ? null : vals.join(";"), page: null })}
              >
                {tag} {vals.length}
              </Button>
            );
          })}
        </div>
      )}
      <div class="xa-toolbar">
        <div class="field-inline">
          <span class="field-inline__k">成员</span>
          <SearchMultiSelect
            label="成员（可多选）"
            values={selValues}
            allLabel="全部成员"
            options={memberOptions}
            onChange={(vs) => set({ key: vs.join(";") || null, page: null })}
          />
        </div>
        <label class="field-inline">
          <span class="field-inline__k">视图</span>
          <select
            class="select"
            value={view}
            onChange={(e) => set({ view: e.currentTarget.value === "grid" ? null : e.currentTarget.value })}
          >
            {VIEWS.map((v) => (
              <option key={v.id} value={v.id}>
                {v.label}
              </option>
            ))}
          </select>
        </label>
        <Button
          disabled={!keyParam || exporting !== null}
          title={keyParam ? "把选中成员的全部原图打包下载（多个成员按账号分文件夹）" : "先选择成员"}
          onClick={exportAll}
        >
          <Download size={15} aria-hidden="true" />
          {exporting === null
            ? selValues.length > 1
              ? `导出 ${selValues.length} 人`
              : "导出全部"
            : exporting < 0
              ? "打包中…"
              : `导出中 ${exporting}%`}
        </Button>
      </div>

      {selMembers.length > 0 && (
        <div class="xa-chips">
          {selMembers.map((m) => (
            <span key={m.value} class="xa-chip">
              {m.label}
              <button
                type="button"
                aria-label={`取消选择 ${m.label}`}
                onClick={() =>
                  set({ key: selValues.filter((v) => v !== m.value).join(";") || null, page: null })
                }
              >
                <X size={12} aria-hidden="true" />
              </button>
            </span>
          ))}
          {selMembers.length > 1 && (
            <button type="button" class="xa-chips__clear" onClick={() => set({ key: null, page: null })}>
              清空
            </button>
          )}
        </div>
      )}

      {error && !data ? (
        <LoadError error={error} />
      ) : !data ? (
        <Skeleton h="320px" />
      ) : items.length === 0 ? (
        <div class="card">
          <Empty title={keyParam ? "该成员暂无生成记录" : "暂无生成记录"}>
            {au
              ? recording
                ? `生成记录功能已开启（提示词${au.prompts ? "开" : "关"}、原图${au.thumbs ? "开（按容量清理）" : "关"}），新的生成请求会出现在这里。${au.retention_days > 0 ? `记录会在 ${au.retention_days} 天后自动清除。` : "记录长期保存。"}`
                : "生成记录功能当前已关闭，不会保存提示词和原图。开启后，记录会在保留期满后自动清除。"
              : ""}
          </Empty>
        </div>
      ) : (
        <div ref={top} class={cx("lg-auditwrap", pending && "xa-busy")} aria-busy={pending}>
          <FreshBar fresh={live.fresh} changed={live.changed} onApply={live.apply} anchor={top} />
          {view === "list" ? (
            <ul class="lg-alist" aria-label="生成记录">
              {items.map((it, i) => {
                const ex = parseExtra(it.extra);
                const pic = hasPicture(it);
                const params = paramsLine(ex);
                return (
                  <li key={it.id} class="card lg-aitem">
                    <button
                      type="button"
                      class="lg-thumb"
                      style={{ aspectRatio: String(aspectOf(ex)) }}
                      title={pic ? undefined : "未记录原图"}
                      aria-label={`放大查看 ${it.key_name} 的记录${pic ? "" : "（未记录原图）"}`}
                      onClick={() => setZoom(i)}
                    >
                      {pic ? (
                        <AuditImage key={it.id} src={thumbUrl(it.id)} alt="" lazy />
                      ) : (
                        <ImageOff size={22} aria-hidden="true" />
                      )}
                    </button>
                    <div class="lg-aitem__b">
                      <div class="row">
                        <b>{it.key_name}</b>
                        <Badge tone={statusTone(it.status)}>{statusText(it.status)}</Badge>
                        <span class="small muted" title={fullTime(it.ts)}>
                          {shortModel(it.model) || "—"} · {kindLabel(it.kind)} · {rel(it.ts)}
                        </span>
                      </div>
                      {it.prompt ? (
                        <details class="lg-prompt">
                          <summary>{it.prompt}</summary>
                          <div class="lg-prompt__full">
                            <PromptBlock
                              title="完整提示词（含角色正面与负面提示词）"
                              text={allPromptText(it, ex)}
                            />
                          </div>
                        </details>
                      ) : (
                        <p class="small muted">未记录提示词</p>
                      )}
                      {params && <p class="small mono muted">{params}</p>}
                    </div>
                  </li>
                );
              })}
            </ul>
          ) : (
            <ul class="lg-gallery" aria-label="生成记录">
              {items.map((it, i) => {
                const pic = hasPicture(it);
                return (
                  <li key={it.id}>
                    <button
                      type="button"
                      class={cx("lg-tile", !pic && "is-none")}
                      title={pic ? undefined : "未记录原图"}
                      aria-label={`查看 ${it.key_name} 的${statusText(it.status)}记录${pic ? "" : "（未记录原图）"}`}
                      onClick={() => setZoom(i)}
                    >
                      {pic ? (
                        <AuditImage key={it.id} src={thumbUrl(it.id)} alt="" lazy />
                      ) : (
                        <ImageOff size={26} aria-hidden="true" />
                      )}
                      {it.status !== "ok" && (
                        <span class="lg-tile__st">
                          <Badge tone={statusTone(it.status)}>{statusText(it.status)}</Badge>
                        </span>
                      )}
                      <span class="lg-tile__who">{it.key_name}</span>
                    </button>
                  </li>
                );
              })}
            </ul>
          )}
          <Pager
            page={data.page}
            pages={data.pages}
            total={data.total}
            perPage={data.per_page}
            sizes={SIZES}
            onPage={(p) => set({ page: p === 1 ? null : String(p) })}
            onSize={(n) => set({ per: n === 24 ? null : String(n), page: null })}
          />
        </div>
      )}

      {zoom >= 0 && <Lightbox items={items} index={zoom} onIndex={setZoom} onClose={() => setZoom(null)} />}
    </div>
  );
}
