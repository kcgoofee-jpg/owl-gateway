import { render } from "preact";
import { installErrorReporter } from "../../lib/api";
import { Toaster } from "../../ui";
import { Admin } from "./Admin";

installErrorReporter("admin");
render(
  <>
    <Admin />
    <Toaster />
  </>,
  document.getElementById("app") as HTMLElement,
);
