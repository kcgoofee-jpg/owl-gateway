import { describe, expect, it } from "vitest";
import {
  ACK_TAG,
  buildFilters,
  countFilter,
  DAY,
  findByKeyId,
  hasReason,
  matchesQuery,
  memberIssues,
  memberKeyOptions,
  memberKeyValue,
  memberLabel,
  memberStatus,
  memberVerdict,
  needsAttention,
  normalizeFilter,
  rejectReasons,
  sortMembers,
} from "./logic";
import type { Member } from "./types";

const NOW = 1_800_000_000;
let seq = 0;
function mk(p: Partial<Member> = {}): Member {
  const id = ++seq;
  return {
    id,
    name: `m${id}`,
    enabled: true,
    invited: false,
    discord_id: `9000000000000000${id}`,
    discord: null,
    created_at: NOW - 10 * DAY,
    last_used_at: NOW - 3600,
    expires_at: NOW + 20 * DAY,
    policy: {
      defaults: {
        daily_images: 100,
        daily_v5: 5,
        daily_anlas: 15,
        monthly_anlas: 99999,
        rpm: 10,
        image: true,
        v5: true,
        vibe: true,
        tags: true,
        img2img: false,
        upscale: false,
        augment: false,
        paid_parameters: false,
      },
      overrides: {},
      effective: {
        daily_images: 100,
        daily_v5: 5,
        daily_anlas: 15,
        monthly_anlas: 99999,
        rpm: 10,
        image: true,
        v5: true,
        vibe: true,
        tags: true,
        img2img: false,
        upscale: false,
        augment: false,
        paid_parameters: false,
      },
    },
    today: { images: 0, legacy_free_images: 0, v5: 0, anlas: 0, requests: 0 },
    week: { images: 0, v5: 0, anlas: 0, requests: 0 },
    total_images: 10,
    registered_at: NOW - 10 * DAY,
    first_image_at: NOW - 9 * DAY,
    last_image_at: NOW - 3600,
    rejected_24h: 0,
    rejected_why: [],
    share: null,
    reset_today: null,
    tags: [],
    auto_tags: [],
    key_count: 1,
    all_key_ids: [id],
    ...p,
  };
}

describe("memberIssues / needsAttention", () => {
  it("健康成员没有待处理事项", () => {
    const m = mk();
    expect(memberIssues(m, NOW)).toEqual([]);
    expect(needsAttention(m, NOW)).toBe(false);
  });

  it("Key 已过期是 bad，必须处理", () => {
    const m = mk({ expires_at: NOW - 10 });
    expect(memberIssues(m, NOW)[0]?.level).toBe("bad");
    expect(needsAttention(m, NOW)).toBe(true);
    expect(memberStatus(m, NOW)?.text).toBe("已过期");
  });

  it("3 天内到期要看一眼；点「没问题」后 24 小时内不再提醒，但 bad 不受影响", () => {
    const soon = mk({ expires_at: NOW + 2 * DAY });
    expect(needsAttention(soon, NOW)).toBe(true);
    const ack = mk({
      expires_at: NOW + 2 * DAY,
      tags: [{ tag: ACK_TAG, note: "", by: "站长", ts: NOW - 3600 }],
    });
    expect(needsAttention(ack, NOW)).toBe(false);
    const ackExpired = mk({
      expires_at: NOW - 5,
      tags: [{ tag: ACK_TAG, note: "", by: "站长", ts: NOW - 3600 }],
    });
    expect(needsAttention(ackExpired, NOW)).toBe(true);
    // 24 小时后「没问题」失效
    const stale = mk({
      expires_at: NOW + 2 * DAY,
      tags: [{ tag: ACK_TAG, note: "", by: "站长", ts: NOW - 2 * DAY }],
    });
    expect(needsAttention(stale, NOW)).toBe(true);
  });

  it("单纯「请求过快」只是提示，不进需要处理；次数很多的其他自动标签才算", () => {
    const rate = mk({ auto_tags: [{ tag: "请求过快", count: 99 }] });
    expect(memberIssues(rate, NOW)[0]?.level).toBe("info");
    expect(needsAttention(rate, NOW)).toBe(false);
    const loud = mk({ auto_tags: [{ tag: "并发超限", count: 60 }] });
    expect(needsAttention(loud, NOW)).toBe(true);
  });

  it("被限流暂停给出「误判，解除」动作", () => {
    const m = mk({ share: { paused_until: NOW + 600 } });
    const i = memberIssues(m, NOW).find((x) => x.id === "share-paused");
    expect(i?.actions).toEqual([{ kind: "share-clear" }]);
    expect(memberStatus(m, NOW)?.text).toBe("已限流");
  });

  it("站长标记「疑似」类标签要处理，并带移除动作；标签名没有特殊含义", () => {
    for (const tag of ["疑似小号", "疑似同一人", "疑似转卖"]) {
      const m = mk({ tags: [{ tag, note: "同网段", by: "站长", ts: NOW - 60 }] });
      const i = memberIssues(m, NOW).find((x) => x.id === `tag:${tag}`);
      expect(i?.need).toBe(true);
      expect(i?.actions).toEqual([{ kind: "tag-del", tag }]);
      expect(needsAttention(m, NOW)).toBe(true);
    }
  });
});

