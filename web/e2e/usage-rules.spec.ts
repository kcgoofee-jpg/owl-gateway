import { expect, test } from "@playwright/test";
import { readCreds } from "./devEnv";
import { asMember, noHorizontalScroll } from "./helpers";
import { orphans } from "./orphans";

for (const member of [false, true])
  for (const width of [1440, 390, 320]) {
    test(`使用规则 ${member ? "成员" : "游客"} ${width}px`, async ({ page, context }) => {
      if (member) {
        test.skip(!readCreds().member, "需要合成成员会话");
        await asMember(context);
      }
      await page.setViewportSize({ width, height: 900 });
      await page.goto("/");
      if (!member) await page.getByRole("tab", { name: "使用须知" }).click();
      const terms = page.locator(".lp-rules");
      for (const rule of [
        "商业化使用",
        "共享、转借、倒卖或公开 Key",
        "绕过限制或异常高频调用",
        "提交违法、极端有害或违反上游规则的内容",
        "干扰服务或影响其他用户正常使用",
      ]) {
        await expect(terms.getByText(rule, { exact: true })).toBeVisible();
      }
      await expect(terms).toContainText("非商业公益服务，不保证持续可用");
      await expect(terms).toContainText("不保存提示词或生成图");
      await expect(page.locator(".lp-foot")).toHaveCount(0);
      expect(await noHorizontalScroll(page, { loose: true })).toBe(true);
      expect(await orphans(page)).toEqual([]);
    });
  }
