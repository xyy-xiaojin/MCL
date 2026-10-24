//! Java 运行时自动下载（Eclipse Adoptium Temurin JRE）。
//!
//! 小白朋友的机器上往往没有 Java，之前只能自己去官网找——这是"打开就能玩"
//! 的分水岭。这里把它收进启动器：
//!
//! 1. **查元数据**：`api.adoptium.net/v3/assets/latest/<major>/hotspot`，
//!   按 OS / 架构取 JRE 的下载地址与 sha256；
//! 2. **下载**：走全局下载队列（界面上有进度、能暂停），
//!    地址优先清华 TUNA 镜像（国内直连 Adoptium 的 GitHub 经常只有几十 KB/s），
//!    失败换官方；下载完校验 sha256；
//! 3. **解压**：Windows 是 zip，用 `zip` crate 解；Linux/macOS 是 tar.gz，
//!    调系统 `tar`（这俩平台必然有）；
//! 4. **注册**：把解出来的 `bin/java` 写进 `settings.java_path`，
//!    之后的探测与启动全部走正常路径。
//!
//! JRE 落在 `<数据目录>/jre/<major>/`，跟 versions / libraries 一样归 MCL 管，
//! 删数据目录就一起没了，不会污染系统。
use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::engine::java;
use crate::engine::queue::NewTask;
use crate::error::{Error, Result};
use crate::state::AppState;

/// Adoptium 的 v3 API。
const ADOPTIUM_API: &str = "https://api.adoptium.net/v3/assets/latest";
/// 清华 TUNA 的 Adoptium 镜像（目录结构与 GitHub release 相同）。
const TUNA: &str = "https://mirrors.tuna.tsinghua.edu.cn/Adoptium";

/// Adoptium API 返回的条目里我们关心的部分。
#[derive(Debug, Deserialize)]
struct AdoptiumEntry {
    binary: AdoptiumBinary,
}

#[derive(Debug, Deserialize)]
struct AdoptiumBinary {
    package: AdoptiumPackage,
}

#[derive(Debug, Deserialize)]
struct AdoptiumPackage {
    name: String,
    link: String,
    #[serde(default)]
    checksum: String,
    #[serde(default)]
    size: u64,
}

/// 当前系统的 Adoptium 平台名。
fn platform() -> (&'static str, &'static str) {
    let os = if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "mac"
    } else {
        "linux"
    };
    let arch = if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "x64"
    };
    (os, arch)
}

/// 查某个主版本最新的 Temurin JRE。
async fn fetch_latest(state: &AppState, major: i64) -> Result<AdoptiumPackage> {
    let (os, arch) = platform();
    let url = format!("{ADOPTIUM_API}/{major}/hotspot?os={os}&architecture={arch}&image_type=jre");
    let text = net_get(&state, &url).await?;
    let entries: Vec<AdoptiumEntry> = serde_json::from_str(&text)
        .map_err(|e| Error::msg(format!("Adoptium 元数据解析失败：{e}")))?;
    entries
        .into_iter()
        .map(|e| e.binary.package)
        .find(|p| !p.link.is_empty())
        .ok_or_else(|| Error::msg(format!("Adoptium 没有 Java {major} 的 {os}/{arch} JRE")))
}

/// 元数据只有官方 API 一个来源，但下载地址会换镜像。
/// 这里单独包一层：文本很小，重试一次足够。
async fn net_get(state: &AppState, url: &str) -> Result<String> {
    crate::engine::net::get_text(&state.client, url).await
}

/// 已安装的 JRE：`jre/<major>` 下能找到可用的 java 就直接用。
fn installed_java(paths: &crate::state::Paths, major: i64) -> Option<java::JavaInfo> {
    let root = paths.jre_dir().join(major.to_string());
    let exe = find_java(&root)?;
    let info = java::JavaInfo {
        path: exe.to_string_lossy().to_string(),
        major: java::major_version(&exe).unwrap_or(0),
        source: format!("MCL 下载（Java {major}）"),
    };
    (info.major > 0).then_some(info)
}

