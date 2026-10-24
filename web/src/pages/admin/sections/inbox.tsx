/**
 * 来信：成员问卷一页看完，文字反馈收在底部折叠区（流量低，默认收起）。
 * 问卷路径 /admin/inbox/survey 不变；分区根路径和旧的反馈子页都换到问卷（做法同 logs.tsx）。
 */
import { useEffect } from "preact/hooks";
import { useLocation } from "preact-iso";
import { SectionHead } from "./_kit";
import { Feedback } from "./inbox/Feedback";
import { Survey } from "./inbox/Survey";
import "./inbox/inbox.css";
import "./logs.css";

/** 地址是否在来信分区之下（/admin/inbox 或 /admin/inbox/…） */
export const inInbox = (path: string): boolean => path === "/admin/inbox" || path.startsWith("/admin/inbox/");

const SURVEY = "/admin/inbox/survey";

export default function Inbox() {
  const { path, query, route } = useLocation();
  const seg = path.split("/")[3] ?? "";
  const moved = seg === "" || seg === "feedback" ? SURVEY : "";
  // 只在真的还在这个分区下才跳：各页是懒加载的，点别的栏目时旧页会带着新地址的 path 多渲染一阵
  useEffect(() => {
    if (!moved || !inInbox(path)) return;
    const qs = new URLSearchParams(query).toString();
    route(qs ? `${moved}?${qs}` : moved, true);
  }, [moved, path, query, route]);
  if (moved) return null;
  return (
    <div class="xa-page">
      <SectionHead title="来信" desc="成员问卷；文字反馈在页面底部。" />
      <Survey />
      <Feedback />
    </div>
  );
}
