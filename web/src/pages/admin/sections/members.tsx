/**
 * 成员：列表（今日 / 近 7 天 / 累计用量）、筛选搜索排序、展开详情与全部 Key 操作。
 * 支持 ?focus=<Key 编号>（⌘K 面板跳转到这里，滚动并高亮该成员）和 ?filter=<筛选 id>（总览的「成员要处理」）。
 */
import { ChevronDown, Plus, Search } from "lucide-preact";
import { useEffect, useMemo, useRef, useState } from "preact/hooks";
import { useLocation } from "preact-iso";
import { Button, Menu, toast } from "../../../ui";
import { useAdmin } from "../Admin";
import { ConfirmDialog, LoadError, Loading, SectionHead } from "./_kit";
import { FreshBar, useFrozenList, useScrolledPast } from "./live";
import {
  addTag,
  clearShare,
  createInvite,
  disableKey,
  type Res,
  regenerateKey,
  removeMember,
  removeTag,
  resetAllKeys,
  setQuotaAuto,
  switchRole,
} from "./members/actions";
import {
  BansDialog,
  BindDiscordDialog,
  CouponDialog,
  KeyRevealDialog,
  ResetQuotaDialog,
  TagDialog,
} from "./members/Dialogs";
import { FilterBar } from "./members/FilterBar";
import { inviteMessage } from "./members/invite";
import { KeyDialog } from "./members/KeyDialog";
import {
  ACK_TAG,
  ALL_FILTER,
  buildFilters,
  countFilter,
  defaultFilter,
  findByKeyId,
  hasReason,
  matchesQuery,
  memberIssues,
  memberLabel,
  normalizeFilter,
  REJECTED_FILTER,
  rejectReasons,
  type SortKey,
  sortMembers,
} from "./members/logic";
import type { DetailActions } from "./members/MemberDetail";
import { MemberList } from "./members/MemberList";
import { Empty, NARROW, useMedia } from "./members/shared";
import type { KeysResp, MemberRow, MembersResp } from "./members/types";
import "./members.css";

type Dlg =
  | { t: "edit"; id: number | null; manual?: boolean }
  | { t: "reveal"; title: string; intro: string; token: string; message?: string }
  | {
      t: "bind" | "coupon" | "tag" | "disable" | "regen" | "reset" | "delete" | "ban";
      id: number;
    }
  | { t: "bans" | "resetAll" };

const PAGE = 60;

