/** 告警渠道和请求来源保留设置。 */
import { useState } from "preact/hooks";
import { Badge, Button, Card } from "../../../../ui";
import { useAdmin } from "../../Admin";
import { actOrError, ConfirmDialog } from "../_kit";
import { withTone } from "./forms";

export type ServerStatus = {
  alerts: { configured: boolean; sent: number };
};

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

export function LogRetentionCard() {
  const { data } = useAdmin<{ log_retention: { origin_keep_days: number } }>("/admin/api/ops");
  const [days, setDays] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  if (!data) return null;
  const value = days ?? String(data.log_retention.origin_keep_days);
  return (
    <Card title="请求来源保留" sub="只影响请求日志里的 IP 和客户端信息；0 表示长期保留。">
      <form
        onSubmit={async (e) => {
          e.preventDefault();
          setSaving(true);
          const ok = await actOrError(
            "PUT",
            "/admin/api/ops/log-retention",
            { origin_keep_days: Number(value) },
            "请求来源保留设置已保存",
          );
          setSaving(false);
          if (ok) setDays(null);
        }}
      >
        <label class="field">
          IP / UA 保留天数
          <input
            class="input"
            type="number"
            min="0"
            max="365"
            step="1"
            required
            value={value}
            onInput={(e) => setDays(e.currentTarget.value)}
          />
        </label>
        <Button type="submit" disabled={saving || days === null}>
          保存保留设置
        </Button>
      </form>
    </Card>
  );
}
