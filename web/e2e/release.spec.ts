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
    const w = window as typeof window & { __uploads: FormData[]; __saved: unknown[] };
    w.__uploads = [];
    w.__saved = [];
    window.requestIdleCallback = () => 0;
    window.fetch = async (_input, init) => {
      const url = String(_input);
      const method = (init?.method ?? "GET").toUpperCase();
      if (url === "/public/me/share-preference") {
        if (method === "GET")
          return new Response(JSON.stringify({ remembered: null, last: null }), { status: 200 });
        if (method === "PUT") {
          w.__saved.push(JSON.parse(String(init?.body)));
          return new Response(JSON.stringify({ ok: true }), { status: 200 });
        }
        return new Response(JSON.stringify({ ok: true }), { status: 200 });
      }
      if (url === "/public/me/share-destination")
        return new Response(JSON.stringify({ connected: false }), { status: 200 });
      w.__uploads.push(init?.body as FormData);
      return new Response("{}", { status: 200, headers: { "content-type": "application/json" } });
    };
  });
  await page.evaluate(bridge);
  // Build valid images, with hidden alpha bits and container metadata carrying a secret marker.
  for (const format of ["png", "webp", "jpeg"]) {
    await page.evaluate(async (format) => {
      const w = window as typeof window & { __original: Blob; __uploads: FormData[] };
      w.__uploads = [];
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
    const dialog = page.getByRole("dialog");
    await expect(dialog.getByText("会转发到 owl 服务器留存备份，注意二传风险。")).toBeVisible();
    const metadata = dialog.getByRole("checkbox", { name: "附带生成参数（提示词、种子）" });
    const remember = dialog.getByRole("checkbox", { name: "记住这次选择" });
    // 站点默认：包含生成元数据；记住选择默认不勾。
    await expect(metadata).toBeChecked();
    await expect(remember).not.toBeChecked();
    await dialog.getByRole("button", { name: "取消", exact: true }).click();
    await expect(upload).not.toHaveAttribute("data-owl-busy", "1");
    expect(await page.evaluate(() => (window as unknown as { __uploads: FormData[] }).__uploads.length)).toBe(
      0,
    );
    await upload.click();
    // 弹窗是点击后异步打开的：先等它出现，再按 Esc，避免按键跑在弹窗创建之前。
    await expect(dialog.getByRole("button", { name: "取消", exact: true })).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(dialog).toHaveCount(0);
    // 默认（包含元数据）：原图字节原样发出，并把 prompt 等元数据随表单提交。
    await upload.click();
    await dialog.getByRole("button", { name: "分享", exact: true }).click();
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
          type: image.type,
          sameBytes: original.length === sent.length && original.every((v, i) => v === sent[i]),
          fields: [...form.keys()].sort(),
          state: form.get("metadata_state"),
          hasSecret: new TextDecoder()
            .decode(await image.arrayBuffer())
            .includes("PRIVATE_GENERATION_PARAMETERS"),
          prompt: form.get("prompt"),
        };
      }),
    ).toEqual({
      type: `image/${format}`,
      sameBytes: true,
      fields: ["destination", "image", "metadata_state", "model", "prompt", "seed", "size"].sort(),
      state: "kept",
      hasSecret: true,
      prompt: "private prompt",
    });
    // 取消勾选：清理后的 PNG（字节必然重编码），不携带任何生成参数。
    await upload.click();
    await metadata.uncheck();
    await dialog.getByRole("button", { name: "分享", exact: true }).click();
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
        state: form.get("metadata_state"),
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
      fields: ["image", "destination", "metadata_state"],
      state: "stripped",
      hasSecret: false,
      opaque: true,
    });
    // 记住本次选择：确认后把目的地与元数据选择存到服务端（保存是旁路请求，轮询等它落地）。
    // 基数在点击前取：PUT 可能比 busy 清除还早落地。
    const savedBefore = await page.evaluate(
      () => (window as unknown as { __saved: unknown[] }).__saved.length,
    );
    await upload.click();
    await remember.check();
    await dialog.getByRole("button", { name: "分享", exact: true }).click();
    await expect(upload).not.toHaveAttribute("data-owl-busy", "1");
    await expect
      .poll(() => page.evaluate(() => (window as unknown as { __saved: unknown[] }).__saved.length))
      .toBe(savedBefore + 1);
    expect(
      await page.evaluate((i) => (window as unknown as { __saved: unknown[] }).__saved[i], savedBefore),
    ).toEqual({
      destination: "owl",
      include_metadata: true,
      binding_id: null,
    });
  }
  // A failed sanitizer must not submit the original image instead.
  // 默认包含元数据不走清理路径，这里先取消勾选，验证的才是“清理失败宁可不上传”。
  await page.evaluate(() => {
    (window as unknown as { __uploads: FormData[] }).__uploads = [];
    HTMLCanvasElement.prototype.toBlob = (callback) => {
      callback(null);
    };
  });
  await page.locator('a[href="/explore/register"]').click();
  await page.getByRole("checkbox", { name: "附带生成参数（提示词、种子）" }).uncheck();
  await page.getByRole("button", { name: "分享", exact: true }).click();
  await expect(page.locator('a[href="/explore/register"]')).not.toHaveAttribute("data-owl-busy", "1");
  expect(await page.evaluate(() => (window as unknown as { __uploads: FormData[] }).__uploads.length)).toBe(
    0,
  );
});