export default function Members() {
  const loc = useLocation();
  const { query } = loc;
  const mem = useAdmin<MembersResp>("/admin/api/members");
  const keys = useAdmin<KeysResp>("/admin/api/keys");
  const bans = useAdmin<{ total: number }>("/admin/api/bans");
  const narrow = useMedia(NARROW);

  const [filter, setFilter] = useState<string | null>(normalizeFilter(query.filter));
  const [reason, setReason] = useState<string | null>(null);
  const [search, setSearch] = useState("");
  const [sort, setSort] = useState<SortKey>("today");
  const [dir, setDir] = useState<"asc" | "desc">("desc");
  const [open, setOpen] = useState<Set<number>>(new Set());
  const [limit, setLimit] = useState(PAGE);
  const [dlg, setDlg] = useState<Dlg | null>(null);
  const [focusId, setFocusId] = useState<number | null>(null);
  const [inviting, setInviting] = useState(false);

  const now = Date.now() / 1000;
  const idleDays = mem.data?.inactivity_days ?? 3;
  const rows: MemberRow[] = useMemo(() => {
    const km = new Map((keys.data?.keys ?? []).map((k) => [k.id, k]));
    // 以成员为主：停用的 Key 不再列出（谁用过、什么时候停用的，日志里都有）
    return (mem.data?.members ?? []).filter((m) => m.enabled).map((m) => ({ ...m, k: km.get(m.id) }));
  }, [mem.data, keys.data]);
  const byId = (id: number) => rows.find((r) => r.id === id);

  const filters = useMemo(() => buildFilters(now, idleDays), [now, idleDays]);
  const all = filters.chips;

  // 第一次有数据：有要处理的人就直接看「需要处理」
  useEffect(() => {
    if (filter === null && rows.length) setFilter(defaultFilter(rows, now));
  }, [filter, rows, now]);
  const activeId = all.some((f) => f.id === filter) ? (filter as string) : ALL_FILTER;
  const active = all.find((f) => f.id === activeId) ?? filters.chips[0];
  // 被拒原因是「24 小时被拒过」之内的第二层；原因随刷新消失（那 24 小时已过）时自动退回
  const reasons = useMemo(() => rejectReasons(rows), [rows]);
  const reasonOn = activeId === REJECTED_FILTER && reason !== null && reasons.some((r) => r.tag === reason);
  const chips = filters.chips.map((f) => ({
    id: f.id,
    label: f.label,
    count: countFilter(rows, f),
    ...(f.hint ? { hint: f.hint } : {}),
    ...(f.id === "attention" ? { hot: true } : {}),
  }));

  const list = useMemo(() => {
    if (!active) return [];
    return sortMembers(
      rows
        .filter(active.test)
        .filter((m) => !reasonOn || hasReason(m, reason as string))
        .filter((m) => matchesQuery(m, search)),
      sort,
      dir,
    );
  }, [rows, active, reasonOn, reason, search, sort, dir]);
  // 展开了某位、开着对话框、或往下看了：顺序和名单冻住（默认按今日用量排，会在眼前跳位置），数值照常更新
  const top = useRef<HTMLDivElement>(null);
  const scrolled = useScrolledPast(top);
  const live = useFrozenList(list, (m) => m.id, {
    frozen: scrolled || open.size > 0 || dlg !== null,
    key: `${activeId}|${reasonOn ? reason : ""}|${search}|${sort}|${dir}`,
  });
  const frozenList = live.rows ?? list;
  const shown = frozenList.slice(0, limit);

  /* ?focus=<Key 编号>：展开、滚到视野中央并高亮 */
  const focusParam = Number(query.focus) || 0;
  const handled = useRef(0);
  useEffect(() => {
    if (!focusParam || !rows.length || handled.current === focusParam) return;
    const m = findByKeyId(rows, focusParam);
    if (!m) return;
    handled.current = focusParam;
    setFilter(ALL_FILTER);
    setReason(null);
    setSearch("");
    setOpen((s) => new Set(s).add(m.id));
    setFocusId(m.id);
    const idx = sortMembers(rows, sort, dir).findIndex((x) => x.id === m.id);
    if (idx >= limit) setLimit(idx + 10);
  }, [focusParam, rows, sort, dir, limit]);
  useEffect(() => {
    if (focusId === null) return;
    const t = setTimeout(() => {
      document.getElementById(`mb-m-${focusId}`)?.scrollIntoView({
        block: "center",
        behavior: matchMedia("(prefers-reduced-motion: reduce)").matches ? "auto" : "smooth",
      });
    }, 60);
    const clear = setTimeout(() => setFocusId(null), 6000);
    return () => {
      clearTimeout(t);
      clearTimeout(clear);
    };
  }, [focusId]);

  const toggleOpen = (id: number) =>
    setOpen((s) => {
      const n = new Set(s);
      if (n.has(id)) n.delete(id);
      else n.add(id);
      return n;
    });
  const onSort = (k: SortKey) => {
    if (k === sort) setDir(dir === "asc" ? "desc" : "asc");
    else {
      setSort(k);
      setDir("desc");
    }
  };

  /* ---------- 操作（对话框之外的，直接出 toast） ---------- */
  const quick = async (p: Promise<Res>, okText: string) => {
    const r = await p;
    if (r.ok) toast(okText);
    else toast(r.error, { error: true });
  };
  const act: DetailActions = {
    edit: (m, manual) => setDlg({ t: "edit", id: m.id, ...(manual ? { manual } : {}) }),
    quotaAuto: (m) => quick(setQuotaAuto(m.id), `「${memberLabel(m)}」已改为统一额度`),
    role: (m, r) =>
      quick(
        switchRole(m.id, r),
        `已将「${memberLabel(m)}」的身份组切换为「${{ regular: "普通", invite: "邀请", test: "测试", admin: "站长" }[r]}」`,
      ),
    issue: (m, a) => {
      if (a.kind === "share-clear") void quick(clearShare(m.id), "已解除限流暂停");
      else if (a.kind === "edit") setDlg({ t: "edit", id: m.id });
      else if (a.kind === "tag-del") void quick(removeTag(m.id, a.tag), `已移除：${a.tag}`);
      else {
        const need = memberIssues(m, now)
          .filter((x) => x.need)
          .map((x) => x.title)
          .join("；");
        void quick(addTag(m.id, ACK_TAG, `看过：${need}`), "好，24 小时内不再提醒");
      }
    },
    bind: (m) => setDlg({ t: "bind", id: m.id }),
    addTag: (m) => setDlg({ t: "tag", id: m.id }),
    removeTag: (m, tag) => quick(removeTag(m.id, tag), `已移除：${tag}`),
    coupon: (m) => setDlg({ t: "coupon", id: m.id }),
    resetQuota: (m) => setDlg({ t: "reset", id: m.id }),
    disable: (m) => setDlg({ t: "disable", id: m.id }),
    regenerate: (m) => setDlg({ t: "regen", id: m.id }),
    remove: (m, ban) => setDlg({ t: ban ? "ban" : "delete", id: m.id }),
  };

  const newInvite = async () => {
    if (inviting) return;
    setInviting(true);
    const r = await createInvite();
    setInviting(false);
    if (!r.ok) return toast(r.error, { error: true });
    const t = r.data.key.token;
    setDlg({
      t: "reveal",
      title: `${r.data.key.name} 已创建`,
      intro: "下面是发给对方的完整消息，复制后直接私信对方。",
      token: t,
      message: inviteMessage(`${location.origin}/`, t),
    });
  };

  const close = () => setDlg(null);
  const target = dlg && "id" in dlg && dlg.id !== null ? byId(dlg.id) : undefined;

  /* ---------- 对话框 ---------- */
  const renderDialog = () => {
    if (!dlg) return null;
    switch (dlg.t) {
      case "edit": {
        if (dlg.id !== null && !target) return null;
        return (
          <KeyDialog
            target={dlg.id === null ? null : (target ?? null)}
            {...(dlg.manual ? { startManual: true } : {})}
            idleDays={idleDays}
            onClose={close}
            onSaved={(text) => {
              close();
              toast(text);
            }}
            onCreated={(key) => {
              setDlg({
                t: "reveal",
                title: "新 Key 已创建",
                intro: "请立即复制新 Key，发给对方：",
                token: key.token,
              });
            }}
          />
        );
      }
      case "reveal":
        return (
          <KeyRevealDialog
            title={dlg.title}
            intro={dlg.intro}
            token={dlg.token}
            {...(dlg.message ? { message: dlg.message } : {})}
            onClose={close}
          />
        );
      case "bind":
        if (!target) return null;
        return (
          <BindDiscordDialog
            keyId={target.id}
            name={memberLabel(target)}
            currentId={target.discord ? String(target.discord.id) : (target.discord_id ?? "")}
            currentUser={target.discord?.username ?? ""}
            onClose={close}
            onDone={(t) => toast(t)}
          />
        );
      case "coupon":
        if (!target) return null;
        return (
          <CouponDialog
            keyId={target.id}
            name={memberLabel(target)}
            onClose={close}
            onDone={(t) => toast(t)}
          />
        );
      case "tag":
        if (!target) return null;
        return (
          <TagDialog keyId={target.id} name={memberLabel(target)} onClose={close} onDone={(t) => toast(t)} />
        );
      case "bans":
        return <BansDialog onClose={close} onChanged={(t) => toast(t)} />;
      case "resetAll":
        return (
          <ConfirmDialog
            title="全站重置 Key"
            confirmText="确认重置"
            danger
            phrase="确认重置"
            onClose={close}
            onConfirm={async () => {
              const r = await resetAllKeys();
              if (!r.ok) return r.error;
              toast(r.data.message || "全站已重置", { ms: 6000 });
              return undefined;
            }}
          >
            <p>
              <b>高危操作：</b>将失效全站所有成员的 Key，并清空 Discord 绑定。带「邀请」标签的 Key
              不受影响（保留启用、绑定与标签）。
            </p>
            <p>执行后，成员需要在 Discord 输入 /register 重新领取 Key。</p>
          </ConfirmDialog>
        );
      case "disable": {
        if (!target) return null;
        const name = memberLabel(target);
        return (
          <ConfirmDialog
            title={`停用「${name}」`}
            confirmText="停用"
            danger
            onClose={close}
            onConfirm={async () => {
              const r = await disableKey(target.id);
              if (!r.ok) return r.error;
              toast(`已停用「${name}」`);
              return undefined;
            }}
          >
            <p>停用后其 Key 立即不能使用，成员列表里不再显示；记录和用量保留，记录在操作日志里。</p>
          </ConfirmDialog>
        );
      }
      case "regen": {
        if (!target) return null;
        const name = memberLabel(target);
        return (
          <ConfirmDialog
            title={`重新生成「${name}」的 Key`}
            confirmText="重新生成"
            danger
            // 成功后确认框会调 onClose；此时已切到「展示新 Key」，不能被它关掉
            onClose={() => setDlg((d) => (d?.t === "reveal" ? d : null))}
            onConfirm={async () => {
              const r = await regenerateKey(target.id);
              if (!r.ok) return r.error;
              setDlg({
                t: "reveal",
                title: "Key 已重新生成",
                intro: "旧 Key 已失效，请复制新 Key 并转交给成员更新客户端：",
                token: r.data.token,
              });
              return undefined;
            }}
          >
            <p>
              旧 Key 将立即失效，需在客户端替换。配额、权限、有效期和历史用量保留，已受理任务继续完成。新 Key
              会显示给你，请转交给成员。
            </p>
          </ConfirmDialog>
        );
      }
      case "reset": {
        if (!target) return null;
        return (
          <ResetQuotaDialog
            keyId={target.id}
            name={memberLabel(target)}
            onClose={close}
            onDone={(text) => toast(text)}
          />
        );
      }
      case "delete":
      case "ban": {
        if (!target) return null;
        const name = memberLabel(target);
        const ban = dlg.t === "ban";
        const dc = !!target.discord_id;
        return (
          <ConfirmDialog
            title={ban ? `删除「${name}」并永久禁止领取` : `删除「${name}」`}
            confirmText={ban ? "删除并永久禁止" : "删除"}
            danger
            onClose={close}
            onConfirm={async () => {
              const r = await removeMember(target.id, ban);
              if (!r.ok) return r.error;
              setOpen((s) => {
                const n = new Set(s);
                n.delete(target.id);
                return n;
              });
              toast(ban ? `已删除「${name}」并永久禁止领取` : `已删除「${name}」`);
              return undefined;
            }}
          >
            {ban ? (
              <p>
                其 Key 将被删除，Discord 领取记录会被清除并摘除身份组。该 Discord
                账号以后无法再次自助领取，此操作不可恢复。
              </p>
            ) : dc ? (
              <p>
                其 Key 将被删除，Discord 领取记录会被清除并摘除身份组。删除后该成员可以重新自助领取新的
                Key，历史用量不再保留。
              </p>
            ) : (
              <p>其 Key 将被删除且不可恢复。</p>
            )}
          </ConfirmDialog>
        );
      }
    }
  };

  /* ---------- 页面 ---------- */
  return (
    <div class="xa-page">
      <SectionHead
        title="成员"
        actions={
          <div class="xa-toolbar">
            <Button variant="primary" onClick={() => setDlg({ t: "edit", id: null })}>
              <Plus size={15} aria-hidden="true" />
              新建 Key
            </Button>
            <Menu
              button={(p) => (
                <Button {...p}>
                  更多
                  <ChevronDown size={15} aria-hidden="true" />
                </Button>
              )}
              items={[
                { label: "新建邀请 Key", onSelect: newInvite },
                { label: `封禁黑名单（${bans.data?.total ?? 0}）`, onSelect: () => setDlg({ t: "bans" }) },
                { label: "全站重置 Key…", onSelect: () => setDlg({ t: "resetAll" }), danger: true },
              ]}
            />
          </div>
        }
      />

      {mem.loading && !mem.data ? (
        <Loading rows={6} />
      ) : mem.error && !mem.data ? (
        <LoadError error={mem.error} />
      ) : (
        <>
          <div class="mb-tools">
            <div class="xa-toolbar">
              <label class="mb-search">
                <Search size={15} aria-hidden="true" />
                <input
                  class="input"
                  type="search"
                  placeholder="搜索名称、Discord ID、标签或备注"
                  aria-label="搜索成员"
                  value={search}
                  onInput={(e) => setSearch(e.currentTarget.value)}
                />
              </label>
            </div>
            <FilterBar
              chips={chips}
              active={activeId}
              onToggle={(id) => {
                // 再点一次选中的标签 = 取消，回到全部成员
                setFilter(id === activeId ? ALL_FILTER : id);
                setReason(null);
                setLimit(PAGE);
              }}
              reasonsOf={REJECTED_FILTER}
              reasons={reasons}
              reason={reasonOn ? reason : null}
              onReason={(tag) => {
                setReason(tag);
                setLimit(PAGE);
              }}
            />
          </div>

          {frozenList.length === 0 ? (
            <div class="card">
              <Empty
                title={
                  activeId === "attention"
                    ? "没有要处理的成员"
                    : rows.length
                      ? "没有匹配的成员"
                      : "还没有成员"
                }
              >
                {activeId === "attention"
                  ? "风险信号、到期、被拒过多的成员会出现在这里。"
                  : rows.length
                    ? "换个筛选或关键词试试。"
                    : "成员在 Discord 领取 Key 后会出现在这里。"}
              </Empty>
            </div>
          ) : (
            <div ref={top} class="stack">
              <FreshBar
                fresh={live.fresh}
                changed={live.changed}
                onApply={live.apply}
                anchor={top}
                unit="位"
                changedText="排序或名单有变化"
              />
              <MemberList
                rows={shown}
                now={now}
                narrow={narrow}
                sort={sort}
                dir={dir}
                onSort={onSort}
                open={open}
                onToggle={toggleOpen}
                focusId={focusId}
                act={act}
              />
              {frozenList.length > shown.length && (
                <div class="mb-more-row">
                  <Button onClick={() => setLimit(limit + PAGE)}>
                    显示更多（还有 {frozenList.length - shown.length} 位）
                  </Button>
                </div>
              )}
            </div>
          )}
        </>
      )}
      {renderDialog()}
    </div>
  );
}
