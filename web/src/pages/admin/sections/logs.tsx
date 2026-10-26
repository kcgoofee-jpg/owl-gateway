/** 日志：请求日志 / 操作日志 / Bug 追踪。生成记录是独立分区。 */
import { useEffect } from "preact/hooks";
import { useLocation } from "preact-iso";
import { Badge } from "../../../ui";
import { inSection, type Tab, TabPage } from "./_tabs";
import { Actions } from "./logs/Actions";
import { Bugs } from "./logs/Bugs";
import { Usage } from "./logs/Usage";
import { useOpenBugs } from "./members/shared";

/** 旧地址（生成记录原来在日志下）：换到新分区，查询串照带 */
export const MOVED: Record<string, string> = {
  audit: "/admin/audit/records",
};

export const inLogs = (path: string): boolean => inSection("/admin/logs", path);

export default function Logs() {
  const { path, query, route } = useLocation();
  const open = useOpenBugs();
  const moved = MOVED[path.split("/")[3] ?? ""];
  useEffect(() => {
    if (!moved || !inLogs(path)) return;
    const qs = new URLSearchParams(query).toString();
    route(qs ? `${moved}?${qs}` : moved, true);
  }, [moved, path, query, route]);
  const tabs: Tab[] = [
    // id 还叫 usage：旧链接（/admin/logs/usage?...）照常能打开
    { id: "usage", label: "请求日志", view: Usage },
    { id: "actions", label: "操作日志", view: Actions },
    { id: "bugs", label: "Bug 追踪", view: Bugs, badge: open > 0 && <Badge tone="danger">{open}</Badge> },
  ];
  if (moved) return null;
  return <TabPage title="日志" base="/admin/logs" label="日志类型" tabs={tabs} />;
}
