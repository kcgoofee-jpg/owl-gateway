export const PROHIBITED_USE = [
  "商业化使用",
  "共享、转借、倒卖或公开 Key",
  "绕过限制或异常高频调用",
  "提交违法、极端有害或违反上游规则的内容",
  "干扰服务或影响其他用户正常使用",
] as const;

export function UsageRules() {
  return (
    <ul class="lp-use-rules">
      {PROHIBITED_USE.map((rule) => (
        <li key={rule}>{rule}</li>
      ))}
    </ul>
  );
}