test("personal channel sharing selects the bound destination and never falls back to public", async ({
  page,
}) => {
  const bridge = readFileSync(
    resolve(import.meta.dirname, "../../deploy/nai-mirror-overrides/bridge.js"),
    "utf8",
  );
  await page.route("https://personal-share-fixture.test/**", (route) =>
    route.fulfill({
      contentType: "text/html",
      body: '<!doctype html><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><img width="256" height="256"><a href="/explore/register">Upload to Explore</a>',
    }),
  );
  await page.goto("https://personal-share-fixture.test/");
  await page.evaluate(() => {
    const w = window as typeof window & {
      __sent: string[];
      __saved: unknown[];
      __failRead: boolean;
      __failSend: boolean;
      __remembered: unknown;
    };
    w.__sent = [];
    w.__saved = [];
    w.__failRead = false;
    w.__failSend = false;
    w.__remembered = null;
    window.requestIdleCallback = () => 0;
    window.fetch = async (url, init) => {
      const u = String(url);
      const method = (init?.method ?? "GET").toUpperCase();
      if (u === "/public/me/share-preference") {
        if (method === "GET")
          return new Response(
            JSON.stringify(
              w.__remembered
                ? { remembered: w.__remembered, last: w.__remembered }
                : { remembered: null, last: null },
            ),
            { status: 200 },
          );
        if (method === "PUT") {
          w.__saved.push(JSON.parse(String(init?.body)));
          return new Response(JSON.stringify({ ok: true }), { status: 200 });
        }
        return new Response(JSON.stringify({ ok: true }), { status: 200 });
      }
      if (u === "/public/me/share-destination")
        return new Response(
          JSON.stringify({
            connected: true,
            binding_id: "900000000000000081",
            channel_url: "https://discord.com/channels/900000000000000083/900000000000000082",
          }),
          { status: w.__failRead ? 503 : 200 },
        );
      const fd = init?.body as FormData;
      w.__sent.push(String(fd.get("destination")));
      if (fd.get("destination") === "personal" && fd.get("binding_id") !== "900000000000000081")
        throw new Error("wrong binding");
      return new Response(
        JSON.stringify(
          w.__failSend
            ? { detail: "个人频道投递失败" }
            : { ok: true, queued: false, destination: "personal" },
        ),
        { status: w.__failSend ? 409 : 200 },
      );
    };
  });
  await page.evaluate(bridge);
  await page.evaluate(async () => {
    const c = document.createElement("canvas");
    c.width = c.height = 32;
    const ctx = c.getContext("2d");
    if (!ctx) throw new Error("no canvas");
    ctx.fillRect(0, 0, 32, 32);
    const blob = await new Promise<Blob>((resolve) =>
      c.toBlob((b) => {
        if (b) resolve(b);
      }, "image/png"),
    );
    const image = document.querySelector("img");
    if (!image) throw new Error("no image");
    image.src = URL.createObjectURL(blob);
    await image.decode();
  });
  const share = page.locator('a[href="/explore/register"]');
  await share.click();
  const dialog = page.getByRole("dialog");
  const destination = dialog.getByRole("combobox", { name: "转发到" });
  await expect(destination).toHaveValue("personal");
  await expect(dialog.getByText("会发到你的个人 Discord 频道，注意二传风险。")).toBeVisible();
  // 切换目的地只改警示文案：元数据勾选是独立的选择，不再被重置。
  const metadata = dialog.getByRole("checkbox", { name: "附带生成参数（提示词、种子）" });
  await expect(metadata).toBeChecked();
  await destination.selectOption("owl");
  await expect(dialog.getByText("会转发到 owl 服务器留存备份，注意二传风险。")).toBeVisible();
  await expect(metadata).toBeChecked();
  await destination.selectOption("personal");
  await dialog.getByRole("checkbox", { name: "记住这次选择" }).check();
  await dialog.getByRole("button", { name: "分享", exact: true }).click();
  await expect(share).not.toHaveAttribute("data-owl-busy", "1");
  expect(await page.evaluate(() => (window as unknown as { __sent: string[] }).__sent)).toEqual(["personal"]);
  await expect
    .poll(() => page.evaluate(() => (window as unknown as { __saved: unknown[] }).__saved.length))
    .toBe(1);
  expect(await page.evaluate(() => (window as unknown as { __saved: unknown[] }).__saved[0])).toEqual({
    destination: "personal",
    include_metadata: true,
    binding_id: "900000000000000081",
  });
  // 记住的选择当前可用：再点分享直接发送，不再弹窗；成功 toast 上留「分享设置」入口。
  await page.evaluate(() => {
    const w = window as typeof window & { __remembered: unknown };
    w.__remembered = {
      destination: "personal",
      include_metadata: false,
      binding_id: "900000000000000081",
    };
  });
  await share.click();
  await expect(dialog).toHaveCount(0);
  await expect(share).not.toHaveAttribute("data-owl-busy", "1");
  const sentToast = page
    .locator("[data-owl-toast]")
    .filter({ has: page.getByRole("button", { name: "分享设置" }) });
  await expect(sentToast).toBeVisible();
  await expect(sentToast).toContainText("已发送到你的个人频道");
  expect(await page.evaluate(() => (window as unknown as { __sent: string[] }).__sent)).toEqual([
    "personal",
    "personal",
  ]);
  await page.evaluate(() => {
    (window as unknown as { __failSend: boolean }).__failSend = true;
  });
  await share.click();
  await expect(share).not.toHaveAttribute("data-owl-busy", "1");
  expect(await page.evaluate(() => (window as unknown as { __sent: string[] }).__sent)).toEqual([
    "personal",
    "personal",
    "personal",
  ]);
  await page.evaluate(() => {
    (window as unknown as { __failRead: boolean }).__failRead = true;
    (window as unknown as { __remembered: unknown }).__remembered = null;
  });
  await share.click();
  await expect(dialog.getByRole("button", { name: "分享", exact: true })).toBeDisabled();
  await expect(dialog.getByRole("button", { name: "重试", exact: true })).toBeVisible();
  await dialog.getByRole("button", { name: "取消", exact: true }).click();
});

