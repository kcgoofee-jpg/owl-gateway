import { render } from "preact";
import { Toaster } from "../../ui";
import { Admin } from "./Admin";

render(
  <>
    <Admin />
    <Toaster />
  </>,
  document.getElementById("app") as HTMLElement,
);
