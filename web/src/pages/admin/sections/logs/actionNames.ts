/** 操作日志：旧名并到新名、按「谁做的 / 做什么」分组，生成筛选下拉的选项（纯函数）。 */

/** 老版本按接口路径或旧名字记的条目，显示时并到对应的名字里（历史记录不改，只改显示）。现在已经不存在的操作名只留在这里做历史归并，不再列进下面的分组 */
const ALIAS: Record<string, string> = {
  "PUT /ops/economy": "节约模式（已下线）",
  节约模式: "节约模式（已下线）",
  "POST /keys/invite": "新建邀请 Key",
  创建邀请: "新建邀请 Key",
  创建邀请Key: "新建邀请 Key",
  "创建邀请 Key": "新建邀请 Key",
  "领取 Key 失败（授权回调）": "领取 Key 失败",
  "领取 Key 失败（/register）": "领取 Key 失败",
  "领取 Key（/register 直发）": "领取 Key",
  移除成员标签: "成员标签",
  取消候补: "候补",
  邀请候补: "候补",
  候补邀请过期: "候补",
  关闭Bug: "处理 Bug",
  "关闭 Bug": "处理 Bug",
  "标记 Bug 已处理": "处理 Bug",
  "全部标记 Bug 已处理": "处理 Bug",
  调整名额上限: "领 Key 名额",
  动态额度调整: "动态额度",
  动态额度应用: "动态额度",
  全站重置Key: "全站杀头重置",
  "全站重置 Key": "全站杀头重置",
};

const GROUPS: Array<[string, string[]]> = [
  [
    "成员",
    [
      "领取 Key",
      "领取 Key 失败",
      "Key 登录网页",
      "成员重置 Key（/resetkey）",
      "提交反馈（/反馈）",
      "成员使用重置券",
    ],
  ],
  [
    "后台 · 成员与 Key",
    [
      "新建 Key",
      "新建邀请 Key",
      "修改 Key",
      "删除 Key",
      "重置今日额度",
      "成员标签",
      "手动绑定 Discord",
      "修改账号身份组",
      "添加黑名单",
      "移出黑名单",
      "全站杀头重置",
    ],
  ],
  [
    "后台 · 设置",
    [
      "修改开放注册设置",
      "领 Key 名额",
      "修改账号保护与排队设置",
      "修改请求来源保留设置",
      "修改生成记录设置",
      "处理 Bug",
      "清理日志",
      "导出成员反馈",
      "清空原图",
      "登录后台",
    ],
  ],
  [
    "手动 · 临时设置",
    [
      "手动：V5 临时加量",
      "手动：账号日上限临时调整",
      "手动：站付 V5",
      "手动：改当天 V5 方案",
      "V5 不设限试验",
      "补记：手动改设置",
    ],
  ],
  [
    "系统自动",
    [
      "动态额度",
      "Anlas 自动分配",
      "Anlas 核对",
      "自动驾驶（观察）",
      "自动调整：每小时上限",
      "退群回收 Key",
      "Key 到期释放名额",
      "闲置回收 Key",
      "闲置回收前提醒",
      "每日清空自助注册",
      "闲置计时重置",
    ],
  ],
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