test("share settings opens as a second view inside the share dialog", async ({ page }) => {
  const bridge = readFileSync(
    resolve(import.meta.dirname, "../../deploy/nai-mirror-overrides/bridge.js"),
    "utf8",
  );
  await page.route("https://settings-fixture.test/**", (route) =>
    route.fulfill({
      contentType: "text/html",
      body: '<!doctype html><meta charset="utf-8"><img width="256" height="256"><a href="/explore/register">Upload to Explore</a>',
    }),
  );
  await page.goto("https://settings-fixture.test/");
  let remembered: unknown = null;
  let cleared = 0;
  const saved: unknown[] = [];
  await page.exposeFunction("__owlTestRemembered", () => remembered);
  await page.exposeFunction(
    "__owlTestSave",
    (body: { destination: string; include_metadata: boolean; binding_id: string | null }) => {
      saved.push(body);
      remembered = { ...body };
    },
  );
  await page.exposeFunction("__owlTestClear", () => {
    cleared += 1;
    remembered = null;
  });
  await page.evaluate(() => {
    window.requestIdleCallback = () => 0;
    window.fetch = async (url, init) => {
      const u = String(url);
      const method = (init?.method ?? "GET").toUpperCase();
      if (u === "/public/me/share-preference") {
        const state = await (
          window as unknown as { __owlTestRemembered: () => unknown }
        ).__owlTestRemembered();
        if (method === "GET")
          return new Response(
            JSON.stringify(state ? { remembered: state, last: state } : { remembered: null, last: null }),
            { status: 200 },
          );
        if (method === "DELETE") {
          await (window as unknown as { __owlTestClear: () => void }).__owlTestClear();
          return new Response(JSON.stringify({ ok: true }), { status: 200 });
        }
        await (window as unknown as { __owlTestSave: (body: unknown) => Promise<void> }).__owlTestSave(
          JSON.parse(String(init?.body)),
        );
        return new Response(JSON.stringify({ ok: true }), { status: 200 });
      }
      if (u === "/public/me/share-destination")
        return new Response(JSON.stringify({ connected: false }), { status: 200 });
      return new Response("{}", { status: 200 });
    };
  });
  await page.evaluate(bridge);
  // 分享设置不再往工具栏塞独立按钮：没有弹窗时页面上找不到这个入口。
  await expect(page.locator(".owl-settings-entry")).toHaveCount(0);
  await expect(page.getByRole("button", { name: "分享设置" })).toHaveCount(0);
  await page.evaluate(async () => {
    const c = document.createElement("canvas");
    c.width = c.height = 32;
    const ctx = c.getContext("2d");
    if (!ctx) throw new Error("no canvas");
    ctx.fillRect(0, 0, 32, 32);
    const blob = await new Promise<Blob>((resolve) =>
      c.toBlob((b) => {
        if (b) resolve(b);
      }, "image/png"),
    );
    const image = document.querySelector("img");
    if (!image) throw new Error("no image");
    image.src = URL.createObjectURL(blob);
    await image.decode();
  });
  await page.locator('a[href="/explore/register"]').click();
  const confirm = page.getByRole("dialog", { name: "分享到 Discord" });
  await expect(confirm).toBeVisible();
  await confirm.getByRole("button", { name: "分享设置" }).click();
  const settings = page.getByRole("dialog", { name: "分享设置" });
  await expect(settings.getByRole("combobox", { name: "分享到" })).toHaveValue("owl");
  // 等面板把服务端状态读回来（未连接个人频道时自动备份是禁用的），后面的勾选断言才不是碰运气。
  await expect(settings.getByRole("checkbox", { name: "出图后自动转发到个人频道" })).toBeDisabled();
  // 没有记住过选择时，附带生成参数默认是勾上的。
  await expect(settings.getByRole("checkbox", { name: "附带生成参数（提示词、种子）" })).toBeChecked();
  // 「记住分享选择」和自动备份一样是勾选框，不再是保存 / 忘记两个按钮。
  const remember = settings.getByRole("checkbox", { name: "记住分享选择" });
  await expect(remember).not.toBeChecked();
  await remember.check();
  await expect.poll(() => saved.length).toBe(1);
  expect(saved[0]).toEqual({ destination: "owl", include_metadata: true, binding_id: null });
  await expect(remember).toBeChecked();
  await remember.uncheck();
  await expect.poll(() => cleared).toBe(1);
  await expect(remember).not.toBeChecked();
  // 二级是同一个弹窗里的视图切换：返回后一级还在，不需要重新点分享图标。
  await settings.getByRole("button", { name: "返回", exact: true }).click();
  await expect(page.getByRole("dialog", { name: "分享到 Discord" })).toBeVisible();
  await page
    .getByRole("dialog", { name: "分享到 Discord" })
    .getByRole("button", { name: "取消", exact: true })
    .click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
});
