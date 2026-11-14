/** 系统：维护模式、后台密码、临时调试记录。 */
import { SectionHead } from "./_kit";
import { DebugRecordCard } from "./system/debug-record";
import { Sect } from "./system/forms";
import PasswordCard from "./system/password";
import { MaintenanceCard } from "./system/server";
import "./system.css";

export default function System() {
  return (
    <Sect>
      <SectionHead title="系统" desc="全站维护、后台密码和临时调试记录" />
      <MaintenanceCard />
      <DebugRecordCard />
      <PasswordCard />
    </Sect>
  );
}
