/** 日志：请求、操作和提示词记录。 */
import { inSection, type Tab, TabPage } from "./_tabs";
import { Actions } from "./logs/Actions";
import { Prompts } from "./logs/Prompts";
import { Usage } from "./logs/Usage";

export const inLogs = (path: string): boolean => inSection("/admin/logs", path);

export default function Logs() {
  const tabs: Tab[] = [
    { id: "usage", label: "请求日志", view: Usage },
    { id: "actions", label: "操作日志", view: Actions },
    { id: "prompts", label: "提示词记录", view: Prompts },
  ];
  return <TabPage title="日志" base="/admin/logs" label="日志类型" tabs={tabs} />;
}
