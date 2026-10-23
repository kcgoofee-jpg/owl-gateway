/**
 * 首屏背后的一层颗粒（canvas），随全站排队人数呼吸：人越多，颗粒刷新越快、起伏越明显。
 * 空闲时几乎不动（半秒一帧，6 秒一次呼吸）；排满时每秒 30 帧、约 1.4 秒一次呼吸。
 * 减少动态效果：只画一帧静止的颗粒。页面在后台或首屏滚出视口时不画。
 */
import { useEffect, useRef } from "preact/hooks";
import { reduceMotion } from "../../lib/motion";
import { useStore } from "./ctx";

/** 排队负载 0–1：等待 + 正在画的张数 / 全站排队上限（拿不到上限按 30）。 */
function useLoad(): number {
  const { live } = useStore();
  const d = live.value;
  const q = (d?.queue?.waiting ?? 0) + (d?.queue?.running ?? 0);
  const cap = d?.rules?.queue_cap && d.rules.queue_cap > 0 ? d.rules.queue_cap : 30;
  return Math.max(0, Math.min(1, q / cap));
}

export function Grain() {
  const ref = useRef<HTMLCanvasElement>(null);
  const load = useLoad();
  const loadRef = useRef(load);
  loadRef.current = load;

  useEffect(() => {
    const cv = ref.current;
    const cx = cv?.getContext("2d");
    if (!cv || !cx) return;
    let img: ImageData | null = null;
    let rgb = [33, 32, 28];
    let visible = true;
    let raf = 0;
    let last = 0;

    const draw = () => {
      if (!img) return;
      const d = img.data;
      for (let i = 0; i < d.length; i += 4) {
        d[i] = rgb[0] ?? 0;
        d[i + 1] = rgb[1] ?? 0;
        d[i + 2] = rgb[2] ?? 0;
        d[i + 3] = (Math.random() * 255) | 0;
      }
      cx.putImageData(img, 0, 0);
    };
    const size = () => {
      const r = cv.getBoundingClientRect();
      if (!r.width || !r.height) return;
      // 半分辨率：颗粒更粗，也省一半像素
      cv.width = Math.max(1, Math.round(r.width / 2));
      cv.height = Math.max(1, Math.round(r.height / 2));
      img = cx.createImageData(cv.width, cv.height);
      rgb = getComputedStyle(cv).getPropertyValue("--grain-rgb").split(",").map(Number);
      draw();
    };
    const tick = (now: number) => {
      raf = requestAnimationFrame(tick);
      if (document.hidden || !visible) return;
      const busy = loadRef.current;
      const fps = busy === 0 ? 0.5 : 3 + busy * 27;
      if (now - last > 1000 / fps) {
        draw();
        last = now;
      }
      const period = 6 - busy * 4.6; // 每次呼吸的秒数：空闲 6 秒，排满约 1.4 秒
      const breath = (Math.sin(((now / 1000) * 2 * Math.PI) / period) + 1) / 2;
      cv.style.opacity = String((0.04 + busy * 0.1) * (0.55 + 0.45 * breath));
    };

    size();
    const ro = new ResizeObserver(size);
    ro.observe(cv);
    // 换主题时颗粒颜色跟着换
    const mo = new MutationObserver(size);
    mo.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });
    const mq = matchMedia("(prefers-color-scheme: dark)");
    mq.addEventListener("change", size);
    const io = new IntersectionObserver(([e]) => {
      visible = !!e?.isIntersecting;
    });
    io.observe(cv);
    if (reduceMotion()) cv.style.opacity = "0.05";
    else raf = requestAnimationFrame(tick);
    return () => {
      cancelAnimationFrame(raf);
      ro.disconnect();
      mo.disconnect();
      io.disconnect();
      mq.removeEventListener("change", size);
    };
  }, []);

  return (
    <div class="grain" aria-hidden="true">
      <canvas ref={ref} />
    </div>
  );
}
