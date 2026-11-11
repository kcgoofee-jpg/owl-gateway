/** 首页和后台共用的请求短名、交付徽标。工具名和模型短名只在这里写一次。 */

export const TOOL_LABELS: Record<string, string> = {
  vibe_encode: "Vibe 编码",
  upscale: "放大",
  "augment-image": "导演工具",
  tags: "标签补全",
};

/** 模型短名：nai-diffusion-4-5-full → V4.5。Furry、Curated、重绘分开。 */
export function shortModel(model: string | null | undefined): string {
  const m = String(model ?? "");
  if (!m) return "";
  const base = /diffusion-5/.test(m)
    ? "V5"
    : /diffusion-4-5/.test(m)
      ? "V4.5"
      : /diffusion-4/.test(m)
        ? "V4"
        : /furry-3/.test(m)
          ? "Furry V3"
          : /diffusion-3/.test(m)
            ? "V3"
            : /diffusion-2/.test(m)
              ? "V2"
              : /furry/.test(m)
                ? "Furry"
                : /^nai-diffusion$/.test(m)
                  ? "V1"
                  : "";
  if (!base) return m.length > 18 ? `${m.slice(0, 16)}…` : m;
  return base + (/curated/.test(m) ? " Curated" : "") + (/inpainting/.test(m) ? " 重绘" : "");
}

/** 请求短名：工具类先显示工具名，出图再显示模型短名。 */
export function modelLabel(r: { model?: string | null; kind?: string | null }): string {
  const tool = TOOL_LABELS[r.kind ?? ""];
  if (tool) return tool;
  return shortModel(r.model) || r.kind || "请求";
}

/** 交付徽标只留异常态。传输完成和没有状态都不显示。 */
export function deliveryLabel(
  state: string | null | undefined,
): { text: string; tone: "warn" | "danger" } | null {
  switch (state) {
    case "transport_interrupted":
      return { text: "传输中断", tone: "danger" };
    case "pending":
      return { text: "传输中", tone: "warn" };
    default:
      return null;
  }
}
