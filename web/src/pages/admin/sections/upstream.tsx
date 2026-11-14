/** 上游：官方账号、拼车平台、捐赠账号分三页，和日志一样走子路由。 */
import { inSection, type Tab, TabPage } from "./_tabs";
import { OfficialUpstream } from "./upstream/official";
import { UpstreamOverview } from "./upstream/overview";
import { ShareUpstream } from "./upstream/share";
import "./upstream.css";

export const inUpstream = (path: string): boolean => inSection("/admin/upstream", path);

const RelayUpstream = () => <ShareUpstream source="relay" />;
const DonatedUpstream = () => <ShareUpstream source="donated" />;

export default function Upstream() {
  const tabs: Tab[] = [
    { id: "overview", label: "运行总览", view: UpstreamOverview },
    { id: "official", label: "官方账号", view: OfficialUpstream },
    { id: "share", label: "拼车网关", view: RelayUpstream },
    { id: "donated", label: "捐赠账号", view: DonatedUpstream },
  ];
  return <TabPage title="上游" base="/admin/upstream" label="上游类型" tabs={tabs} />;
}