/// 下载并安装指定主版本的 JRE，完成后写进设置并返回它的 JavaInfo。
pub async fn install(state: &AppState, major: i64) -> Result<java::JavaInfo> {
    if !(8..=99).contains(&major) {
        return Err(Error::msg(format!("Java 版本号不合法：{major}")));
    }
    if let Some(info) = installed_java(&state.paths, major) {
        // 已经装过：直接注册进设置，等于修复了"设置里指向了别处"的情况
        register(state, &info)?;
        return Ok(info);
    }

    let package = fetch_latest(state, major).await?;
    if package.size > 0 {
        // JRE 本体不到 200 MB，预检给 512 MB 余量
        crate::engine::disk::ensure_free(
            &state.paths.root,
            package.size + 512 * 1024 * 1024,
            &format!("Java {major} 运行时"),
        )?;
    }

    let dest_dir = state.paths.jre_downloads();
    std::fs::create_dir_all(&dest_dir)?;
    let archive = dest_dir.join(&package.name);

    // TUNA 的文件名与 GitHub release 完全一致，只换域名；
    // 镜像没有这个文件（404）时队列会自动换官方地址。
    let (os, arch) = platform();
    let image = "jre";
    let tuna_url = format!("{TUNA}/{major}/{image}/{arch}/{os}/{}", package.name);

    let task = NewTask::new(
        "java",
        format!("Java {major} 运行时（Temurin JRE）"),
        tuna_url.clone(),
        archive.clone(),
    )
    .with_fallbacks(vec![package.link.clone()])
    .with_size(Some(package.size));
    let ids = state.queue.enqueue(vec![task]);
    let failures = state.queue.wait(&ids).await;
    if !failures.is_empty() {
        let _ = std::fs::remove_file(&archive);
        return Err(Error::msg(format!(
            "Java 运行时下载失败：{}",
            failures
                .iter()
                .map(|(l, r)| format!("{l}：{r}"))
                .collect::<Vec<_>>()
                .join("；")
        )));
    }

    // 队列只校验 SHA-1（Mojang 的清单是 sha1），Adoptium 给的是 sha256，
    // 在这里补一道：装一个坏的 Java 比不装更坑。
    let actual = sha256_of(&archive).await?;
    if !package.checksum.is_empty() && !actual.eq_ignore_ascii_case(&package.checksum) {
        let _ = std::fs::remove_file(&archive);
        return Err(Error::msg(format!(
            "Java 下载内容校验不过（期望 sha256 {}，实际 {actual}），已删除。再点一次会重新下载",
            &package.checksum[..16.min(package.checksum.len())]
        )));
    }

    // 解压前清掉半截目录：上次解压失败可能留了一堆文件
    let jre_root = state.paths.jre_dir().join(major.to_string());
    let _ = std::fs::remove_dir_all(&jre_root);
    std::fs::create_dir_all(&jre_root)?;
    let used_mirror = state.source() != "official";
    let archive_path = archive.clone();
    let target = jre_root.clone();
    let is_zip = cfg!(windows) || package.name.to_ascii_lowercase().ends_with(".zip");
    tokio::task::spawn_blocking(move || -> Result<()> {
        if is_zip {
            extract_zip(&archive_path, &target)
        } else {
            extract_targz(&archive_path, &target)
        }
    })
    .await
    .map_err(|e| Error::msg(format!("解压 Java 失败：{e}")))??;
    let _ = std::fs::remove_file(&archive);

    let exe = find_java(&jre_root).ok_or_else(|| {
        Error::msg("解压完成后没找到 bin/java，压缩包内容可能不完整，请重试")
    })?;
    let info = java::JavaInfo {
        path: exe.to_string_lossy().to_string(),
        major: java::major_version(&exe).unwrap_or(major),
        source: format!("MCL 下载（{}）", if used_mirror { "镜像" } else { "官方" }),
    };
    if info.major > 0 && info.major != major {
        return Err(Error::msg(format!(
            "下载到的 Java 主版本是 {}，与请求的 {major} 不符",
            info.major
        )));
    }
    register(state, &info)?;
    Ok(info)
}

/// 把下载好的 Java 写进设置：之后的探测、启动、内存计算全走正常路径。
fn register(state: &AppState, info: &java::JavaInfo) -> Result<()> {
    let mut settings = state.settings();
    settings.java_path = info.path.clone();
    let normalized = settings.normalized();
    normalized.save(&state.paths)?;
    if let Ok(mut guard) = state.settings.write() {
        *guard = normalized;
    }
    Ok(())
}

async fn sha256_of(path: &Path) -> Result<String> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let mut file = std::fs::File::open(&path)?;
        let mut hasher = Sha256::new();
        std::io::copy(&mut file, &mut hasher)?;
        let digest = hasher.finalize();
        Ok(digest.iter().map(|b| format!("{b:02x}")).collect())
    })
    .await
    .map_err(|e| Error::msg(format!("校验任务异常：{e}")))?
}

/// 在解压目录里找 `bin/java`（Adoptium 的布局是 `jdk-21.x.y+z/bin/java`，最多三层）。
fn find_java(root: &Path) -> Option<PathBuf> {
    find_java_at(root, 0)
}

fn find_java_at(dir: &Path, depth: u32) -> Option<PathBuf> {
    let exe = if cfg!(windows) { "java.exe" } else { "java" };
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        // 读不了的子目录跳过，不能让整个搜索断在这里
        Err(_) => return None,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if depth < 3 {
                if let Some(found) = find_java_at(&path, depth + 1) {
                    return Some(found);
                }
            }
        } else if path.file_name().map(|n| n == exe).unwrap_or(false) {
            return Some(path);
        }
    }
    None
}

/// Windows / 通用 zip 解包。带 zip-slip 防护：条目路径逃出目标目录就拒绝。
fn extract_zip(archive: &Path, target: &Path) -> Result<()> {
    let file = std::fs::File::open(archive)?;
    let mut zip = zip::ZipArchive::new(file)?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let Some(name) = entry.enclosed_name() else {
            continue;
        };
        let out = target.join(name);
        if entry.is_dir() {
            std::fs::create_dir_all(&out)?;
        } else {
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut out_file = std::fs::File::create(&out)?;
            std::io::copy(&mut entry, &mut out_file)?;
        }
    }
    Ok(())
}

/// Linux / macOS 的 tar.gz：直接调系统 tar（这两个平台必然有）。
fn extract_targz(archive: &Path, target: &Path) -> Result<()> {
    let status = std::process::Command::new("tar")
        .arg("-xzf")
        .arg(archive)
        .arg("-C")
        .arg(target)
        .status()?;
    if !status.success() {
        return Err(Error::msg("tar 解压失败"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_names_match_adoptium_naming() {
        let (os, arch) = platform();
        assert!(matches!(os, "windows" | "linux" | "mac"));
        assert!(matches!(arch, "x64" | "aarch64"));
    }

    #[test]
    fn tuna_url_derives_from_package_name() {
        // 与 fetch_latest 拼出来的 URL 形状保持一致
        let name = "OpenJDK21U-jre_x64_windows_hotspot_21.0.5_11.zip";
        assert_eq!(
            format!("{TUNA}/21/jre/x64/windows/{name}"),
            "https://mirrors.tuna.tsinghua.edu.cn/Adoptium/21/jre/x64/windows/OpenJDK21U-jre_x64_windows_hotspot_21.0.5_11.zip"
        );
    }
}
