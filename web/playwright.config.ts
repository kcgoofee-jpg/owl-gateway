import { defineConfig, devices } from "@playwright/test";

/**
 * 端到端测试：连的是已经在跑的前端（默认 Vite 开发服务 http://localhost:5174），不在这里启动服务。
 * 后端是 `cargo xtask dev`（真实的 Rust 网关 + 假上游 + 合成演示数据，端口 18000 / 18001）。
 *
 *   cargo xtask dev                                           # 终端 1：网关 + 假上游，Ctrl-C 退出
 *   cd web && pnpm exec vite --port 5174 --strictPort         # 终端 2：前端（接口代理到 VITE_GATE，默认 http://127.0.0.1:18000）
 *   cd web && E2E_BASE=http://localhost:5174 pnpm exec playwright test   # 终端 3
 *
 * `cargo xtask dev` 每次启动都重建数据目录（target/owl-dev）并写出 cookies.json（后台密码 / 会话、成员会话 / Key），
 * 用例读它（见 e2e/devEnv.ts）；E2E_COOKIES 可改它的位置，没有这个文件时成员相关用例自动跳过。
 * 用例会改数据（停用 / 重新生成 Key、新建 Key……）；想回到干净的数据就重启 `cargo xtask dev`。
 */
export default defineConfig({
  testDir: "./e2e",
  fullyParallel: true,
  // `cargo xtask dev` fixtures share one synthetic database; parallel mutation can trip request cooldowns.
  // 多个真实出图用例共用同一个假上游，本地也强制单 worker，避免并发限流互踩。
  workers: 1,
  forbidOnly: !!process.env.CI,
  retries: 0,
  reporter: [["list"]],
  use: {
    baseURL: process.env.E2E_BASE ?? "http://localhost:5174",
    trace: "retain-on-failure",
    // 默认按「减少动态效果」跑：开屏不播（不挡点击）、滚动显影直接是最终状态（排版和对比度检查看到的就是成品）。
    // 动效本身由 landing.spec.ts 里的「开屏」用例单独打开来测。
    reducedMotion: "reduce",
  },
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],
});
