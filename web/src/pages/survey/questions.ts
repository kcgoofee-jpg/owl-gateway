/** 问卷题目与提交格式（与旧版 survey.html / POST /member/survey 一致：选项的 value 就是后台看到的文字）。 */

export type Choice = { value: string; label: string };

export const IMPROVE: Choice[] = [
  { value: "报错提示更清楚", label: "报错提示更清楚（一眼知道为什么被拒、该等多久）" },
  { value: "排队和等待体验", label: "排队和等待体验（能看到排第几、预计时间更准）" },
  { value: "教程和文档", label: "教程和文档（接入步骤、常见问题）" },
  { value: "稳定性", label: "稳定性（少报错、少失败）" },
  { value: "网页功能", label: "网页功能（额度查询、状态页、反馈入口）" },
  { value: "客户端接入便利性", label: "客户端接入便利性（填地址就能用，少踩坑）" },
];

export type Answers = {
  client: string;
  improve: string[];
  improveOther: string;
};

export const EMPTY: Answers = { client: "", improve: [], improveOther: "" };

export type Errors = { client?: string };

/** 必答题没填 / 没选的提示（题号与页面上一致）。 */
export function validate(a: Answers): Errors {
  const errs: Errors = {};
  if (!a.client.trim()) errs.client = "第 1 题还没填";
  return errs;
}

/** 提交给 /member/survey 的 JSON（字段名沿用旧版）。 */
export function payload(a: Answers) {
  return {
    client: a.client.trim(),
    improve: a.improve,
    improve_other: a.improveOther.trim(),
  };
}
