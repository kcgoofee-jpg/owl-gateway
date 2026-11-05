import { readFileSync } from "node:fs";
import { resolve } from "node:path";
/** Release checks use the real local gateway, synthetic credentials and fake upstream. */
import { expect, test } from "@playwright/test";
import { readCreds } from "./devEnv";
import { adminSession, asMember } from "./helpers";

test.beforeAll(() => {
  const creds = readCreds();
  expect(creds.member, "Start cargo xtask dev and set E2E_COOKIES").toBeTruthy();
  expect(creds.admin_password).toBeTruthy();
});

test("guest landing renders and member APIs require credentials", async ({ page, request }) => {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto("/");
  await expect(page.locator(".lp-status__text")).not.toBeEmpty();
  expect((await request.get("/v1/me")).status()).toBe(401);
  expect((await request.get("/public/me/request-stats")).status()).toBe(401);
  expect(errors).toEqual([]);
});

// 镜像页由网关直接渲染（静态资源在 STATIC_DIR，不进仓库）；CI 里没有镜像时跳过。
const GATE = process.env.E2E_GATE ?? "http://127.0.0.1:18000";

test("member entry lands on the mirrored image page", async ({ page, context }) => {
  await asMember(context, GATE);
  const mirror = await context.request.get(`${GATE}/image`, { maxRedirects: 0 });
  test.skip(
    mirror.status() === 503 && (await mirror.text()).includes("前端未构建"),
    "镜像资源未就绪（需要 STATIC_DIR 指向 mirror/）",
  );
  expect(mirror.status()).toBe(200);
  await page.goto(`${GATE}/workbench`);
  await expect(page).toHaveURL(/\/image$/);
  // 官网镜像页的最小信号：模型行 + 生成按钮
  await expect(page.getByText("Model", { exact: true })).toBeVisible({ timeout: 20_000 });
  await expect(page.locator("button", { hasText: /Generate \d+ Image/ }).first()).toBeVisible();
});

test("admin login loads member management", async ({ page, context, browser, baseURL }) => {
  const cookies = await adminSession(browser, baseURL, readCreds().admin_password ?? "");
  await context.addCookies(cookies);
  await page.goto("/admin/members");
  await expect(page.getByRole("heading", { level: 1, name: "成员" })).toBeVisible();
  await expect(page.getByRole("button", { name: /展开.*的详情/ }).first()).toBeVisible();
});

test("retired status routes are absent and health remains available", async ({ request }) => {
  for (const path of ["/status", "/status.json", "/_owl/status-page.css", "/_owl/status-page.js"]) {
    expect((await request.get(path)).status(), path).toBe(404);
  }
  for (const path of ["/healthz", "/readyz", "/livez"]) {
    const response = await request.get(path);
    expect(response.status(), path).toBe(200);
    expect((await response.json()).ok).toBe(true);
  }
});

test("member statistics match the full service day on desktop and mobile", async ({ page, context }) => {
  await asMember(context);
  const response = await context.request.get("/public/me/request-stats?before=1&status=error");
  expect(response.status()).toBe(200);
  const data = await response.json();
  await page.goto("/");
  await page.getByRole("tab", { name: "统计", exact: true }).click();
  const panel = page.getByRole("tabpanel", { name: "统计", exact: true });
  await expect(panel.locator(".lp-metric").first()).toHaveText(
    `请求总数${data.summary.total.toLocaleString("zh-CN")}次`,
  );
  await expect(panel.getByText("请求结果", { exact: true })).toBeVisible();
  await page.setViewportSize({ width: 390, height: 844 });
  await expect(panel.getByText("模型与工具", { exact: true })).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await page.getByRole("tab", { name: /^全部/ }).click();
  await expect(page.locator(".lp-req").first()).toBeVisible();
  await page.getByRole("tab", { name: "统计", exact: true }).click();
  await expect(panel.locator(".lp-metric").first()).toHaveText(
    `请求总数${data.summary.total.toLocaleString("zh-CN")}次`,
  );
});

