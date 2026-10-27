/** 日志相关的显示格式化（纯函数）：模型短名、功能名、耗时。 */
import { secondsText } from "../../../../lib/format";
import { FEATURES, RETIRED } from "../features";

/** 请求日志 kind → 功能 id（后端 features.KIND_FEATURE 的副本） */
const KIND_FEATURE: Record<string, string> = {
  image: "image",
  image_stream: "image",
  upscale: "upscale",
  "augment-image": "augment",
  vibe_encode: "vibe",
  tags: "tags",
  text: "text",
  chat: "text",
  voice: "voice",
};

export function kindLabel(kind: string | null | undefined): string {
  const id = KIND_FEATURE[kind ?? ""];
  const f = FEATURES.find((x) => x.id === id) ?? RETIRED.find((x) => x.id === id);
  return f ? f.short : kind || "—";
}

/** 模型短名：nai-diffusion-4-5-full → V4.5（完整名放 title） */
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

/** 耗时：生成 4.2 秒、等 12 秒（等待 < 100ms 不显示） */
export function timing(waitMs: number, durMs: number): { gen: string; wait: string } {
  return { gen: durMs ? secondsText(durMs) : "", wait: waitMs >= 100 ? secondsText(waitMs) : "" };
}
