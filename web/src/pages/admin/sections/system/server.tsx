/** 告警渠道设置。 */
import { useState } from "preact/hooks";
import { Badge, Button, Card } from "../../../../ui";
import { act, useAdmin } from "../../Admin";
import { actOrError, ConfirmDialog } from "../_kit";
import { SwitchRow, withTone } from "./forms";

export type ServerStatus = {
  alerts: { configured: boolean; sent: number };
};

type MaintenanceMode = "off" | "all" | "owner";

export function MaintenanceCard() {
  const { data } = useAdmin<{ maintenance: { enabled: boolean; mode?: MaintenanceMode } }>("/admin/api/ops");
  const [override, setOverride] = useState<MaintenanceMode | null>(null);
  const [saving, setSaving] = useState(false);
  if (!data) return null;
  const current: MaintenanceMode =
    override ?? data.maintenance.mode ?? (data.maintenance.enabled ? "all" : "off");
  const change = async (next: MaintenanceMode) => {
    if (saving || next === current) return;
    setOverride(next);
    setSaving(true);
    const result = await act<{ enabled: boolean; mode?: MaintenanceMode }>(
      "PUT",
      "/admin/api/ops/maintenance",
      { mode: next },
      next === "off" ? "全站服务已恢复" : next === "all" ? "全站服务已暂停" : "维护中：只有站长账号可用",
    );
    setOverride(result?.mode ?? next);
    setSaving(false);
  };
  const options: Array<{ id: MaintenanceMode; title: string; desc: string }> = [
    { id: "off", title: "正常开放", desc: "所有人可以访问页面和出图。" },
    { id: "all", title: "全部关闭", desc: "访客看维护页，成员和客户端请求收到 503；后台仍可用。" },
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

export function AlertsCard() {
  const { data } = useAdmin<ServerStatus>("/admin/api/status");
  const [open, setOpen] = useState(false);
  if (!data) return null;
  const a = data.alerts;
  return (
    <Card
      title="告警渠道"
      sub="只在重大事故时私信站长：上游 Token 失效、账号可能被封控、服务停了、磁盘快满。"
      actions={
        <Badge {...withTone(a.configured ? "ok" : "warn")}>{a.configured ? "已配置" : "未配置"}</Badge>
      }
    >
      <div class="stack-sm">
        <p class="small">
          {a.configured
            ? `已发送 ${a.sent} 条告警。`
            : "在服务器的 .env 里设置 DISCORD_BOT_TOKEN 和 ALERT_USER_ID 后，机器人会私信你。"}
        </p>
        <div class="actions">
          <Button disabled={!a.configured} onClick={() => setOpen(true)}>
            发送测试告警
          </Button>
        </div>
      </div>
      <ConfirmDialog
        open={open}
        title="发送测试告警？"
        confirmText="发送"
        onClose={() => setOpen(false)}
        onConfirm={() =>
          actOrError("POST", "/admin/api/alerts/test", undefined, "测试告警已发出，请检查告警渠道")
        }
      >
        <p>会向告警渠道发一条真实的测试消息（10 秒内只能发一次）。</p>
      </ConfirmDialog>
    </Card>
  );
}
