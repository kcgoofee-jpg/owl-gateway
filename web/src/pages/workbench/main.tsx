import { render } from "preact";
import { lazy, Suspense } from "preact/compat";
import { Toaster } from "../../ui";
import { NaiApp } from "./nai/NaiApp";

/** 旧版工作台懒加载：两套皮肤的类名都叫 nai-*，静态同载会互相污染样式。 */
const LegacyApp = lazy(async () => {
  const m = await import("./App");
  return { default: m.WorkbenchApp };
});

/** 默认进 NovelAI 风格的复刻页；?legacy=1 回旧版工作台。 */
const legacy = new URLSearchParams(window.location.search).has("legacy");

render(
  <>
    {legacy ? (
      <Suspense fallback={null}>
        <LegacyApp />
      </Suspense>
    ) : (
      <NaiApp />
    )}
    <Toaster />
  </>,
  document.getElementById("app") as HTMLElement,
);
