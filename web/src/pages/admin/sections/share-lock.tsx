import { useState } from "preact/hooks";
import { fullTime } from "../../../lib/format";
import { Badge, Card, Notice } from "../../../ui";
import { act, useAdmin } from "../data";
import { CardPending } from "./_kit";
import { Kv, SwitchRow } from "./system/forms";

export type ShareLockStatus = {
  enabled: boolean;
  mode: "quota" | "daily";
  enforcing: boolean;
  healthy: boolean;
  bot_online: boolean;
  channel_url: string;
  chat?: {
    running: number;
    waiting: number;
    circuit_open: boolean;
    worker_connected?: boolean;
    last_model_result?: { state: string; category: string; at: number } | null;
  };
  reactions?: { enabled: boolean; pending: number; error: string | null };
  detection: {
    connected: boolean;
    last_complete_at: number;
    complete_day_start: number;
    coverage_start?: number | null;
    pending: number;
    oldest_failure_at: number | null;
    error: string | null;
  };
};

export function ShareLockCard() {
  const query = useAdmin<ShareLockStatus>("/admin/api/ops/share-lock");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  if (!query.data) return <CardPending title="分享要求" loading={query.loading} error={query.error} />;
  const data = query.data;
  const change = async (changes: { enabled: boolean } | { mode: "quota" | "daily" }) => {
    setSaving(true);
    setError("");
    const result = await act("PUT", "/admin/api/ops/share-lock", changes, "分享限制已保存");
    if (result === null) setError("保存失败，请重试；开关仍按已保存的设置执行。");
    setSaving(false);
  };
  return (
    <Card title="分享要求">
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
        </div>
        {error && (
          <p class="field__error" role="alert">
            {error}
          </p>
        )}
        <p>
          {data.mode === "daily"
            ? "每天北京时间 07:00 重置。除站长和已开通赞助席的成员外，所有成员昨日或今日需分享一张图才能生图，不要求昨天用满额度。"
            : "每天北京时间 07:00 重置。昨日用满 V4.5 或 V5 个人额度后，昨日或今日在指定频道分享即可继续，昨日已分享无需重复。站长和赞助席豁免。"}
          新帖和已有帖子补图都算。
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
            ["待确认 / 重试", data.detection.pending],
            [
              "完整覆盖起点",
              data.detection.coverage_start ? fullTime(data.detection.coverage_start) : "尚未核实昨日记录",
            ],
            ["聊天工作进程", data.chat?.worker_connected ? "可连接" : "不可连接 / 尚未核实"],
            ["聊天处理 / 等待", data.chat ? `${data.chat.running} / ${data.chat.waiting}` : "未知"],
            [
              "聊天上游",
              data.chat ? (data.chat.circuit_open ? "熔断冷却" : "未熔断（不代表上游必定可用）") : "未知",
            ],
            [
              "最近模型结果",
              data.chat?.last_model_result
                ? `${data.chat.last_model_result.state === "success" ? "成功" : "失败"} · ${fullTime(data.chat.last_model_result.at)}`
                : "尚无请求记录",
            ],
            ["反应待发送", data.reactions?.pending ?? "未知"],
            ["最早异常", data.detection.oldest_failure_at ? fullTime(data.detection.oldest_failure_at) : "—"],
          ]}
        />
        {data.reactions?.error && <Notice tone="warn">{data.reactions.error}</Notice>}
        {data.detection.error && <Notice tone="warn">{data.detection.error}</Notice>}
        {!data.enabled ? (
          <Notice>分享限制已关闭，检测继续运行。</Notice>
        ) : !data.enforcing ? (
          <Notice tone="warn">
            分享检测尚未就绪或超过 5
            分钟未可靠同步，现暂时放行。完成昨日及本服务日补扫并清空重试后自动恢复限制。
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
