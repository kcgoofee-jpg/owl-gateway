/** 告警渠道；生成记录的原图存储（清空原图，在「生成记录」页用）。 */
import { useState } from "preact/hooks";
import { withUnit } from "../../../../lib/format";
import { Badge, Button, Card, Notice } from "../../../../ui";
import { useAdmin } from "../../Admin";
import { actOrError, ConfirmDialog } from "../_kit";
import type { AuditInfo } from "../onboarding/core";
import { bytesText } from "./core";
import { withTone } from "./forms";

export type ServerStatus = {
  alerts: { configured: boolean; sent: number };
  audit: AuditInfo;
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

export function StorageCard() {
  const { data } = useAdmin<ServerStatus>("/admin/api/status");
  const [open, setOpen] = useState(false);
  if (!data) return null;
  const a = data.audit;
  return (
    <Card title="清空原图" sub="只删原图，提示词和参数保留">
      <div class="actions">
        <Button variant="danger" disabled={a.image_bytes <= 0} onClick={() => setOpen(true)}>
          清空服务器上的全部原图
        </Button>
      </div>
      <ConfirmDialog
        open={open}
        title="清空全部原图？"
        confirmText="清空"
        danger
        phrase="清空"
        onClose={() => setOpen(false)}
        onConfirm={() =>
          actOrError<{ purged: number }>(
            "POST",
            "/admin/api/audit/purge-images",
            { confirm: "清空" },
            (r) => `已清空 ${withUnit(r.purged, "张原图")}`,
          )
        }
      >
        <Notice tone="danger">
          将删除服务器上的全部原图（当前占用 {bytesText(a.image_bytes)}
          ），且无法恢复。提示词和参数保留。
        </Notice>
      </ConfirmDialog>
    </Card>
  );
}
