import { render } from "preact";
import { Toaster } from "../../ui";
import { Landing } from "./Landing";

render(
  <>
    <Landing />
    <Toaster />
  </>,
  document.getElementById("app") as HTMLElement,
);
