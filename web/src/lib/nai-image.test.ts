/** nai-image.ts：文件头识别、ZIP 解包（中央目录 + deflate-raw）、data URL。 */
import { describe, expect, it } from "vitest";
import { extractImages, type ImageMime, isZip, sniffImage, toDataUrl } from "./nai-image";

/** 一张真的 1×1 PNG（网关测试夹具同款思路：能被识别即可）。 */
const PNG_1X1 = Uint8Array.from(
  atob("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII="),
  (c) => c.charCodeAt(0),
);
const JPEG_1X1 = Uint8Array.from([0xff, 0xd8, 0xff, 0xe0, 0, 0x10, 0x4a, 0x46, 0x49, 0x46, 0, 1, 0, 1, 0, 0]);
const WEBP_1X1 = Uint8Array.from([
  0x52, 0x49, 0x46, 0x46, 0, 0, 0, 0, 0x57, 0x45, 0x42, 0x50, 0x56, 0x50, 0x38, 0x20,
]);

const CRC_TABLE = (() => {
  const t = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c >>> 0;
  }
  return t;
})();
const crc32 = (d: Uint8Array): number => {
  let c = 0xffffffff;
  for (const b of d) c = (CRC_TABLE[(c ^ b) & 0xff] ?? 0) ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
};

class Writer {
  private buf = new Uint8Array(1024);
  private len = 0;
  private grow(n: number) {
    if (this.len + n <= this.buf.length) return;
    let cap = this.buf.length * 2;
    while (cap < this.len + n) cap *= 2;
    const next = new Uint8Array(cap);
    next.set(this.buf.subarray(0, this.len));
    this.buf = next;
  }
  u8(v: number) {
    this.grow(1);
    this.buf[this.len++] = v & 0xff;
  }
  u16(v: number) {
    this.grow(2);
    this.buf[this.len++] = v & 0xff;
    this.buf[this.len++] = (v >>> 8) & 0xff;
  }
  u32(v: number) {
    this.grow(4);
    for (let i = 0; i < 4; i++) this.buf[this.len++] = (v >>> (8 * i)) & 0xff;
  }
  bytes(d: Uint8Array) {
    this.grow(d.length);
    this.buf.set(d, this.len);
    this.len += d.length;
  }
  out(): Uint8Array {
    return this.buf.subarray(0, this.len);
  }
}

type ZipInput = { name: string; data: Uint8Array; method?: number; csizeOverride?: number };

/** 手写一个 ZIP（本地头 + 中央目录 + EOCD），CRC 真算，和上游的包同构。 */
function buildZip(entries: ZipInput[]): Uint8Array {
  const locals: Uint8Array[] = [];
  const central: (ZipInput & { offset: number })[] = [];
  let offset = 0;
  for (const e of entries) {
    const crc = crc32(e.data);
    const csize = e.method === 8 ? e.data.length : e.data.length;
    const w = new Writer();
    const name = new TextEncoder().encode(e.name);
    w.u32(0x04034b50);
    w.u16(20);
    w.u16(0);
    w.u16(e.method ?? 0);
    w.u16(0);
    w.u16(0);
    w.u32(crc);
    w.u32(csize);
    w.u32(e.data.length);
    w.u16(name.length);
    w.u16(0);
    w.bytes(name);
    w.bytes(e.data);
    const local = w.out();
    locals.push(local);
    central.push({ ...e, offset });
    offset += local.length;
  }
  const w = new Writer();
  const cdStart = offset;
  for (const e of central) {
    const name = new TextEncoder().encode(e.name);
    w.u32(0x02014b50);
    w.u16(20);
    w.u16(20);
    w.u16(0);
    w.u16(e.method ?? 0);
    w.u16(0);
    w.u16(0);
    w.u32(crc32(e.data));
    w.u32(e.csizeOverride ?? e.data.length);
    w.u32(e.data.length);
    w.u16(name.length);
    w.u16(0);
    w.u16(0);
    w.u16(0);
    w.u16(0);
    w.u32(0);
    w.u32(e.offset);
    w.bytes(name);
  }
  const cd = w.out();
  const end = new Writer();
  end.u32(0x06054b50);
  end.u16(0);
  end.u16(0);
  end.u16(central.length);
  end.u16(central.length);
  end.u32(cd.length);
  end.u32(cdStart);
  end.u16(0);
  const parts = [...locals, cd, end.out()];
  const total = parts.reduce((n, p) => n + p.length, 0);
  const out = new Uint8Array(total);
  let at = 0;
  for (const p of parts) {
    out.set(p, at);
    at += p.length;
  }
  return out;
}

