import { render } from "preact";
import { act } from "preact/test-utils";
import { expect, it } from "vitest";
import { type Form, useForm } from "./forms";

it("keeps a saved value in the next full submission before refetch", () => {
  const server = { first: 1, second: 2 };
  const specs = {
    first: { kind: "int" as const, label: "first", min: 0, max: 100 },
    second: { kind: "int" as const, label: "second", min: 0, max: 100 },
  };
  let form!: Form;
  function Probe() {
    form = useForm(specs, server);
    return null;
  }
  const root = document.createElement("div");
  document.body.append(root);
  act(() => render(<Probe />, root));
  act(() => form.set("first", "10"));
  expect(form.values()).toEqual({ first: 10, second: 2 });
  act(() => form.commit());
  act(() => form.set("second", "20"));
  expect(form.changes()).toEqual({ second: 20 });
  expect(form.values()).toEqual({ first: 10, second: 20 });
  act(() => render(null, root));
  root.remove();
});

it("does not discard edits made while an earlier save is pending", () => {
  const server = { first: 1, second: 2 };
  const specs = {
    first: { kind: "int" as const, label: "first", min: 0, max: 100 },
    second: { kind: "int" as const, label: "second", min: 0, max: 100 },
  };
  let form!: Form;
  function Probe() {
    form = useForm(specs, server);
    return null;
  }
  const root = document.createElement("div");
  document.body.append(root);
  act(() => render(<Probe />, root));
  act(() => form.set("first", "10"));
  const submitted = form;
  act(() => {
    form.set("first", "11");
    form.set("second", "20");
  });
  act(() => submitted.commit());
  expect(form.changes()).toEqual({ first: 11, second: 20 });
  expect(form.values()).toEqual({ first: 11, second: 20 });
  act(() => render(null, root));
  root.remove();
});
