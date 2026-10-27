/** 系统：告警渠道、后台密码。 */
import { SectionHead } from "./_kit";
import { Sect } from "./system/forms";
import PasswordCard from "./system/password";
import { AlertsCard, LogRetentionCard } from "./system/server";
import "./system.css";

export default function System() {
  return (
    <Sect>
      <SectionHead title="系统" desc="告警渠道和后台密码" />
      <AlertsCard />
      <LogRetentionCard />
      <PasswordCard />
    </Sect>
  );
}
