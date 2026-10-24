/**
 * 来信：成员问卷 / 文字反馈两个子页（/admin/inbox/survey、/admin/inbox/feedback）。
 * 分区根路径换到问卷页；日志页搬走的旧地址（/admin/logs/{survey,feedback}）也换到这里。
 * 文字反馈不再折在问卷页最底部：低流量也要能一眼找到（两个 tab，URL 可收藏）。
 */
import { Badge } from "../../../ui";
import { useAdmin } from "../Admin";
import { type Tab, TabPage } from "./_tabs";
import { Feedback } from "./inbox/Feedback";
import { Survey } from "./inbox/Survey";
import type { FeedbackResp } from "./logs/types";

export default function Inbox() {
  const feedback = useAdmin<FeedbackResp>("/admin/api/feedback");
  const count = feedback.data?.items.length ?? 0;
  const tabs: Tab[] = [
    { id: "survey", label: "问卷", view: Survey },
    {
      id: "feedback",
      label: "文字反馈",
      view: Feedback,
      ...(count > 0 ? { badge: <Badge tone="info">{count}</Badge> } : {}),
    },
  ];
  return <TabPage title="来信" base="/admin/inbox" label="来信类型" tabs={tabs} />;
}
