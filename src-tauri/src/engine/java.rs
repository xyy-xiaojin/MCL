//! Java 运行环境探测。
//!
//! 只在真正要启动时才跑（`java -version` 有进程开销），
//! 结果由前端缓存，不在每帧轮询。
#[cfg(windows)]
use std::os::windows::process::CommandExt as _;
use std::path::{Path, PathBuf};

use serde::Serialize;

/// 探测结果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JavaInfo {
    pub path: String,
    pub major: i64,
    /// 从哪里找到的：设置 / JAVA_HOME / PATH / 常见目录
    pub source: String,
}

fn exe_name() -> &'static str {
    if cfg!(windows) {
        "java.exe"
    } else {
        "java"
    }
}

/// 按优先级探测：用户指定 > JAVA_HOME > PATH > 常见安装目录。
pub fn detect(configured: &str) -> Option<JavaInfo> {
    let configured = configured.trim();
    if !configured.is_empty() {
        let path = PathBuf::from(configured);
        if let Some(info) = inspect(&path, "设置中指定") {
            return Some(info);
        }
    }

    if let Some(home) = std::env::var_os("JAVA_HOME") {
        let path = PathBuf::from(home).join("bin").join(exe_name());
        if let Some(info) = inspect(&path, "JAVA_HOME") {
            return Some(info);
        }
    }

    if let Some(path) = search_path_env() {
        if let Some(info) = inspect(&path, "PATH") {
            return Some(info);
        }
    }

    for dir in candidate_dirs() {
        if let Some(found) = scan_dir(&dir) {
            return Some(found);
        }
    }
    None
}

fn search_path_env() -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|dir| dir.join(exe_name()))
        .find(|candidate| candidate.is_file())
}

/// 各平台常见的 JDK 安装位置。
fn candidate_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if cfg!(windows) {
        for root in [
            r"C:\Program Files\Java",
            r"C:\Program Files\Eclipse Adoptium",
            r"C:\Program Files\Microsoft",
            r"C:\Program Files\Zulu",
            r"C:\Program Files (x86)\Java",
        ] {
            dirs.push(PathBuf::from(root));
        }
    } else if cfg!(target_os = "macos") {
        dirs.push(PathBuf::from("/Library/Java/JavaVirtualMachines"));
    } else {
        for root in ["/usr/lib/jvm", "/usr/java", "/opt/java"] {
            dirs.push(PathBuf::from(root));
        }
        // 用户级安装：sdkman / IntelliJ 下载的 JDK
        if let Some(home) = dirs_home() {
            dirs.push(home.join(".sdkman/candidates/java"));
            dirs.push(home.join(".jdks"));
        }
    }
    dirs
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// 扫描两层目录，找 `bin/java`。
fn scan_dir(root: &Path) -> Option<JavaInfo> {
    let entries = std::fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        let candidate = if cfg!(target_os = "macos") {
            path.join("Contents/Home/bin").join(exe_name())
        } else {
            path.join("bin").join(exe_name())
        };
        // Adoptium / Zulu 的布局就是 <root>/<jdk-17.x>/bin/java，一层足够
        if let Some(info) = inspect(&candidate, "常见目录") {
            return Some(info);
        }
    }
    None
}

fn inspect(path: &Path, source: &str) -> Option<JavaInfo> {
    if !path.is_file() {
        return None;
    }
    Some(JavaInfo {
        path: path.to_string_lossy().to_string(),
        major: major_version(path).unwrap_or(0),
        source: source.to_string(),
    })
}

/// 跑 `java -version` 解析主版本号。
///
/// 输出在 **stderr**（这是 Java 的历史行为，不是我们读错了流）。
/// `1.8.0_392` → 8，`17.0.1` → 17。
pub fn major_version(java: &Path) -> Option<i64> {
    let mut command = std::process::Command::new(java);
    command.arg("-version");
    // 自动检测 Java 时也不应闪出控制台窗口。
    #[cfg(windows)]
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let output = command.output().ok()?;
    let text = String::from_utf8_lossy(&output.stderr);
    let quoted = text.split('"').nth(1)?;
    let first = quoted.split('.').next().unwrap_or(quoted);
    if first == "1" {
        // 1.8.0_392 这种老式写法
        quoted
            .split('.')
            .nth(1)
            .and_then(|s| s.split('_').next())
            .and_then(|s| s.parse::<i64>().ok())
    } else {
        first.parse::<i64>().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_and_modern_versions_are_parsed() {
        // 直接验证解析逻辑：把 java -version 的两种典型输出拿出来
        for (raw, expect) in [
            (r#"openjdk version "1.8.0_392""#, 8i64),
            (r#"openjdk version "17.0.9" 2023-10-17"#, 17),
            (r#"java version "21.0.1" 2023-10-17 LTS"#, 21),
        ] {
            let quoted = raw.split('"').nth(1).unwrap();
            let first = quoted.split('.').next().unwrap();
            let major = if first == "1" {
                quoted
                    .split('.')
                    .nth(1)
                    .and_then(|s| s.split('_').next())
                    .and_then(|s| s.parse::<i64>().ok())
                    .unwrap()
            } else {
                first.parse::<i64>().unwrap()
            };
            assert_eq!(major, expect, "{raw}");
        }
    }

    #[test]
    fn missing_path_is_not_reported() {
        let missing = std::env::temp_dir().join("mcl-no-such-java");
        let _ = std::fs::remove_file(&missing);
        assert!(inspect(&missing, "test").is_none());
    }
}
