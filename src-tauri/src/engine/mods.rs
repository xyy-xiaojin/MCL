//! 本地 mod 管理：列出 `mods/` 下的 jar，并支持启用/停用。
//!
//! 启用/停用用的是**改扩展名**（`x.jar` ↔ `x.jar.disabled`）这个业界惯例，
//! 保证游戏只加载 `.jar`，同时文件仍然留在目录里、不丢不删。
use std::path::Path;

use serde::Serialize;

use crate::error::{Error, Result};

const ACTIVE: &str = ".jar";
const DISABLED: &str = ".jar.disabled";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModEntry {
    /// 磁盘上的真实文件名（前端操作时回传这个）。
    pub file_name: String,
    /// 去掉扩展名后的显示名。
    pub name: String,
    pub enabled: bool,
    pub size: u64,
}

/// 列出目录里的 mod。目录不存在时返回空列表而不是报错 —— 空目录是正常状态。
pub fn list(dir: &Path) -> Vec<ModEntry> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let file_name = entry.file_name().to_string_lossy().to_string();
        // 先判 .disabled：否则 "x.jar.disabled" 也会被 .jar 匹配上
        let (enabled, name) = if let Some(stem) = file_name.strip_suffix(DISABLED) {
            (false, stem.to_string())
        } else if let Some(stem) = file_name.strip_suffix(ACTIVE) {
            (true, stem.to_string())
        } else {
            continue;
        };
        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
        out.push(ModEntry {
            file_name,
            name,
            enabled,
            size,
        });
    }
    out.sort_by_key(|m| m.name.to_lowercase());
    out
}

/// 切换启用状态，返回切换后是否启用。
pub fn toggle(dir: &Path, file_name: &str) -> Result<bool> {
    // 只接受纯文件名：`../` 或带分隔符的名字一律拒绝
    if file_name.is_empty()
        || file_name.contains('/')
        || file_name.contains('\\')
        || file_name == ".."
        || file_name.starts_with('.')
    {
        return Err(Error::msg(format!("文件名不合法：{file_name}")));
    }
    let from = dir.join(file_name);
    if !from.is_file() {
        return Err(Error::msg(format!("找不到 mod 文件：{file_name}")));
    }

    let to = if let Some(stem) = file_name.strip_suffix(DISABLED) {
        dir.join(format!("{stem}{ACTIVE}"))
    } else if let Some(stem) = file_name.strip_suffix(ACTIVE) {
        dir.join(format!("{stem}{DISABLED}"))
    } else {
        return Err(Error::msg("只支持 .jar / .jar.disabled 两种后缀"));
    };

    std::fs::rename(&from, &to)?;
    Ok(to.to_string_lossy().ends_with(ACTIVE))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mcl-mods-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn lists_enabled_and_disabled_mods() {
        let dir = temp_dir("list");
        std::fs::write(dir.join("Sodium.jar"), b"x").unwrap();
        std::fs::write(dir.join("Iris.jar.disabled"), b"yy").unwrap();
        std::fs::write(dir.join("readme.txt"), b"ignored").unwrap();

        let mods = list(&dir);
        assert_eq!(mods.len(), 2, "只统计 jar 与 jar.disabled");
        let sodium = mods.iter().find(|m| m.name == "Sodium").unwrap();
        assert!(sodium.enabled);
        let iris = mods.iter().find(|m| m.name == "Iris").unwrap();
        assert!(!iris.enabled);
        assert_eq!(iris.size, 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn toggle_round_trips() {
        let dir = temp_dir("toggle");
        std::fs::write(dir.join("Sodium.jar"), b"x").unwrap();

        assert!(!toggle(&dir, "Sodium.jar").unwrap(), "应当变成停用");
        assert!(dir.join("Sodium.jar.disabled").is_file());
        assert!(toggle(&dir, "Sodium.jar.disabled").unwrap(), "应当重新启用");
        assert!(dir.join("Sodium.jar").is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn toggle_rejects_path_traversal() {
        let dir = temp_dir("traversal");
        for bad in ["../evil.jar", "sub/dir.jar", "..\\evil.jar", ""] {
            assert!(toggle(&dir, bad).is_err(), "应当拒绝 {bad:?}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_directory_lists_empty() {
        let dir = std::env::temp_dir().join("mcl-mods-does-not-exist");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(list(&dir).is_empty());
    }
}
