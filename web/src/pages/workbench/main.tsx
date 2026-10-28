import { render } from "preact";
import { Toaster } from "../../ui";
import { WorkbenchApp } from "./App";

render(
  <>
    <WorkbenchApp />
    <Toaster />
  </>,
  document.getElementById("app") as HTMLElement,
);
