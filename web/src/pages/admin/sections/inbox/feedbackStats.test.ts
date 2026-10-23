import { describe, expect, it } from "vitest";
import type { FeedbackItem, SurveyRow } from "../logs/types";
import { filterFeedback, splitTail, surveyStats } from "./feedbackStats";

const fb = (text: string, p: Partial<FeedbackItem> = {}): FeedbackItem => ({
  id: 1,
  ts: 0,
  discord_id: "900000000000000009",
  username: "u",
  key_id: 1,
  key_name: "月见草",
  text,
  legacy: false,
  ...p,
});

describe("filterFeedback", () => {
  const items = [
    fb("排队有点久，希望快一点", { id: 1, key_name: "A", username: "alice" }),
    fb("教程很清楚，谢谢站长", { id: 2, key_name: "", username: "Bob", discord_id: "900000000000000002" }),
    // 旧版选择题的记录：服务端拼成一行摘要，照样能搜
    fb("（旧版选择题）整体：满意；困扰：出图失败或报错", {
      id: 3,
      key_name: "C",
      username: "carol",
      legacy: true,
    }),
  ];
  const ids = (xs: FeedbackItem[]) => xs.map((x) => x.id);

  it("关键字为空（或只有空白）= 全部，且保持原顺序", () => {
    expect(ids(filterFeedback(items, ""))).toEqual([1, 2, 3]);
    expect(ids(filterFeedback(items, "   "))).toEqual([1, 2, 3]);
  });
  it("按内容搜", () => {
    expect(ids(filterFeedback(items, "排队"))).toEqual([1]);
    expect(ids(filterFeedback(items, "谢谢"))).toEqual([2]);
    expect(ids(filterFeedback(items, "没有这句话"))).toEqual([]);
  });
  it("按成员搜：Key 名、用户名、Discord ID，不分大小写", () => {
    expect(ids(filterFeedback(items, "ALICE"))).toEqual([1]);
    expect(ids(filterFeedback(items, "carol"))).toEqual([3]);
    expect(ids(filterFeedback(items, "bob"))).toEqual([2]);
    expect(ids(filterFeedback(items, "900000000000000002"))).toEqual([2]);
  });
  it("旧版选择题的记录按摘要搜", () => {
    expect(ids(filterFeedback(items, "旧版选择题"))).toEqual([3]);
    expect(ids(filterFeedback(items, "出图失败"))).toEqual([3]);
  });
});

describe("splitTail", () => {
  it("末尾 n 个字拆出来，前后拼回去等于原文", () => {
    expect(splitTail("排队有点久，希望快一点", 4)).toEqual(["排队有点久，希", "望快一点"]);
  });
  it("短文字、最后一行本来就短（成员自己换的行）不拆", () => {
    expect(splitTail("谢谢", 4)).toEqual(["谢谢", ""]);
    expect(splitTail("1234", 4)).toEqual(["1234", ""]);
    expect(splitTail("排队有点久，希望快一点\n谢谢", 4)).toEqual(["排队有点久，希望快一点\n谢谢", ""]);
  });
  it("按字符拆，不把表情 / 生僻字劈成两半；换行之后的那一行才算末尾", () => {
    expect(splitTail("很好用😀😀😀😀", 4)).toEqual(["很好用", "😀😀😀😀"]);
    expect(splitTail("第一行很长很长很长\n第二行也有五个字", 4)).toEqual([
      "第一行很长很长很长\n第二行也",
      "有五个字",
    ]);
  });
});

describe("surveyStats", () => {
  it("最常用客户端和 Top 改进诉求（多选用「、」分隔）", () => {
    const rows: SurveyRow[] = [
      { ts: 1, client: "柏宝绘 1.2", improve: "多给 V5、出图更快" },
      { ts: 2, client: " 柏宝绘 1.2 ", improve: "多给 V5" },
      { ts: 3, client: "ComfyUI", improve: "" },
    ];
    expect(surveyStats(rows)).toEqual({
      total: 3,
      topClient: ["柏宝绘 1.2", 2],
      topImprove: ["多给 V5", 2],
    });
    expect(surveyStats([]).total).toBe(0);
  });
});
