import { useState } from "preact/hooks";
import { fullTime } from "../../../lib/format";
import { Badge, Card, Notice } from "../../../ui";
import { act, useAdmin } from "../data";
import { CardPending } from "./_kit";
import { Kv, SwitchRow } from "./system/forms";

export type ShareLockStatus = {
  enabled: boolean;
  enforcing: boolean;
  healthy: boolean;
  bot_online: boolean;
  channel_url: string;
  detection: {
    connected: boolean;
    last_complete_at: number;
    complete_day_start: number;
    pending: number;
    oldest_failure_at: number | null;
    error: string | null;
  };
};

export function ShareLockCard() {
  const query = useAdmin<ShareLockStatus>("/admin/api/ops/share-lock");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  if (!query.data)
    return <CardPending title="用满额度后需分享" loading={query.loading} error={query.error} />;
  const data = query.data;
  const change = async (enabled: boolean) => {
    setSaving(true);
    setError("");
    const result = await act("PUT", "/admin/api/ops/share-lock", { enabled }, "分享限制已保存");
    if (result === null) setError("保存失败，请重试；开关仍按已保存的设置执行。");
    setSaving(false);
  };
  return (
    <Card title="用满额度后需分享">
      <div class="stack-sm">
        <SwitchRow
          title="开启分享限制"
          checked={data.enabled}
          disabled={saving}
          onChange={(v) => void change(v)}
        />
        {error && (
          <p class="field__error" role="alert">
            {error}
          </p>
        )}
        <p>
          每天北京时间 07:00 重置。昨日用满 V4.5 或 V5
          个人额度后，今天需在指定频道发一张图才能继续。新帖和已有帖子补图都算，站长和赞助席豁免。
        </p>
        <p>只暂停生图，保留 Key、登录和频道身份组；图片被确认后自动恢复。</p>
        <p>
          <a href={data.channel_url} target="_blank" rel="noreferrer">
            打开分享频道
          </a>
        </p>
        <p>
          <Badge tone={data.bot_online ? "ok" : "warn"}>Bot {data.bot_online ? "在线" : "离线"}</Badge>{" "}
          <Badge tone={data.healthy ? "ok" : "warn"}>分享检测{data.healthy ? "正常" : "待恢复"}</Badge>
        </p>
        <Kv
          rows={[
            ["Discord 连接", data.detection.connected ? "已连接" : "未连接"],
            [
              "最近完整核对",
              data.detection.last_complete_at ? fullTime(data.detection.last_complete_at) : "尚未完成",
            ],
            ["待重试记录", data.detection.pending],
            ["最早异常", data.detection.oldest_failure_at ? fullTime(data.detection.oldest_failure_at) : "—"],
          ]}
        />
        {data.detection.error && <Notice tone="warn">{data.detection.error}</Notice>}
        {!data.enabled ? (
          <Notice>分享限制已关闭，检测继续运行。</Notice>
        ) : !data.enforcing ? (
          <Notice tone="warn">
            分享检测尚未就绪或超过 5 分钟未可靠同步，现暂时放行。完成本服务日补扫并清空重试后自动恢复限制。
          </Notice>
        ) : !data.healthy ? (
          <Notice tone="warn">
            检测正在恢复，当前仍按最近一次完整核对执行；超过 5 分钟未可靠同步将暂时放行。
          </Notice>
        ) : null}
      </div>
    </Card>
  );
}
