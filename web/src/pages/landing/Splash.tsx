/**
 * 开屏：深色夜空上两只琥珀色眼睛，加载时眨两下，然后睁开；下面是从模糊里显影的「Drawing / fantasy」。
 * 睁眼后眼睛起飞：游客飞回顶栏 Logo 的眼睛里；有 Key 的成员变成 V4.5、V5 两个额度圆环。起飞时文字淡出。
 *
 * - 不显示任何提示文字，点任意处（或按任意键）跳过；
 * - 每天只播一次（localStorage，读写都包 try/catch；读不到就照播）；
 * - 系统开了「减少动态效果」时不播；
 * - 落点不在屏幕内时（比如圆环被挤到下面），直接淡出。
 */
import { signal } from "@preact/signals";
import { useEffect, useRef } from "preact/hooks";
import { animate, EASE_OUT, reduceMotion } from "../../lib/motion";
import "./splash.css";

const KEY = "owl-splash";
/** 本地日期 YYYY-MM-DD */
const today = () => new Date().toLocaleDateString("sv-SE");

/** 今天要不要播（页面加载时只判断一次）。 */
export function splashDue(): boolean {
  if (reduceMotion()) return false;
  try {
    return localStorage.getItem(KEY) !== today();
  } catch {
    return true;
  }
}

/**
 * 开屏还没结束：圆环等它落地后再画弧、数数。
 * 初值在页面加载时就按「今天要不要播」定好（和 Landing 首帧的判断同一时刻、同一结果）：
 * 如果等开屏组件挂载后的副作用再置 true，圆环首帧已经以为没有开屏，标签的入场动画会提前播完。
 */
export const splashActive = signal(splashDue());

function markShown() {
  try {
    localStorage.setItem(KEY, today());
  } catch {
    /* 隐私模式：下次再播一遍 */
  }
}

/** 眼睛的落点。rings = 落成额度圆环（带描边颜色），否则是 Logo 的两只眼睛。 */
export type Landing = { targets: HTMLElement[]; rings: boolean };

export function Splash(props: {
  /** 页面数据就绪后给出落点（眼睛眨完以后才调用） */
  land: () => Promise<Landing>;
  onDone: () => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const skipRef = useRef<() => void>(() => {});

  // 只在挂载时播一遍，之后 props 变化不重播
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    markShown();
    splashActive.value = true;
    const root = document.documentElement;
    root.dataset.splash = "pending";
    // 开屏落在页面顶部的元素上：刷新时不要恢复到上次的滚动位置
    if ("scrollRestoration" in history) history.scrollRestoration = "manual";
    window.scrollTo(0, 0);

    const bg = el.querySelector<HTMLElement>(".splash__bg");
    const title = el.querySelector<HTMLElement>(".splash__title");
    const eyes = [...el.querySelectorAll<HTMLElement>(".splash__eye")];
    const skipped = new Promise<"skip">((res) => {
      skipRef.current = () => res("skip");
    });
    const onKey = () => skipRef.current();
    document.addEventListener("keydown", onKey);
    const race = (p: Promise<unknown>) => Promise.race([p.then(() => "done" as const), skipped]);

    let alive = true;
    (async () => {
      try {
        if (title)
          animate(
            title,
            { opacity: [0, 1], filter: ["blur(12px)", "blur(0px)"] },
            { duration: 0.9, ease: EASE_OUT },
          );
        // 加载中：眨两下
        const blink = animate(
          eyes,
          { scaleY: [1, 0.08, 1] },
          {
            duration: 0.3,
            delay: 0.35,
            repeat: 1,
            repeatDelay: 0.3,
            ease: "easeInOut",
          },
        );
        if ((await race(blink.finished)) === "skip") return;
        // 等页面知道自己是游客还是成员（最多 3 秒），落点才确定
        const timeout = new Promise<Landing>((res) =>
          setTimeout(() => res({ targets: [], rings: false }), 3000),
        );
        const landing = await Promise.race([props.land(), timeout, skipped]);
        if (landing === "skip") return;
        root.dataset.splash = landing.rings ? "rings" : "logo";
        // 睁眼
        if (
          (await race(
            animate(eyes, { scale: [1, 1.25, 1] }, { duration: 0.35, ease: "easeOut" }).finished,
          )) === "skip"
        )
          return;
        el.classList.add("splash--flying");
        const rects = landing.targets.map((t) => t.getBoundingClientRect());
        if (rects.length < 2 || rects.some((b) => b.bottom < 0 || b.top > innerHeight)) {
          await animate(el, { opacity: [1, 0] }, { duration: 0.4 }).finished;
          return;
        }
        const from = eyes.map((e) => e.getBoundingClientRect());
        eyes.forEach((e, i) => {
          const f = from[i] as DOMRect;
          Object.assign(e.style, {
            position: "fixed",
            margin: "0",
            left: `${f.left}px`,
            top: `${f.top}px`,
            width: `${f.width}px`,
            height: `${f.height}px`,
          });
        });
        const flights = eyes.map((e, i) => {
          const r = rects[i] as DOMRect;
          const target = landing.targets[i] as HTMLElement;
          // 圆环：画出来的圈外沿在 104 单位视框里缩进 3；Logo 的眼睛：正好落上去
          const inset = landing.rings ? (r.width * 3) / 104 : 0;
          const to: Record<string, string> = {
            left: `${r.left + inset}px`,
            top: `${r.top + inset}px`,
            width: `${r.width - inset * 2}px`,
            height: `${r.height - inset * 2}px`,
            boxShadow: "0 0 0px rgba(0,0,0,0)",
          };
          if (landing.rings) {
            const color = getComputedStyle(target).getPropertyValue("--ring-c").trim();
            Object.assign(to, {
              borderWidth: `${(r.width * 6) / 104}px`,
              borderColor: color,
              backgroundColor: "rgba(0,0,0,0)",
            });
          }
          return animate(e, to, {
            duration: landing.rings ? 0.75 : 0.65,
            delay: i * 0.06,
            ease: [0.65, 0, 0.35, 1],
          }).finished;
        });
        if (bg) animate(bg, { opacity: 0 }, { duration: 0.6, delay: 0.1 });
        if (title) animate(title, { opacity: 0, y: -10 }, { duration: 0.3 });
        await race(Promise.all(flights));
      } finally {
        if (alive) {
          document.removeEventListener("keydown", onKey);
          delete root.dataset.splash;
          splashActive.value = false;
          props.onDone();
        }
      }
    })();
    return () => {
      alive = false;
      document.removeEventListener("keydown", onKey);
      delete root.dataset.splash;
      splashActive.value = false;
    };
  }, []);

  return (
    // 纯装饰的开屏：点任意处跳过（键盘任意键在 document 上监听）
    <div ref={ref} class="splash" aria-hidden="true" onClick={() => skipRef.current()}>
      <div class="splash__bg" />
      <div class="splash__stack">
        <div class="splash__eyes">
          <i class="splash__eye" />
          <i class="splash__eye" />
        </div>
        <p class="splash__title" lang="en">
          Drawing
          <br />
          fantasy
        </p>
      </div>
    </div>
  );
}
