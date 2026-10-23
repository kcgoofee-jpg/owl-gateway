import { useEffect, useRef, useState } from "preact/hooks";
import { copyText } from "../../ui";
import data from "./gallery-data.json";

function Example({ id, prompt, index }: { id: string; prompt: string; index: number }) {
  const [failed, setFailed] = useState(false);
  const [feedback, setFeedback] = useState("");
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  useEffect(() => () => clearTimeout(timer.current), []);
  const src = `https://static.novelai.net/quickstart/v5/${id}.webp`;
  async function copy() {
    let ok = false;
    try {
      ok = await copyText(prompt);
    } catch {
      /* Report failure instead of claiming success. */
    }
    clearTimeout(timer.current);
    setFeedback(ok ? "已复制提示词" : "复制失败，请重试");
    timer.current = setTimeout(() => setFeedback(""), 2000);
  }
  return (
    <div class="lp-gallery__item">
      {failed ? (
        <a class="lp-gallery__unavailable" href={src} target="_blank" rel="noreferrer">
          图片暂不可用 · 查看原图 ↗
        </a>
      ) : (
        <button
          type="button"
          class="lp-gallery__copy"
          aria-label={`复制示例 ${index + 1} 的提示词`}
          title="点击复制提示词"
          onClick={copy}
        >
          <img
            src={src}
            alt={`NovelAI 官方示例作品 ${index + 1}`}
            width={832}
            height={1216}
            loading="lazy"
            decoding="async"
            referrerPolicy="no-referrer"
            onError={() => setFailed(true)}
          />
          <span class={`lp-gallery__feedback${feedback ? " is-visible" : ""}`} aria-hidden="true">
            {feedback}
          </span>
        </button>
      )}
      <span class="sr-only" role="status" aria-live="polite">
        {feedback}
      </span>
    </div>
  );
}

export function Gallery() {
  return (
    <section id="gallery" class="lp-gallery" aria-label="图片瀑布流" tabIndex={-1}>
      {/* 悬停 title 在触屏上不存在：常驻一行说明，让手机访客也知道图片可点 */}
      <p class="lp-gallery__hint">点击图片复制提示词</p>
      <div class="lp-gallery__images">
        {data.images.map((image, i) => (
          <Example key={image.id} {...image} index={i} />
        ))}
      </div>
    </section>
  );
}
