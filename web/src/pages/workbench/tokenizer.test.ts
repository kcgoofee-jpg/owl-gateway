import { describe, expect, it } from "vitest";
import { createClipEncoder, tokenizerKind, tokenizerLabel } from "./tokenizer";

describe("NovelAI image tokenizer selection", () => {
  it("matches the official encoder families for V5, V4/V4.5, and V3", () => {
    expect(tokenizerKind("nai-diffusion-5-curated")).toBe("qwen");
    expect(tokenizerKind("nai-diffusion-5-full")).toBe("qwen");
    expect(tokenizerKind("nai-diffusion-4-5-full")).toBe("t5");
    expect(tokenizerKind("nai-diffusion-4-curated")).toBe("t5");
    expect(tokenizerKind("nai-diffusion-furry-3")).toBe("clip");
    expect(tokenizerLabel("nai-diffusion-5-curated")).toBe("Qwen 3.5 Tokenizer");
  });

  it("applies CLIP byte-level merges and decodes the merged token", () => {
    const encoder = createClipEncoder({ text: "#version: 0.2\nr e\nre d</w>" });
    const result = encoder.tokenize("red");
    expect(result).toEqual({ ids: [513], tokens: ["red"] });
    expect(encoder.decode(result.ids)).toBe("red");
  });
});
