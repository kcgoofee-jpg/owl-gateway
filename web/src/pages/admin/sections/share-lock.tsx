/** 分享要求：开 / 关、执行档位、检测是否就绪。细则见 docs/用户限制说明.md。 */
import { useState } from "preact/hooks";
import { fullTime } from "../../../lib/format";
import { Badge, Card, Notice } from "../../../ui";
import { adminWrite, useAdmin } from "../data";
import { CardPending } from "./_kit";
import { SwitchRow } from "./system/forms";

export type ShareLockStatus = {
  enabled: boolean;
  mode: "quota" | "daily";
  enforcing: boolean;
  healthy: boolean;
  bot_online: boolean;
  channel_url: string;
  detection: {
    connected: boolean;
    last_complete_at: number;
    pending: number;
    error: string | null;
  };
};

const MODE_COPY = {
  quota: "昨日用满个人额度后，昨日或今日在频道分享一张图即可继续。",
  daily: "除站长和赞助席外，所有成员每天都要分享一张图。",
} as const;

export function ShareLockCard() {
  const query = useAdmin<ShareLockStatus>("/admin/api/ops/share-lock");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  if (!query.data) return <CardPending title="分享要求" loading={query.loading} error={query.error} />;
  const data = query.data;
  const change = async (changes: { enabled: boolean } | { mode: "quota" | "daily" }) => {
    setSaving(true);
    setError("");
    const result = await adminWrite("PUT", "/admin/api/ops/share-lock", changes, {
      okText: "分享限制已保存",
    });
    if (!result.ok) setError(result.error);
    setSaving(false);
  };
  return (
    <Card
      title="分享要求"
      sub="只暂停生图，保留 Key、登录和身份组；图片被确认后自动恢复。每天 07:00 重置。"
      actions={
        <a class="small" href={data.channel_url} target="_blank" rel="noreferrer">
          打开分享频道
        </a>
      }
    >
      <div class="stack-sm">
        <SwitchRow
          title="开启分享限制"
          checked={data.enabled}
          disabled={saving}
          onChange={(v) => void change({ enabled: v })}
        />
        <div class="field">
          <label class="field__label" for="share-lock-mode">
            执行档位
          </label>
          <select
            id="share-lock-mode"
            class="select"
            value={data.mode}
            disabled={saving}
            onChange={(e) => void change({ mode: e.currentTarget.value === "daily" ? "daily" : "quota" })}
          >
            <option value="quota">昨日用满额度后需分享</option>
            <option value="daily">所有非豁免成员需分享</option>
          </select>
          <span class="field__hint">{MODE_COPY[data.mode]}</span>
        </div>
        {error && (
          <p class="field__error" role="alert">
            {error}
          </p>
        )}
        <p class="small muted">
          <Badge tone={data.bot_online ? "ok" : "warn"}>Bot {data.bot_online ? "在线" : "离线"}</Badge>{" "}
          <Badge tone={data.healthy ? "ok" : "warn"}>分享检测{data.healthy ? "正常" : "待恢复"}</Badge> ·
          最近完整核对{" "}
          {data.detection.last_complete_at ? fullTime(data.detection.last_complete_at) : "尚未完成"}
          {data.detection.pending > 0 && ` · 待确认 ${data.detection.pending}`}
        </p>
        {!data.enabled ? (
          <Notice>分享限制已关闭，检测继续运行。</Notice>
        ) : !data.enforcing ? (
          <Notice tone="warn">检测尚未就绪，现暂时放行；完成补扫后自动恢复限制。</Notice>
        ) : !data.healthy ? (
          <Notice tone="warn">检测正在恢复，仍按最近一次完整核对执行。</Notice>
        ) : null}
        {data.detection.error && <Notice tone="warn">{data.detection.error}</Notice>}
      </div>
    </Card>
  );
}
