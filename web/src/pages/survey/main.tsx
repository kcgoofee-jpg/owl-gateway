import { render } from "preact";
import { installErrorReporter } from "../../lib/api";
import { Toaster } from "../../ui";
import { Survey } from "./Survey";

installErrorReporter("survey");
render(
  <>
    <Survey />
    <Toaster />
  </>,
  document.getElementById("app") as HTMLElement,
);
