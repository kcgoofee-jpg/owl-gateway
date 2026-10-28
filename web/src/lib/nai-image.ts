/**
 * 网页出图收图：上游（经网关原样转发）回的是 NAI 格式——单张 PNG 直接给字节，多张或 V5 给 ZIP。
 * 这里只认文件头，不信 content-type；ZIP 用浏览器自带的 DecompressionStream 解包。
 * 限制沿用上游口径：单张 ≤ 32 MiB、单边 ≤ 4096 像素由上游保证，这里只负责解包与识别。
 */

export type ImageMime = "image/png" | "image/jpeg" | "image/webp";
export type ImageBytes = { bytes: Uint8Array; mime: ImageMime };

/** 按文件头识别图片格式；认不出的返回 null（HTML 错误页、JSON 都会落到这里）。 */
export function sniffImage(b: Uint8Array): ImageMime | null {
  if (b.length < 12) return null;
  const at = (i: number): number => b[i] ?? 0;
  if (at(0) === 0x89 && at(1) === 0x50 && at(2) === 0x4e && at(3) === 0x47) return "image/png";
  if (at(0) === 0xff && at(1) === 0xd8 && at(2) === 0xff) return "image/jpeg";
  if (
    at(0) === 0x52 &&
    at(1) === 0x49 &&
    at(2) === 0x46 &&
    at(3) === 0x46 &&
    at(8) === 0x57 &&
    at(9) === 0x45
  ) {
    return "image/webp";
  }
  return null;
}

/** ZIP 的本地文件头签名 PK\x03\x04（稀里糊涂把 ZIP 当 PNG 塞进 <img> 的事不能发生）。 */
export function isZip(b: Uint8Array): boolean {
  const at = (i: number): number => b[i] ?? 0;
  return (
    b.length >= 4 && at(0) === 0x50 && at(1) === 0x4b && (at(2) === 0x03 || at(2) === 0x05 || at(2) === 0x07)
  );
}

const u16 = (b: Uint8Array, o: number): number => (b[o] ?? 0) | ((b[o + 1] ?? 0) << 8);
const u32 = (b: Uint8Array, o: number): number =>
  ((b[o] ?? 0) | ((b[o + 1] ?? 0) << 8) | ((b[o + 2] ?? 0) << 16) | ((b[o + 3] ?? 0) << 24)) >>> 0;

/**
 * 出图响应 → 图片列表：直接图片返回一张；ZIP 解包后按包内顺序返回图片条目。
 * 解不出图片（错误 JSON、损坏数据、动态 WebP）抛错，文案给成员看。
 */
export async function extractImages(data: Uint8Array): Promise<ImageBytes[]> {
  if (isZip(data)) return unzipImages(data);
  const mime = sniffImage(data);
  if (mime) return [{ bytes: data, mime }];
  throw new Error("这次返回的不是图片，可能是上游出了问题，稍后再试一次。");
}

type ZipEntry = { name: string; method: number; size: number; offset: number };

/** 只解析中央目录（不扫数据区）：NAI 的包不大，这样也天然跳过数据描述符的坑。 */
function zipEntries(b: Uint8Array): ZipEntry[] {
  // 从尾部找 EOCD（PK\x05\x06），注释最长 65535
  for (let i = b.length - 22; i >= Math.max(0, b.length - 22 - 65535); i--) {
    if (u32(b, i) !== 0x06054b50) continue;
    const count = u16(b, i + 10);
    let off = u32(b, i + 16);
    const entries: ZipEntry[] = [];
    for (let n = 0; n < count && off + 46 <= b.length; n++) {
      if (u32(b, off) !== 0x02014b50) break;
      const nameLen = u16(b, off + 28);
      const extraLen = u16(b, off + 30);
      const commentLen = u16(b, off + 32);
      const name = new TextDecoder().decode(b.slice(off + 46, off + 46 + nameLen));
      entries.push({
        name,
        method: u16(b, off + 10),
        // 切数据用压缩后大小（off+20）；解出来的大小在 inflate 之后另查
        size: u32(b, off + 20),
        offset: u32(b, off + 42),
      });
      off += 46 + nameLen + extraLen + commentLen;
    }
    return entries;
  }
  throw new Error("收到的压缩包不完整，没有解开来。");
}

async function inflateRaw(data: Uint8Array): Promise<Uint8Array> {
  if (typeof DecompressionStream === "undefined") {
    throw new Error("这个浏览器解不了图片压缩包，请换新版 Chrome / Edge / Firefox / Safari。");
  }
  const stream = new Blob([data as BlobPart]).stream().pipeThrough(new DecompressionStream("deflate-raw"));
  return new Uint8Array(await new Response(stream).arrayBuffer());
}

async function unzipImages(b: Uint8Array): Promise<ImageBytes[]> {
  const out: ImageBytes[] = [];
  for (const e of zipEntries(b)) {
    if (e.size <= 0 || e.size > 32 * 1024 * 1024) continue;
    // 本地头的名字 / 扩展长度可能与中央目录不同，各自按本地头跳
    const localNameLen = u16(b, e.offset + 26);
    const localExtraLen = u16(b, e.offset + 28);
    const start = e.offset + 30 + localNameLen + localExtraLen;
    const raw = b.slice(start, start + e.size);
    let bytes: Uint8Array;
    if (e.method === 0) bytes = raw;
    else if (e.method === 8) bytes = await inflateRaw(raw);
    else continue;
    // 解压炸弹兜底：上游单张上限 32 MiB，解出来大得离谱的直接跳过
    if (bytes.length > 48 * 1024 * 1024) continue;
    const mime = sniffImage(bytes);
    if (mime) out.push({ bytes, mime });
  }
  if (!out.length) throw new Error("压缩包里没有认出图片，稍后再试一次。");
  return out;
}

/** data URL：页面 CSP 的 img-src 允许 data:（blob: 不在名单里，别用）。 */
export function toDataUrl(img: ImageBytes): string {
  let s = "";
  const chunk = 0x8000;
  for (let i = 0; i < img.bytes.length; i += chunk) {
    s += String.fromCharCode(...img.bytes.subarray(i, i + chunk));
  }
  return `data:${img.mime};base64,${btoa(s)}`;
}

export const imageExt = (mime: ImageMime): string =>
  mime === "image/png" ? "png" : mime === "image/jpeg" ? "jpg" : "webp";
