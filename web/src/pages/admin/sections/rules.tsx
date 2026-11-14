/** 按运营任务汇总成员规则、分享要求、领取条件和全站预算；旧地址继续可用。 */
import { type Tab, TabPage } from "./_tabs";
import Limits from "./limits";
import Onboarding from "./onboarding";

const tabs: Tab[] = [
  { id: "members", label: "成员规则与领取", view: Onboarding },
  { id: "budget", label: "全站预算与运行保护", view: Limits },
];
export default function Rules() {
  return <TabPage title="规则与额度" base="/admin/rules" label="规则分类" tabs={tabs} />;
}
