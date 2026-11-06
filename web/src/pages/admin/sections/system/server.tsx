/** 全站维护模式：正常 / 全关 / 只有站长可用。 */
import { useState } from "preact/hooks";
import { Card } from "../../../../ui";
import { act, useAdmin } from "../../Admin";
import { SwitchRow } from "./forms";

type MaintenanceMode = "off" | "all" | "owner";

export function MaintenanceCard() {
  const { data } = useAdmin<{ maintenance: { enabled: boolean; mode?: MaintenanceMode } }>("/admin/api/ops");
  const [saving, setSaving] = useState(false);
  if (!data) return null;
  const current: MaintenanceMode = data.maintenance.mode ?? (data.maintenance.enabled ? "all" : "off");
  const change = async (next: MaintenanceMode) => {
    if (saving || next === current) return;
    setSaving(true);
    await act<{ enabled: boolean; mode?: MaintenanceMode }>(
      "PUT",
      "/admin/api/ops/maintenance",
      { mode: next },
      next === "off" ? "全站服务已恢复" : next === "all" ? "全站服务已暂停" : "维护中：只有站长账号可用",
    );
    setSaving(false);
  };
  const options: { id: MaintenanceMode; title: string; desc: string }[] = [
    { id: "off", title: "正常开放", desc: "所有人正常使用。" },
    { id: "all", title: "全站暂停", desc: "所有人收维护响应；后台和健康检查不受影响。" },
    {
      id: "owner",
      title: "只有站长可用",
      desc: "除站长本人的网页登录外，其他人和客户端都收维护响应，方便自己先验证新版本。",
    },
  ];
  return (
    <Card
      title="全站维护"
      sub="三档：正常开放 / 全部关闭 / 只有站长可用。后台和健康检查始终可用，已开始的请求继续完成。"
    >
      <div class="stack-sm">
        {options.map((option) => (
          <SwitchRow
            key={option.id}
            title={option.title}
            desc={option.desc}
            checked={current === option.id}
            disabled={saving}
            onChange={() => void change(option.id)}
          />
        ))}
      </div>
    </Card>
  );
}