const deflateRaw = async (data: Uint8Array): Promise<Uint8Array> => {
  const stream = new Blob([data as BlobPart]).stream().pipeThrough(new CompressionStream("deflate-raw"));
  return new Uint8Array(await new Response(stream).arrayBuffer());
};

describe("sniffImage / isZip", () => {
  it("按文件头认 PNG / JPEG / WebP，不认 JSON", () => {
    expect(sniffImage(PNG_1X1)).toBe<ImageMime>("image/png");
    expect(sniffImage(JPEG_1X1)).toBe<ImageMime>("image/jpeg");
    expect(sniffImage(WEBP_1X1)).toBe<ImageMime>("image/webp");
    expect(sniffImage(new TextEncoder().encode('{"error":{}}'))).toBeNull();
    expect(sniffImage(new Uint8Array(8))).toBeNull();
  });
  it("ZIP 签名认 PK\\x03\\x04", () => {
    expect(isZip(PNG_1X1)).toBe(false);
    expect(isZip(buildZip([{ name: "a.png", data: PNG_1X1 }]))).toBe(true);
  });
});

describe("extractImages", () => {
  it("直接给 PNG：原样返回一张", async () => {
    const imgs = await extractImages(PNG_1X1);
    expect(imgs).toHaveLength(1);
    expect(imgs[0]?.mime).toBe("image/png");
  });

  it("ZIP（stored）：只留图片条目", async () => {
    const zip = buildZip([
      { name: "readme.txt", data: new TextEncoder().encode("not an image") },
      { name: "image_0.png", data: PNG_1X1 },
    ]);
    const imgs = await extractImages(zip);
    expect(imgs).toHaveLength(1);
    expect(imgs[0]?.mime).toBe("image/png");
  });

  (typeof CompressionStream === "undefined" ? it.skip : it)(
    "ZIP（deflate）：用 DecompressionStream 解开",
    async () => {
      const zip = buildZip([{ name: "image_0.png", data: await deflateRaw(PNG_1X1), method: 8 }]);
      const imgs = await extractImages(zip);
      expect(imgs).toHaveLength(1);
      expect(imgs[0]?.bytes).toEqual(PNG_1X1);
    },
  );

  it("ZIP 里没有图片 / 响应不是图片：抛给成员能看懂的错", async () => {
    const zip = buildZip([{ name: "note.txt", data: new TextEncoder().encode("hi") }]);
    await expect(extractImages(zip)).rejects.toThrow("没有认出图片");
    await expect(extractImages(new TextEncoder().encode('{"detail":"nope"}'))).rejects.toThrow("不是图片");
  });

  it("中央目录里写着超大条目：跳过，不当真", async () => {
    const zip = buildZip([{ name: "bomb.png", data: PNG_1X1, csizeOverride: 64 * 1024 * 1024 }]);
    await expect(extractImages(zip)).rejects.toThrow("没有认出图片");
  });
});

describe("toDataUrl", () => {
  it("data:image/png;base64 且内容能解回来", () => {
    const url = toDataUrl({ bytes: PNG_1X1, mime: "image/png" });
    expect(url.startsWith("data:image/png;base64,")).toBe(true);
    expect(atob(url.slice("data:image/png;base64,".length))).toBe(
      Array.from(PNG_1X1, (b) => String.fromCharCode(b)).join(""),
    );
  });
});
