# syntax=docker/dockerfile:1.10
# 猫头鹰公益站 3.0：一个镜像三个二进制（nai-gate 网关 / nai-bots 机器人 / nai-ops 运维）。
# 由 GitHub Actions 构建并导出为 tar 附件（owl-gate-image）；服务器（2 核）下载后 docker load，不 pull、不在本机编译。

# ---------- 前端 ----------
FROM node:24-bookworm-slim AS web
WORKDIR /src/web
RUN corepack enable
COPY web/package.json web/pnpm-lock.yaml ./
RUN pnpm install --frozen-lockfile
COPY web/ ./
RUN pnpm vite build

# ---------- Rust 依赖缓存（cargo-chef） ----------
FROM rust:1.99-bookworm AS chef
# BoringSSL（wreq 的 Chrome TLS 指纹）需要 cmake / clang；nai-gate 的分配器 mimalloc 也在这里用 C 编译器编
RUN apt-get update && apt-get install -y --no-install-recommends cmake clang libclang-dev \
 && rm -rf /var/lib/apt/lists/* \
 && cargo install cargo-chef --locked --version 0.1.78
WORKDIR /src

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS build
COPY --from=planner /src/recipe.json recipe.json
RUN cargo chef cook --release --locked --recipe-path recipe.json
COPY . .
COPY --from=web /src/web/dist web/dist
RUN cargo build --release --locked --bin nai-gate --bin nai-bots --bin nai-ops

# ---------- 运行镜像 ----------
FROM gcr.io/distroless/cc-debian12:nonroot
# UID 10001：与 2.x 的数据目录属主一致（chown -R 10001:10001 data 只需做过一次）
USER 10001:10001
WORKDIR /app
ENV DATA_DIR=/app/data TZ=Asia/Shanghai RUST_LOG=info
COPY --from=build /src/target/release/nai-gate /src/target/release/nai-bots /src/target/release/nai-ops /usr/local/bin/
EXPOSE 8000
HEALTHCHECK --interval=30s --timeout=5s --retries=3 CMD ["/usr/local/bin/nai-gate", "healthcheck"]
ENTRYPOINT ["/usr/local/bin/nai-gate"]
CMD ["serve"]
