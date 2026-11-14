/** 全站维护模式：正常 / 全关 / 只有站长可用。 */
import { useState } from "preact/hooks";
import { Card } from "../../../../ui";
import { act, useAdmin } from "../../data";

type Mode = "off" | "all" | "owner";

const COPY: Record<Mode, string> = {
  off: "所有人正常使用。",
  all: "所有人收维护响应；后台和健康检查不受影响。",
  owner: "只有站长网页登录和绑定站长 Discord 的 Key 能出图，其他人收维护响应。",
};

export function MaintenanceCard() {
  const { data } = useAdmin<{ maintenance?: { enabled: boolean; mode?: Mode } }>("/admin/api/ops");
  const [saving, setSaving] = useState(false);
  const maintenance = data?.maintenance;
  if (!maintenance) return null;
  const current: Mode = maintenance.mode ?? (maintenance.enabled ? "all" : "off");
  const change = async (next: Mode) => {
    if (saving || next === current) return;
    setSaving(true);
    await act("PUT", "/admin/api/ops/maintenance", { mode: next }, COPY[next]);
    setSaving(false);
  };
  return (
    <Card title="全站维护" sub="后台和健康检查始终可用，已开始的请求继续完成。">
      <div class="field">
        <label class="field__label" for="maintenance-mode">
          当前状态
        </label>
        <select
          id="maintenance-mode"
          class="select"
          value={current}
          disabled={saving}
          onChange={(e) => void change(e.currentTarget.value as Mode)}
        >
          <option value="off">正常开放</option>
          <option value="all">全站暂停</option>
          <option value="owner">只有站长可用</option>
        </select>
        <span class="field__hint">{COPY[current]}</span>
      </div>
    </Card>
  );
}
