/** 读一张本地图成 data URL + 不带前缀的 base64。工作台各页共用。 */
import { MAX_FILE_BYTES, type SourceImage } from "./workbench-config";

export function readImageFile(file: File): Promise<SourceImage> {
  return new Promise((resolve, reject) => {
    if (file.size > MAX_FILE_BYTES) {
      reject(new Error(`「${file.name}」超过 12 MiB，先压缩再传。`));
      return;
    }
    const reader = new FileReader();
    reader.onerror = () => reject(new Error("这张图读不出来。"));
    reader.onload = () => {
      const url = String(reader.result ?? "");
      const comma = url.indexOf(",");
      if (!url.startsWith("data:") || comma < 0) {
        reject(new Error("这张图读不出来。"));
        return;
      }
      resolve({ name: file.name, dataUrl: url, b64: url.slice(comma + 1) });
    };
    reader.readAsDataURL(file);
  });
}
