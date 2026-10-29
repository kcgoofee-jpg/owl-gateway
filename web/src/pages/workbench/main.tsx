import { render } from "preact";
import { Toaster } from "../../ui";
import { NaiApp } from "./nai/NaiApp";

render(
  <>
    <NaiApp />
    <Toaster />
  </>,
  document.getElementById("app") as HTMLElement,
);
