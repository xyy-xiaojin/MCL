//! 磁盘剩余空间预检。
//!
//! 下 300 MB 客户端时磁盘只剩 100 MB，之前会一路下到写满才失败，
//! 玩家看到的还是一条莫名其妙的"写入失败"。这里在入队前先问一次操作系统。
//!
//! 只查 `root` 所在卷就够：versions / libraries / assets 全在 root 下。

use crate::error::{Error, Result};

/// 查一个目录所在卷的剩余字节数。目录不存在时返回 `None`（调用方自行兜底）。
pub fn free_bytes(dir: &std::path::Path) -> Option<u64> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

        let wide: Vec<u16> = dir
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut available: u64 = 0;
        // 安全性：指针都是本函数自己的局部变量，调用同步完成
        let ok = unsafe {
            GetDiskFreeSpaceExW(wide.as_ptr(), &mut available, std::ptr::null_mut(), std::ptr::null_mut())
        };
        (ok != 0).then_some(available)
    }

    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let c = match std::ffi::CString::new(dir.as_os_str().as_bytes()) {
            Ok(c) => c,
            Err(_) => return None,
        };
        let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
        // statvfs 的 f_bavail 才是"非特权进程可用的剩余空间"（f_bfree 含保留块）
        let ok = unsafe { libc::statvfs(c.as_ptr(), &mut stat) } == 0;
        if ok {
            Some(stat.f_bavail as u64 * stat.f_bsize as u64)
        } else {
            None
        }
    }
}

/// 确保目标卷至少还有 `needed` 字节可用，否则给出能看懂的错误。
///
/// 拿不到剩余空间（权限、临时目录被删）时放行——预检失败不该拦住本来能下的任务。
pub fn ensure_free(dir: &std::path::Path, needed: u64, label: &str) -> Result<()> {
    let Some(free) = free_bytes(dir) else {
        return Ok(());
    };
    if free >= needed {
        return Ok(());
    }
    Err(Error::msg(format!(
        "磁盘空间不够：装「{label}」还需要约 {}，{} 只剩 {}。清一下空间再试",
        human_mb(needed),
        dir.display(),
        human_mb(free),
    )))
}

fn human_mb(bytes: u64) -> String {
    if bytes >= 1024 * 1024 * 1024 {
        format!("{:.1} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    } else {
        format!("{} MB", bytes / (1024 * 1024))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temp_dir_reports_free_space() {
        assert!(free_bytes(&std::env::temp_dir()).is_some());
    }

    #[test]
    fn ensure_free_passes_when_enough() {
        // 1 KB 的需求在临时目录上永远满足
        ensure_free(&std::env::temp_dir(), 1024, "测试").unwrap();
    }

    #[test]
    fn ensure_free_rejects_absurd_request() {
        // 1 EB 的需求在任何机器上都不满足
        let err = ensure_free(&std::env::temp_dir(), u64::MAX / 2, "测试").unwrap_err();
        assert!(err.to_string().contains("磁盘空间不够"));
    }

    #[test]
    fn missing_dir_is_not_an_error() {
        let missing = std::env::temp_dir().join("mcl-no-such-dir-for-disk");
        ensure_free(&missing, u64::MAX / 2, "测试").unwrap();
    }
}