describe("memberVerdict / memberStatus", () => {
  it("按优先级给一句话结论", () => {
    expect(memberVerdict(mk({ first_image_at: null }), NOW).title).toContain("还没出过图");
    expect(
      memberVerdict(
        mk({ today: { images: 100, legacy_free_images: 100, v5: 0, anlas: 0, requests: 3 } }),
        NOW,
      ).title,
    ).toContain("额度用完");
    expect(
      memberVerdict(mk({ today: { images: 7, v5: 0, anlas: 0, requests: 3, legacy_free_images: 7 } }), NOW)
        .title,
    ).toContain("今天出了 7 张");
  });
  it("V5 图片不消耗 V4.5 免费图额度", () => {
    const member = mk({ today: { images: 125, legacy_free_images: 25, v5: 100, anlas: 0, requests: 125 } });
    expect(memberVerdict(member, NOW).title).toContain("正常使用");
    expect(memberStatus(member, NOW)?.text).not.toBe("额度用完");
  });
  it("正常成员没有状态徽标", () => {
    expect(memberStatus(mk(), NOW)).toBeNull();
  });
  it("新加入的成员标「新加入」", () => {
    expect(memberStatus(mk({ registered_at: NOW - 3600 }), NOW)?.text).toBe("新加入");
  });
});

describe("筛选横栏", () => {
  const ids = (f: ReturnType<typeof buildFilters>) => f.chips.map((c) => c.id);
  it("一条横栏按顺序：全部 / 今日 / 没出图 / 新加入 / 被拒 / 快回收 / 邀请 / 需要处理", () => {
    expect(ids(buildFilters(NOW, 3))).toEqual([
      "all",
      "today",
      "never",
      "new",
      "rejected",
      "reclaim",
      "invite",
      "attention",
    ]);
  });
  it("已下线的筛选不在横栏里：普通身份组、后台手动建的 Key、旧的 staff 筛选、站长标签", () => {
    const all = ids(buildFilters(NOW, 3)).join(",");
    for (const gone of ["role:regular", "role:admin", "role:test", "manual", "staff", "tag:"])
      expect(all).not.toContain(gone);
  });
  it("「快被回收」所有领取方式使用相同闲置窗口，标签显示有效天数", () => {
    const idle = mk({ last_image_at: NOW - 2.5 * DAY });
    const fresh = mk({ last_image_at: NOW - 3600 });
    const invite = mk({ last_image_at: NOW - 2.5 * DAY, invited: true });
    const never = mk({ last_image_at: null, first_image_at: null, created_at: NOW - 2.5 * DAY });
    const reclaim = buildFilters(NOW, 3).chips.find((x) => x.id === "reclaim");
    expect(reclaim?.label).toBe("快被回收（满 3 天）");
    expect([idle, fresh, invite, never].filter((m) => reclaim?.test(m))).toEqual([idle, invite, never]);
    expect(buildFilters(NOW, 7).chips.find((x) => x.id === "reclaim")?.label).toBe("快被回收（满 7 天）");
  });
  it("回收天数为 0（关闭）时没有「快被回收」", () => {
    expect(ids(buildFilters(NOW, 0))).not.toContain("reclaim");
  });
  it("各标签的人数", () => {
    const rows = [
      mk({ today: { images: 1, v5: 0, anlas: 0, requests: 2, legacy_free_images: 1 } }),
      mk({ first_image_at: null, registered_at: NOW - 3600 }),
      mk({ invited: true, tags: [{ tag: "邀请", note: "", by: "", ts: NOW }], rejected_24h: 2 }),
    ];
    const f = buildFilters(NOW, 3);
    const n = (id: string) => countFilter(rows, f.chips.find((c) => c.id === id) as never);
    expect(n("all")).toBe(3);
    expect(n("today")).toBe(1);
    expect(n("never")).toBe(1);
    expect(n("new")).toBe(1);
    expect(n("rejected")).toBe(1);
    expect(n("invite")).toBe(1);
    expect(n("attention")).toBe(0);
  });
  it("被拒原因：只数近 24 小时被拒的，人数多的在前", () => {
    const tag = (t: string, count = 1) => ({ tag: t, count, note: "" });
    const rows = [
      mk({ rejected_24h: 3, auto_tags: [tag("请求过快", 3)] }),
      mk({ rejected_24h: 1, auto_tags: [tag("角色超 6 个"), tag("请求过快")] }),
      mk({ rejected_24h: 2, auto_tags: [tag("请求过快", 2)] }),
      mk({ rejected_24h: 0, auto_tags: [tag("旧标签")] }),
    ];
    expect(rejectReasons(rows)).toEqual([
      { tag: "请求过快", count: 3 },
      { tag: "角色超 6 个", count: 1 },
    ]);
    expect(hasReason(rows[1] as Member, "角色超 6 个")).toBe(true);
    expect(hasReason(rows[0] as Member, "角色超 6 个")).toBe(false);
  });
  it("老链接 ?filter=role:invite 换成「邀请 Key」", () => {
    expect(normalizeFilter("role:invite")).toBe("invite");
    expect(normalizeFilter("never")).toBe("never");
    expect(normalizeFilter(undefined)).toBeNull();
  });
  it("搜索：名称、Discord ID、用户名、标签与备注", () => {
    const m = mk({
      name: "月见草",
      discord_id: "900000000000000042",
      discord: {
        id: "900000000000000042",
        username: "tsukimi",
        display_name: "月见",
        avatar_url: "",
        profile_url: "",
      },
      tags: [{ tag: "邀请", note: "朋友介绍", by: "站长", ts: NOW }],
    });
    for (const q of ["月见草", "0042", "TSUKI", "朋友介绍"]) expect(matchesQuery(m, q)).toBe(true);
    expect(matchesQuery(m, "不存在")).toBe(false);
  });
});