test("Discord sharing confirmation keeps or removes file and hidden metadata", async ({ page }) => {
  const bridge = readFileSync(
    resolve(import.meta.dirname, "../../deploy/nai-mirror-overrides/bridge.js"),
    "utf8",
  );
  await page.route("https://share-fixture.test/**", (route) =>
    route.fulfill({
      contentType: "text/html",
      body: '<!doctype html><meta charset="utf-8"><body><div class="ProseMirror" style="width:300px">private prompt</div><img id="fixture-image" width="256" height="256"><a href="/explore/register">Upload to Explore</a></body>',
    }),
  );
  await page.goto("https://share-fixture.test/");
  await page.evaluate(() => {
    const w = window as typeof window & { __uploads: FormData[] };
    w.__uploads = [];
    window.requestIdleCallback = () => 0;
    window.fetch = async (_input, init) => {
      w.__uploads.push(init?.body as FormData);
      return new Response("{}", { status: 200, headers: { "content-type": "application/json" } });
    };
  });
  await page.evaluate(bridge);
  // Build valid images, with hidden alpha bits and container metadata carrying a secret marker.
  for (const format of ["png", "webp", "jpeg"]) {
    await page.evaluate(async (format) => {
      const w = window as typeof window & { __original: Blob; __uploads: FormData[] };
      const canvas = document.createElement("canvas");
      canvas.width = canvas.height = 32;
      const ctx = canvas.getContext("2d");
      if (!ctx) throw new Error("no canvas");
      const pixels = ctx.createImageData(32, 32);
      const magic = new TextEncoder().encode("stealth_pngcomp");
      for (let i = 0; i < 1024; i++) {
        pixels.data.set(
          [101, 151, 201, 254 | (((magic[Math.floor(i / 8) % magic.length] ?? 0) >> (7 - (i % 8))) & 1)],
          i * 4,
        );
      }
      ctx.putImageData(pixels, 0, 0);
      const blob = await new Promise<Blob>((resolve) =>
        canvas.toBlob(
          (blob) => {
            if (!blob) throw new Error("encode failed");
            resolve(blob);
          },
          `image/${format}`,
          1,
        ),
      );
      const bytes = new Uint8Array(await blob.arrayBuffer());
      const secret = new TextEncoder().encode('Comment\0{"prompt":"PRIVATE_GENERATION_PARAMETERS"}');
      let modified: Uint8Array<ArrayBuffer>;
      if (format === "png") {
        const chunk = new Uint8Array(12 + secret.length);
        new DataView(chunk.buffer).setUint32(0, secret.length);
        chunk.set(new TextEncoder().encode("tEXt"), 4);
        chunk.set(secret, 8);
        let crc = 0xffffffff;
        for (const byte of chunk.subarray(4, chunk.length - 4)) {
          crc ^= byte;
          for (let j = 0; j < 8; j++) crc = (crc >>> 1) ^ (crc & 1 ? 0xedb88320 : 0);
        }
        new DataView(chunk.buffer).setUint32(chunk.length - 4, (crc ^ 0xffffffff) >>> 0);
        modified = new Uint8Array(bytes.length + chunk.length);
        modified.set(bytes.subarray(0, 33));
        modified.set(chunk, 33);
        modified.set(bytes.subarray(33), 33 + chunk.length);
      } else if (format === "jpeg") {
        const comment = new Uint8Array(secret.length + 4);
        comment.set([255, 254]);
        new DataView(comment.buffer).setUint16(2, secret.length + 2);
        comment.set(secret, 4);
        modified = new Uint8Array(bytes.length + comment.length);
        modified.set(bytes.subarray(0, 2));
        modified.set(comment, 2);
        modified.set(bytes.subarray(2), 2 + comment.length);
      } else {
        const chunk = new Uint8Array(8 + secret.length + (secret.length % 2));
        chunk.set(new TextEncoder().encode("EXIF"));
        new DataView(chunk.buffer).setUint32(4, secret.length, true);
        chunk.set(secret, 8);
        modified = new Uint8Array(bytes.length + chunk.length);
        modified.set(bytes);
        modified.set(chunk, bytes.length);
        new DataView(modified.buffer).setUint32(4, modified.length - 8, true);
      }
      w.__original = new Blob([modified], { type: `image/${format}` });
      const image = document.querySelector<HTMLImageElement>("#fixture-image");
      if (!image) throw new Error("no image");
      image.src = URL.createObjectURL(w.__original);
      await image.decode();
    }, format);
    const upload = page.locator('a[href="/explore/register"]');
    await upload.click();
    const dialog = page.getByRole("dialog", { name: "确认分享" });
    await expect(
      dialog.getByText("该分享转发至 owl 服务器进行保存备份，请确认是否有潜在二传违规风险！"),
    ).toBeVisible();
    const checkbox = dialog.getByRole("checkbox", { name: "包含生成元数据（提示词、种子等）" });
    await expect(checkbox).not.toBeChecked();
    await dialog.getByRole("button", { name: "取消", exact: true }).click();
    await expect(upload).not.toHaveAttribute("data-owl-busy", "1");
    expect(await page.evaluate(() => (window as unknown as { __uploads: FormData[] }).__uploads.length)).toBe(
      0,
    );
    await upload.click();
    await page.keyboard.press("Escape");
    await expect(dialog).toHaveCount(0);
    await upload.click();
    await dialog.getByRole("button", { name: "确认转发", exact: true }).click();
    await expect(upload).not.toHaveAttribute("data-owl-busy", "1");
    const clean = await page.evaluate(async () => {
      const w = window as typeof window & { __uploads: FormData[] };
      const form = w.__uploads.pop();
      if (!form) throw new Error("no upload");
      const image = form.get("image") as File;
      const pixels = await createImageBitmap(image);
      const canvas = document.createElement("canvas");
      canvas.width = pixels.width;
      canvas.height = pixels.height;
      const ctx = canvas.getContext("2d");
      if (!ctx) throw new Error("no context");
      ctx.drawImage(pixels, 0, 0);
      const data = ctx.getImageData(0, 0, pixels.width, pixels.height).data;
      return {
        type: image.type,
        width: pixels.width,
        height: pixels.height,
        fields: [...form.keys()],
        hasSecret: new TextDecoder()
          .decode(await image.arrayBuffer())
          .includes("PRIVATE_GENERATION_PARAMETERS"),
        opaque: data.every((v, i) => i % 4 !== 3 || v === 255),
      };
    });
    expect(clean).toEqual({
      type: "image/png",
      width: 32,
      height: 32,
      fields: ["image"],
      hasSecret: false,
      opaque: true,
    });
    await upload.click();
    await checkbox.check();
    await dialog.getByRole("button", { name: "确认转发", exact: true }).click();
    await expect(upload).not.toHaveAttribute("data-owl-busy", "1");
    expect(
      await page.evaluate(async () => {
        const w = window as typeof window & { __uploads: FormData[]; __original: Blob };
        const form = w.__uploads.pop();
        if (!form) throw new Error("no upload");
        const image = form.get("image") as File;
        const original = new Uint8Array(await w.__original.arrayBuffer());
        const sent = new Uint8Array(await image.arrayBuffer());
        return {
          sameBytes: original.length === sent.length && original.every((v, i) => v === sent[i]),
          prompt: form.get("prompt"),
        };
      }),
    ).toEqual({ sameBytes: true, prompt: "private prompt" });
  }
  // A failed sanitizer must not submit the original image instead.
  await page.evaluate(() => {
    HTMLCanvasElement.prototype.toBlob = (callback) => {
      callback(null);
    };
  });
  await page.locator('a[href="/explore/register"]').click();
  await page.getByRole("button", { name: "确认转发", exact: true }).click();
  await expect(page.locator('a[href="/explore/register"]')).not.toHaveAttribute("data-owl-busy", "1");
  expect(await page.evaluate(() => (window as unknown as { __uploads: FormData[] }).__uploads.length)).toBe(
    0,
  );
});
