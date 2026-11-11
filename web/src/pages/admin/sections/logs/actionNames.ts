/**
 * 操作日志：旧名并到新名、按「谁做的 / 做什么」分组，生成筛选下拉的选项（纯函数）。
 * 只列当前还会产生的操作；已下线功能的动作名不进分组（历史记录仍按原名显示在「其他」里）。
 */

/** 老版本按接口路径或旧名字记的条目，显示时并到对应的名字里（历史记录不改，只改显示）。 */
const ALIAS: Record<string, string> = {
  "领取 Key 失败（授权回调）": "领取 Key 失败",
  "领取 Key 失败（/register）": "领取 Key 失败",
  "领取 Key（/register 直发）": "领取 Key",
  移除成员标签: "成员标签",
  取消候补: "候补",
  邀请候补: "候补",
  候补邀请过期: "候补",
  调整名额上限: "领 Key 名额",
};

/** 只含 action_log::ADMIN_ACTIONS 里现在还会写的名字。历史名走 ALIAS 和「其他」。 */
const GROUPS: Array<[string, string[]]> = [
  ["后台 · 登录", ["登录后台", "修改后台密码", "退出登录"]],
  [
    "后台 · 成员与 Key",
    [
      "新建 Key",
      "重新生成 Key",
      "修改 Key",
      "重置今日额度",
      "删除 Key",
      "成员标签",
      "发重置券",
      "解除限流暂停",
      "切换站长生图权限",
      "设置赞助席",
      "全站杀头重置",
    ],
  ],
  [
    "后台 · 上游",
    ["添加上游 Token", "替换上游 Token", "删除上游 Token", "修改上游 Token V5 日限", "启用/停用上游 Token"],
  ],
  ["后台 · 设置", ["修改预算", "手动改设置", "修改开放注册设置", "修改账号保护与排队设置", "修改排队与冷却"]],
];

export const actionLabel = (a: string): string => ALIAS[a] ?? a;

export type KindGroup = { title: string; options: Array<{ value: string; label: string }> };

const collator = new Intl.Collator(["zh-Hans-CN", "en"], { numeric: true, sensitivity: "base" });

/** kinds：接口给的全部原始操作名。同一个显示名可能对应多个旧名，筛选值用「|」连起来（后端按 | 拆开）。 */
export function actionKindGroups(kinds: string[]): KindGroup[] {
  const byLabel = new Map<string, string[]>();
  for (const k of kinds) {
    const l = actionLabel(k);
    byLabel.set(l, [...(byLabel.get(l) ?? []), k]);
  }
  const used = new Set<string>();
  const pick = (title: string, labels: string[]): KindGroup | null => {
    const options = labels
      .filter((l) => byLabel.has(l))
      .map((l) => {
        used.add(l);
        return { value: (byLabel.get(l) ?? []).join("|"), label: l };
      });
    return options.length ? { title, options } : null;
  };
  const head = GROUPS.map(([t, ls]) => pick(t, ls)).filter((g): g is KindGroup => g !== null);
  // 没归类的；裸接口路径（"POST /xxx"）不列
  const rest = [...byLabel.keys()]
    .filter((l) => !used.has(l) && !/^[A-Z]+ \//.test(l))
    .sort(collator.compare);
  const other = pick("其他", rest);
  return other ? [...head, other] : head;
}

/** 「操作者」筛选：接口的 actor 参数。旧链接里的 admin / system 并到手动 / 自动。 */
export const ACTOR_FILTERS = [
  { id: "", label: "全部" },
  { id: "manual", label: "手动（站长 · Claude）" },
  { id: "auto", label: "自动（定时任务）" },
  { id: "member", label: "成员自己" },
] as const;

export function actorFilter(q: string | undefined): string {
  const v = q === "admin" ? "manual" : q === "system" ? "auto" : (q ?? "");
  return ACTOR_FILTERS.some((a) => a.id === v) ? v : "";
}
