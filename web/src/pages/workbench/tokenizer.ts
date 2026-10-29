import { Tokenizer as HuggingFaceTokenizer } from "@huggingface/tokenizers";
import { decompressSync } from "fflate";
import { Encoder } from "nai-js-tokenizer";

export type TokenizerKind = "qwen" | "t5" | "clip";
export type TokenizedText = { ids: number[]; tokens: string[]; text: string };

export type TokenEncoder = {
  tokenize(text: string): { ids: number[]; tokens: string[] };
  decode(ids: number[]): string;
  token(id: number): string;
};

const loaded = new Map<TokenizerKind, Promise<TokenEncoder>>();
const assetHash: Record<TokenizerKind, string> = {
  qwen: "f4040d875827d2f9edc30dd4d736bc4a853a19ceeb0d04f6e3648f17796d1667",
  t5: "3cbf698ff2c19efb5c75165d781c1d1b9c035baf119c3e71f1a380cbbfb9dfcb",
  clip: "05852800e8ca59374d5b6d6d3993d9ccea63b7613dc827d12abff64f601a1744",
};

export function tokenizerKind(model: string): TokenizerKind {
  if (model.startsWith("nai-diffusion-5")) return "qwen";
  if (model.startsWith("nai-diffusion-4")) return "t5";
  return "clip";
}

export function tokenizerLabel(model: string): string {
  switch (tokenizerKind(model)) {
    case "qwen":
      return "Qwen 3.5 Tokenizer";
    case "t5":
      return "T5 Tokenizer";
    case "clip":
      return "CLIP Tokenizer";
  }
}

async function load(kind: TokenizerKind): Promise<TokenEncoder> {
  let promise = loaded.get(kind);
  if (promise) return promise;
  promise = (async () => {
    const response = await fetch(`/public/tokenizer/${kind}?sha256=${assetHash[kind]}`, {
      cache: "force-cache",
    });
    if (!response.ok) throw new Error(`分词器资源读取失败（${response.status}）。`);
    const compressed = new Uint8Array(await response.arrayBuffer());
    const data = JSON.parse(new TextDecoder().decode(decompressSync(compressed))) as Record<string, unknown>;
    if (kind === "qwen") return qwenEncoder(data);
    if (kind === "t5") return t5Encoder(data);
    return createClipEncoder(data);
  })();
  loaded.set(kind, promise);
  try {
    return await promise;
  } catch (error) {
    loaded.delete(kind);
    throw error;
  }
}

function qwenEncoder(data: Record<string, unknown>): TokenEncoder {
  const config = data.config as ConstructorParameters<typeof Encoder>[3];
  const encoder = new Encoder(
    data.vocab as Record<string, number>,
    data.merges as string[][],
    data.specialTokens as string[],
    config,
  );
  return {
    tokenize: (text) => {
      const ids = encoder.encode(text);
      return { ids, tokens: ids.map((id) => encoder.decode([id])) };
    },
    decode: (ids) => encoder.decode(ids),
    token: (id) => encoder.decode([id]),
  };
}

function t5Encoder(data: Record<string, unknown>): TokenEncoder {
  const tokenizer = new HuggingFaceTokenizer(data, {});
  return {
    tokenize: (text) => {
      const encoded = tokenizer.encode(text);
      return { ids: encoded.ids, tokens: encoded.tokens };
    },
    decode: (ids) => tokenizer.decode(ids),
    token: (id) => tokenizer.decode([id]),
  };
}

