import { render } from "preact";
import { installErrorReporter } from "../../lib/api";
import { Toaster } from "../../ui";
import { Landing } from "./Landing";

installErrorReporter("landing");
render(
  <>
    <Landing />
    <Toaster />
  </>,
  document.getElementById("app") as HTMLElement,
);
