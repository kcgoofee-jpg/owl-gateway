/** 上游：官方账号 / 拼车与捐赠，两页，和日志一样走子路由。 */
import { inSection, type Tab, TabPage } from "./_tabs";
import { OfficialUpstream } from "./upstream/official";
import { ShareUpstream } from "./upstream/share";
import "./upstream.css";

export const inUpstream = (path: string): boolean => inSection("/admin/upstream", path);

export default function Upstream() {
  const tabs: Tab[] = [
    { id: "official", label: "官方账号", view: OfficialUpstream },
    { id: "share", label: "拼车与捐赠", view: ShareUpstream },
  ];
  return <TabPage title="上游" base="/admin/upstream" label="上游类型" tabs={tabs} />;
}
