//! 文件小工具：原子写入私密文件（0600）。旧版有三份几乎一样的实现（admin / token_store / params）。

use std::io::Write;
use std::path::Path;

/// 先写同目录 `.tmp`（强制 0600，覆盖残留旧文件的权限）→ fsync → rename。
pub fn write_private(path: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let tmp = path.with_file_name(format!("{name}.tmp"));
    {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        f.write_all(data)?;
        f.flush()?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}