describe("排序 / 身份 / 查找", () => {
  it("数字列倒序，同值按名称正序兜底", () => {
    const a = mk({ name: "b", today: { images: 5, v5: 0, anlas: 0, requests: 1, legacy_free_images: 5 } });
    const b = mk({ name: "a", today: { images: 5, v5: 0, anlas: 0, requests: 1, legacy_free_images: 5 } });
    const c = mk({ name: "c", today: { images: 9, v5: 0, anlas: 0, requests: 1, legacy_free_images: 9 } });
    expect(sortMembers([a, b, c], "today", "desc").map((m) => m.name)).toEqual(["c", "a", "b"]);
    expect(sortMembers([a, b, c], "today", "asc").map((m) => m.name)).toEqual(["a", "b", "c"]);
  });
  it("显示名优先 Discord 昵称", () => {
    expect(memberLabel(mk({ name: "后台名" }))).toBe("后台名");
    expect(
      memberLabel(
        mk({ discord: { id: "1", username: "u", display_name: "昵称", avatar_url: "", profile_url: "" } }),
      ),
    ).toBe("昵称");
  });
  it("按任一把 Key 找到合并后的成员", () => {
    const a = mk({ all_key_ids: [11, 12], id: 12 });
    expect(findByKeyId([a], 11)).toBe(a);
    expect(findByKeyId([a], 99)).toBeUndefined();
  });
});

describe("成员下拉选项", () => {
  it("值是名下所有 Key 的 id，多把 Key 时标签带数量", () => {
    const a = mk({ all_key_ids: [11, 12], id: 12, key_count: 2, name: "kes" });
    const b = mk({
      id: 5,
      all_key_ids: [5],
      name: "tan",
      discord: { display_name: "狸猫" } as Member["discord"],
    });
    expect(memberKeyValue(a)).toBe("11,12");
    expect(memberKeyOptions([a, b])).toEqual([
      { value: "11,12", label: "kes（2 把 Key）", hint: "kes" },
      { value: "5", label: "狸猫", hint: "tan" },
    ]);
  });
});

describe("头像首字", () => {
  it("跳过颜文字符号，名字里没有字母就用用户名", async () => {
    const { initialOf } = await import("../_kit");
    expect(initialOf("月见草")).toBe("月");
    expect(initialOf("kestrel")).toBe("K");
    expect(initialOf("(๑•̀ㅂ•́)", "tanuki")).toBe("T");
    expect(initialOf("✨", null)).toBe("?");
  });
});
