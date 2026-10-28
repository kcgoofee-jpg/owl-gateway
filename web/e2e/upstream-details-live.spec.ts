import { expect, test } from "@playwright/test";
import { readCreds } from "./devEnv";

test("完整信息经过真实网关和假上游，不依赖浏览器拦截", async ({ page, context, baseURL }) => {
  const creds = readCreds();
  test.skip(!creds.admin, "需要合成开发环境");
  await context.addCookies([
    { name: "nai_gate_admin", value: creds.admin ?? "", url: baseURL ?? "http://localhost:5174" },
  ]);
  await page.goto("/admin/upstream");
  const response = page.waitForResponse((r) =>
    /\/admin\/api\/upstream\/[^/]+\/details$/.test(new URL(r.url()).pathname),
  );
  await page.getByRole("button", { name: "完整信息", exact: true }).first().click();
  const details = await response;
  expect(details.status(), await details.text()).toBe(200);
  expect(details.headers()["cache-control"]).toBe("no-store");
  const value = await details.json();
  expect(value.information.data.plainTextEmail).toBe("dev@example.invalid");
  const dialog = page.getByRole("dialog");
  await expect(dialog.getByText("dev@example.invalid", { exact: true })).toBeVisible();
  await dialog.getByRole("button", { name: "订阅", exact: true }).click();
  await expect(dialog.getByText("订阅等级", { exact: true })).toBeVisible();
  await dialog.getByRole("button", { name: "优先级", exact: true }).click();
  await expect(dialog.getByText("任务优先级", { exact: true })).toBeVisible();
  await dialog.getByRole("button", { name: "关闭", exact: true }).last().click();
  await expect(page.getByText("dev@example.invalid", { exact: true })).toHaveCount(0);
});
