//! rust-embed 在编译期需要 web/dist 存在。本地只跑 Rust 测试时还没构建前端：放一个占位页，
//! 运行时网关会提示「前端未构建」。CI / Docker 里先构建前端，这里什么都不做。
fn main() {
    let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/dist");
    if !dist.exists() {
        let _ = std::fs::create_dir_all(&dist);
        let _ = std::fs::write(
            dist.join(".placeholder"),
            "run `pnpm --dir web vite build`\n",
        );
    }
    println!("cargo:rerun-if-changed=../../web/dist");
}
