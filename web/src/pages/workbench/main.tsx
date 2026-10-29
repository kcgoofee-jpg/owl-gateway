import { render } from "preact";
import { Toaster } from "../../ui";
import { WorkbenchApp } from "./App";
import { NaiApp } from "./nai/NaiApp";

/** 默认进 NovelAI 风格的复刻页；?legacy=1 回旧版工作台。 */
const legacy = new URLSearchParams(window.location.search).has("legacy");

render(
  <>
    {legacy ? <WorkbenchApp /> : <NaiApp />}
    <Toaster />
  </>,
  document.getElementById("app") as HTMLElement,
);
