import type { MyRequest } from "./types";

/** 工具类请求（不是出图）的名称：这类请求也要花 Anlas，光看模型名会以为是出图。 */
export const TOOL_KINDS: Record<string, string> = {
  vibe_encode: "Vibe 编码",
  upscale: "放大",
  "augment-image": "导演工具",
  tags: "标签补全",
};

/** 请求 → 短名：工具类优先显示工具名，出图才显示 V5 / V4.5 / V4 / V3。 */
export function modelLabel(r: Pick<MyRequest, "model" | "kind">): string {
  const tool = TOOL_KINDS[r.kind ?? ""];
  if (tool) return tool;
  const m = r.model ?? "";
  if (/diffusion-5/.test(m)) return "V5";
  if (/diffusion-4-5/.test(m)) return "V4.5";
  if (/diffusion-4/.test(m)) return "V4";
  if (/diffusion-3|furry/.test(m)) return "V3";
  return r.kind || "请求";
}
