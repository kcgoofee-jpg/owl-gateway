import { readFile } from "node:fs/promises";
import { extname, join, normalize, resolve } from "node:path";
import preact from "@preact/preset-vite";
import type { Plugin } from "vite";
import { defineConfig } from "vitest/config";

// 开发时模拟网关的页面路由：/ → landing.html，/admin、/admin/* → admin.html
function pageRoutes(): Plugin {
  return {
    name: "owl-page-routes",
    configureServer(server) {
      server.middlewares.use((req, res, next) => {
        const url = req.url ?? "/";
        const path = url.split("?")[0] ?? "/";
        const qs = url.slice(path.length);
        if (path === "/") req.url = `/_owl/landing.html${qs}`;
        else if (path === "/workbench") {
          res.writeHead(302, { location: "/image" });
          res.end();
          return;
        } else if ((path === "/admin" || path.startsWith("/admin/")) && !path.startsWith("/admin/api/"))
          req.url = `/_owl/admin.html${qs}`;
        next();
      });
    },
  };
}

// 开发时把 /poker-assets 指到仓库外的 mirror 目录（线上由网关的 STATIC_DIR 提供）
function pokerAssets(): Plugin {
  const root = resolve(import.meta.dirname, "../mirror/poker-assets");
  const MIME: Record<string, string> = {
    ".png": "image/png",
    ".jpg": "image/jpeg",
    ".webp": "image/webp",
    ".json": "application/json",
  };
  return {
    name: "owl-poker-assets",
    configureServer(server) {
      server.middlewares.use("/poker-assets", async (req, res, next) => {
        const rel = decodeURIComponent((req.url ?? "").split("?")[0] ?? "");
        const full = normalize(join(root, rel));
        if (!full.startsWith(root)) return next();
        try {
          const buf = await readFile(full);
          res.setHeader("content-type", MIME[extname(full)] ?? "application/octet-stream");
          res.end(buf);
        } catch {
          next();
        }
      });
    },
  };
}

// 本站前端使用 /_owl/ 前缀，和官网镜像的静态资源路径分开。
// 多页：首页和后台各一个入口，网关按路径返回对应的 HTML（并注入首屏数据）。
export default defineConfig({
  base: "/_owl/",
  plugins: [pageRoutes(), pokerAssets(), preact()],
  build: {
    outDir: "dist",
    assetsDir: ".",
    target: "es2023",
    sourcemap: false,
    cssCodeSplit: true,
    // 字体不内联成 data: URI：CSP 只允许同源字体文件（font-src 'self'）
    assetsInlineLimit: (file) => (file.endsWith(".woff2") ? false : undefined),
    rollupOptions: {
      input: {
        landing: resolve(import.meta.dirname, "landing.html"),
        admin: resolve(import.meta.dirname, "admin.html"),
      },
    },
  },
  server: {
    port: 5173,
    proxy: {
      // 官网镜像页（/image）整页由网关渲染，静态资源也在网关的 STATIC_DIR 里，
      // 开发与 e2e 只把这几条路径转过去即可
      "^/(image|bridge.js|_next|tokenizer|cf-fonts|icons|manifest.json|updateReload.json)": {
        target: process.env.VITE_GATE ?? "http://127.0.0.1:18000",
      },
      "^/(public|admin/api|self-register|login|logout|member|v1|ai|nai|status|livez|readyz|healthz|announcement|_owl/status-page)":
        {
          target: process.env.VITE_GATE ?? "http://127.0.0.1:18000",
          // Preserve the browser authority so strict same-origin checks also run in development.
          changeOrigin: false,
        },
    },
  },
  test: {
    environment: "happy-dom",
    include: ["src/**/*.test.{ts,tsx}"],
  },
});