export function createClipEncoder(data: Record<string, unknown>): TokenEncoder {
  const text = typeof data.text === "string" ? data.text : "";
  const lines = text.split("\n");
  const mergePairs = lines
    .slice(1, 48_895)
    .filter(Boolean)
    .map((line) => line.split(" "));
  const printable = [
    ...Array.from({ length: 94 }, (_, index) => index + 33),
    ...Array.from({ length: 12 }, (_, index) => index + 161),
    ...Array.from({ length: 82 }, (_, index) => index + 174),
  ];
  const bytes = new Map<number, string>(printable.map((value) => [value, String.fromCodePoint(value)]));
  let extra = 0;
  for (let value = 0; value < 256; value++) {
    if (!bytes.has(value)) bytes.set(value, String.fromCodePoint(256 + extra++));
  }
  const vocabulary: Record<string, number> = {};
  const byteTokens = [...bytes.values()];
  byteTokens.forEach((token, index) => {
    vocabulary[token] = index;
    vocabulary[`${token}</w>`] = index + 256;
  });
  const mergeRanks = new Map<string, number>();
  mergePairs.forEach(([left, right], index) => {
    if (left && right) {
      vocabulary[left + right] = index + 512;
      mergeRanks.set(`${left}·😎·${right}`, index);
    }
  });
  const specialTokens = ["<|startoftext|>", "<|endoftext|>"];
  vocabulary[specialTokens[0] ?? ""] = 512 + mergePairs.length;
  vocabulary[specialTokens[1] ?? ""] = 513 + mergePairs.length;
  const byId = Object.fromEntries(Object.entries(vocabulary).map(([token, id]) => [id, token]));
  const byteDecoder = new Map([...bytes].map(([byte, token]) => [token, byte]));
  const decoder = new TextDecoder();
  const decodeHtmlEntities = (input: string) =>
    input.replace(
      /&(?:#(\d+)|#x([0-9a-f]+)|amp|lt|gt|quot|apos|nbsp);/gi,
      (entity, decimal: string, hex: string) => {
        const numeric = decimal ? Number(decimal) : hex ? Number.parseInt(hex, 16) : -1;
        if (numeric >= 0 && numeric <= 0x10ffff) return String.fromCodePoint(numeric);
        return (
          { "&amp;": "&", "&lt;": "<", "&gt;": ">", "&quot;": '"', "&apos;": "'", "&nbsp;": " " }[
            entity.toLowerCase()
          ] ?? entity
        );
      },
    );
  const tokenText = (token: string) => {
    if (specialTokens.includes(token)) return token;
    const content = token.endsWith("</w>") ? token.slice(0, -4) : token;
    return decoder.decode(Uint8Array.from([...content].map((char) => byteDecoder.get(char) ?? 0)));
  };
  const bpe = (encoded: string): string[] => {
    const last = [...encoded].at(-1);
    if (!last) return [];
    let word = [...encoded.slice(0, -1), `${last}</w>`];
    while (word.length > 1) {
      let bestRank = Number.POSITIVE_INFINITY;
      let bestPair: string | null = null;
      for (let i = 0; i < word.length - 1; i++) {
        const pair = `${word[i]}·😎·${word[i + 1]}`;
        const rank = mergeRanks.get(pair);
        if (rank !== undefined && rank < bestRank) {
          bestRank = rank;
          bestPair = pair;
        }
      }
      if (bestPair === null) break;
      const [left, right] = bestPair.split("·😎·");
      const next: string[] = [];
      for (let i = 0; i < word.length; ) {
        if (word[i] === left && word[i + 1] === right) {
          next.push(`${left}${right}`);
          i += 2;
        } else {
          const token = word[i];
          if (token) next.push(token);
          i++;
        }
      }
      word = next;
    }
    return word;
  };
  const split =
    /<\|startoftext\|>|<\|endoftext\|>|(?:'s|'t|'re|'ve|'m|'ll|'d)|\p{L}+|\p{N}|[^\s\p{L}\p{N}]+/giu;
  const encode = (input: string) => {
    const cleaned = decodeHtmlEntities(decodeHtmlEntities(input))
      .replace(/[\x5b\x5d{}]/g, " ")
      .trim()
      .replace(/\s+/g, " ")
      .toLowerCase();
    const ids: number[] = [];
    for (const piece of cleaned.match(split) ?? []) {
      if (specialTokens.includes(piece)) {
        ids.push(vocabulary[piece] ?? 0);
        continue;
      }
      const encoded = [...new TextEncoder().encode(piece)].map((byte) => bytes.get(byte) ?? "").join("");
      ids.push(...bpe(encoded).map((token) => vocabulary[token] ?? 0));
    }
    return ids;
  };
  const decode = (ids: number[]) => {
    let output = "";
    let pending: number[] = [];
    const flush = () => {
      if (pending.length) output += decoder.decode(Uint8Array.from(pending));
      pending = [];
    };
    for (const id of ids) {
      const token = byId[id] ?? "";
      if (specialTokens.includes(token)) {
        flush();
        output += token;
        continue;
      }
      const content = token.endsWith("</w>") ? token.slice(0, -4) : token;
      for (const char of content) {
        const byte = byteDecoder.get(char);
        if (byte === undefined) {
          flush();
          output += char;
        } else {
          pending.push(byte);
        }
      }
      if (token.endsWith("</w>")) pending.push(32);
    }
    flush();
    return output.trim();
  };
  return {
    tokenize: (input) => {
      const ids = encode(input);
      return { ids, tokens: ids.map((id) => tokenText(byId[id] ?? "")) };
    },
    decode,
    token: (id) => tokenText(byId[id] ?? ""),
  };
}

export async function tokenizeText(model: string, text: string): Promise<TokenizedText> {
  const encoder = await load(tokenizerKind(model));
  const { ids, tokens } = encoder.tokenize(text);
  return { ids, tokens, text: encoder.decode(ids) };
}

export async function decodeTokenIds(model: string, input: string): Promise<TokenizedText> {
  const ids = input
    .trim()
    .split(/[\s,]+/)
    .filter(Boolean)
    .map(Number);
  if (ids.some((id) => !Number.isInteger(id) || id < 0))
    throw new Error("请输入空格或逗号分隔的非负 token ID。");
  const encoder = await load(tokenizerKind(model));
  return { ids, tokens: ids.map((id) => encoder.token(id)), text: encoder.decode(ids) };
}
