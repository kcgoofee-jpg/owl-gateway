import { render } from "preact";
import { Toaster } from "../../ui";
import { Patron } from "./Patron";

render(
  <>
    <Patron />
    <Toaster />
  </>,
  document.getElementById("app") as HTMLElement,
);
