/** 生成记录：成员生成过的图（画廊 / 大图 / 列表，可多选成员一键导出）+ 记录相关设置（开关、保留时长、容量、清空原图）。 */
import { useAdmin } from "../Admin";
import { LoadError, Loading } from "./_kit";
import { TabPage } from "./_tabs";
import { Audit } from "./audit/Records";
import AuditCard from "./audit/Settings";
import type { Ops } from "./onboarding/core";
import { Sect } from "./system/forms";
import { StorageCard } from "./system/server";

function Settings() {
  const ops = useAdmin<Ops>("/admin/api/ops");
  return (
    <Sect>
      {ops.loading && !ops.data && <Loading rows={3} />}
      {!ops.data && ops.error && <LoadError error={ops.error} />}
      {ops.data && <AuditCard ops={ops.data} />}
      <StorageCard />
    </Sect>
  );
}

export default function AuditSection() {
  return (
    <TabPage
      title="生成记录"
      base="/admin/audit"
      label="生成记录"
      tabs={[
        { id: "records", label: "记录", view: Audit },
        { id: "settings", label: "设置", view: Settings },
      ]}
    />
  );
}
