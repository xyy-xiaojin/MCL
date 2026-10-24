//! 实例导出：把 mods / config / 存档按规则打成 zip。
//!
//! 目录结构对齐 Modrinth 的 `.mrpack`：所有内容放在 `overrides/` 下，
//! 再加一个 `mcl-manifest.json` 记录版本、加载器和文件清单。
//! 这样导出物既是我们自己的备份，也能被导入逻辑直接读回。
//!
//! 规则参考 PCL 的 `PageInstanceExport`：默认只带"能重建游戏体验"的目录，
//! 日志、崩溃报告、缓存一律排除——它们只会让包变大且没法用。
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::{Error, Result};
use crate::state::Paths;

/// 单个目录最多扫这么多文件，防止误把整个磁盘框进来。
const MAX_FILES: usize = 20_000;
const MAX_DEPTH: usize = 12;

/// 导出时勾选的项。
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ExportOptions {
    pub name: String,
    pub include_mods: bool,
    pub include_config: bool,
    pub include_saves: bool,
    pub include_resourcepacks: bool,
    pub include_shaderpacks: bool,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            name: String::new(),
            include_mods: true,
            include_config: true,
            include_saves: false,
            include_resourcepacks: false,
            include_shaderpacks: false,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportReport {
    pub path: String,
    pub files: usize,
    pub bytes: u64,
}

/// 永远不打包的目录名。
const SKIP_DIRS: [&str; 12] = [
    "logs",
    "crash-reports",
    "structureCacheV1",
    ".cache",
    ".fabric",
    ".git",
    "avatar-cache",
    "cosmetic-cache",
    ".mixin.out",
    "screenshots",
    "debug",
    "local",
];

/// 永远不打包的扩展名 / 文件名。
fn is_noise(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".log")
        || lower.ends_with(".log.gz")
        || lower.ends_with(".dat_old")
        || lower.ends_with(".bakacoreinfo")
        || lower == "log4j2.xml"
        || lower == "hmclversion.cfg"
        || lower == "usercache.json"
}

/// 按勾选项决定一个顶层名字该不该进包。
fn wanted(top: &str, options: &ExportOptions) -> bool {
    match top {
        "mods" => options.include_mods,
        "config" | "defaultconfigs" | "kubejs" | "scripts" => options.include_config,
        "saves" => options.include_saves,
        "resourcepacks" => options.include_resourcepacks,
        "shaderpacks" => options.include_shaderpacks,
        // 这几个是单文件，跟着 config 一起走
        "options.txt" | "servers.dat" | "optionsshaders.txt" => options.include_config,
        _ => false,
    }
}

/// 已经是压缩格式的扩展名。
///
/// mod / 材质包 / 光影包本身就是 jar、zip、png、ogg，再 deflate 一遍几乎不省空间，
/// 却要烧满一个核心：实测 84 MB 的 jar 压了 1969 ms，包只小了 7%。
/// 这些直接 `Stored`（原样拷贝），导出立刻从"CPU 密集"变成"磁盘拷贝"。
fn already_compressed(name: &str) -> bool {
    const EXTS: [&str; 15] = [
        "jar", "zip", "mrpack", "png", "jpg", "jpeg", "gif", "webp", "ogg", "mp3", "mca", "mcr",
        "dat", "gz", "xz",
    ];
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|ext| EXTS.iter().any(|known| known.eq_ignore_ascii_case(ext)))
        .unwrap_or(false)
}

struct Collected {
    /// 相对 game 目录的路径，统一用 `/`
    relative: String,
    absolute: PathBuf,
}

fn walk(dir: &Path, base: &Path, out: &mut Vec<Collected>, depth: usize) -> Result<()> {
    if depth > MAX_DEPTH || out.len() >= MAX_FILES {
        return Ok(());
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        // 权限不足的目录直接跳过，不让整次导出失败
        Err(_) => return Ok(()),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        // file_type() 来自 read_dir 本身，不必再为每个条目多打两次 stat
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            if SKIP_DIRS.iter().any(|skip| skip.eq_ignore_ascii_case(&name)) {
                continue;
            }
            walk(&path, base, out, depth + 1)?;
        } else if kind.is_file() && !is_noise(&name) {
            let Ok(relative) = path.strip_prefix(base) else {
                continue;
            };
            out.push(Collected {
                relative: relative.to_string_lossy().replace('\\', "/"),
                absolute: path,
            });
            if out.len() >= MAX_FILES {
                return Ok(());
            }
        }
    }
    Ok(())
}

/// 导出某个版本的实例。
///
/// 版本已经隔离，所以导出的是 `versions/<id>/game/` 这一份，
/// 不会把别的版本的 mod 混进来。
///
/// 传 [`Paths`] 而不是整个 `AppState`，是为了让调用方能把它丢进
/// `spawn_blocking`：打包上 GB 的文件 + 压缩是纯阻塞活，不该占 async worker。
pub fn export(
    paths: &Paths,
    options: &ExportOptions,
    version_id: &str,
) -> Result<ExportReport> {
    if version_id.trim().is_empty() {
        return Err(Error::msg("请先在「版本管理」里选择一个版本再导出"));
    }
    let root = paths.instance_game(version_id);
    if !root.is_dir() {
        return Err(Error::msg("这个版本还没有游戏目录，先启动一次再导出"));
    }
    let game_version = detect_game_version(paths, version_id);

    // 1) 收集文件：只走勾选了的顶层目录 / 文件
    let mut collected: Vec<Collected> = Vec::new();
    for top in [
        "mods",
        "config",
        "defaultconfigs",
        "kubejs",
        "scripts",
        "saves",
        "resourcepacks",
        "shaderpacks",
    ] {
        if !wanted(top, options) {
            continue;
        }
        let dir = root.join(top);
        if dir.is_dir() {
            walk(&dir, &root, &mut collected, 0)?;
        }
    }
    for file in ["options.txt", "servers.dat", "optionsshaders.txt"] {
        if !wanted(file, options) {
            continue;
        }
        let path = root.join(file);
        if path.is_file() {
            collected.push(Collected {
                relative: file.to_string(),
                absolute: path,
            });
        }
    }

    if collected.is_empty() {
        return Err(Error::msg(
            "按当前勾选没有找到任何可导出的内容（mods / config 可能是空的）",
        ));
    }

    // 2) 输出到 exports/ 下，文件名带时间戳避免覆盖
    let exports = paths.root.join("exports");
    std::fs::create_dir_all(&exports)?;
    let display = if options.name.trim().is_empty() {
        "MCL-instance".to_string()
    } else {
        safe_name(options.name.trim())
    };
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // 同一秒里点两次导出不能互相覆盖：文件名带的是秒级时间戳
    let mut out_path = exports.join(format!("{display}-{stamp}.zip"));
    if out_path.exists() {
        for index in 2..1000u32 {
            let candidate = exports.join(format!("{display}-{stamp}-{index}.zip"));
            if !candidate.exists() {
                out_path = candidate;
                break;
            }
        }
    }

    // 3) 写包
    let file = std::fs::File::create(&out_path)?;
    // 中途失败（磁盘满 / 某个文件读失败）时不能留一个打不开的半截 zip
    let mut cleanup = RemoveOnFailure {
        path: &out_path,
        keep: false,
    };
    let mut zip = zip::ZipWriter::new(file);
    let stored = zip::write::FileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .unix_permissions(0o644);
    let deflated = zip::write::FileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o644);

    // 先把文件写进包，再按"真的写进去了哪些"生成清单 ——
    // 否则清单会比包多列几项（读失败被跳过的文件）。
    let mut written: Vec<(String, u64)> = Vec::with_capacity(collected.len());
    let mut bytes = 0u64;
    for item in &collected {
        // 先打开再建条目：文件被占用时直接跳过，不会在包里留半截内容
        let Ok(mut source) = std::fs::File::open(&item.absolute) else {
            continue;
        };
        // FileOptions 是 Copy：直接按值传，别 clone
        let options = if already_compressed(&item.relative) {
            stored
        } else {
            deflated
        };
        zip.start_file(format!("overrides/{}", item.relative), options)
            .map_err(|e| Error::msg(format!("写入 {} 失败：{e}", item.relative)))?;
        let copied = std::io::copy(&mut source, &mut zip)?;
        bytes += copied;
        written.push((item.relative.clone(), copied));
    }

    let manifest = serde_json::json!({
        "name": display,
        "launcher": format!("MCL {}", env!("CARGO_PKG_VERSION")),
        "exportedAt": stamp,
        "gameVersion": game_version,
        "files": written
            .iter()
            .map(|(path, size)| serde_json::json!({ "path": path, "size": size }))
            .collect::<Vec<_>>(),
    });
    zip.start_file("mcl-manifest.json", deflated)
        .map_err(|e| Error::msg(format!("写入清单失败：{e}")))?;
    zip.write_all(
        serde_json::to_string_pretty(&manifest)
            .map_err(|e| Error::msg(e.to_string()))?
            .as_bytes(),
    )?;

    zip.finish()
        .map_err(|e| Error::msg(format!("收尾写入失败：{e}")))?;
    // 走到这里包是完整的，别让 guard 把它删掉
    cleanup.keep = true;

    Ok(ExportReport {
        path: out_path.to_string_lossy().to_string(),
        files: written.len(),
        bytes,
    })
}

/// 导出中途失败时删掉半截的 zip。
struct RemoveOnFailure<'a> {
    path: &'a Path,
    keep: bool,
}

impl Drop for RemoveOnFailure<'_> {
    fn drop(&mut self) {
        if !self.keep {
            let _ = std::fs::remove_file(self.path);
        }
    }
}

/// 去掉文件名里不能出现的字符。
/// 从版本 json 推断基础游戏版本：有 `inheritsFrom` 就用它，否则用自身 id。
///
/// 前端不需要知道这件事 —— 它只关心"导出哪个版本"。
fn detect_game_version(paths: &Paths, id: &str) -> String {
    let path = paths.version_dir(id).join(format!("{id}.json"));
    if let Ok(text) = std::fs::read_to_string(&path) {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
            if let Some(base) = value.get("inheritsFrom").and_then(|v| v.as_str()) {
                if !base.trim().is_empty() {
                    return base.to_string();
                }
            }
        }
    }
    id.to_string()
}

fn safe_name(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '_' | ' ' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.').to_string();
    if trimmed.is_empty() {
        "MCL-instance".to_string()
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn noisy_files_are_excluded() {
        for name in ["latest.log", "debug.log.gz", "level.dat_old", "log4j2.xml", "usercache.json"] {
            assert!(is_noise(name), "{name} 应被排除");
        }
        for name in ["sodium.jar", "options.txt", "config.json", "level.dat"] {
            assert!(!is_noise(name), "{name} 不该被排除");
        }
    }

    #[test]
    fn top_level_follows_checkboxes() {
        let mut options = ExportOptions::default();
        assert!(wanted("mods", &options));
        assert!(wanted("config", &options));
        // 存档默认不带
        assert!(!wanted("saves", &options));
        assert!(!wanted("resourcepacks", &options));
        // 不在白名单里的目录永远不导出
        assert!(!wanted("logs", &options));

        options.include_saves = true;
        assert!(wanted("saves", &options));
    }

    #[test]
    fn safe_name_strips_path_separators() {
        assert_eq!(safe_name("我的整合包 v1.2"), "我的整合包 v1.2");
        assert_eq!(safe_name("a/b\\c:d"), "a_b_c_d");
        assert_eq!(safe_name("   "), "MCL-instance");
        assert_eq!(safe_name(".."), "MCL-instance");
    }

    /// mod 的 jar 已经压缩过，必须原样打包（Stored）；配置文本仍然要压。
    /// 这条断言守住的是"别再把整个实例 deflate 一遍"这个性能改动。
    #[test]
    fn jars_are_stored_while_configs_are_deflated() {
        let root = std::env::temp_dir().join(format!(
            "mcl-export-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let game = root.join("versions/demo/game");
        std::fs::create_dir_all(game.join("mods")).unwrap();
        std::fs::create_dir_all(game.join("config")).unwrap();

        // 不可压的"jar"：反复的 deflate 只会白烧 CPU
        let jar: Vec<u8> = (0..256 * 1024u32)
            .map(|i| (i.wrapping_mul(2654435761) >> 13) as u8)
            .collect();
        std::fs::write(game.join("mods/sodium.jar"), &jar).unwrap();
        // 可压的配置文本
        let config = "[video]\n".repeat(4000);
        std::fs::write(game.join("config/sodium.json"), &config).unwrap();
        std::fs::write(game.join("options.txt"), "fov:0.5\n").unwrap();

        let paths = Paths { root: root.clone() };
        let options = ExportOptions {
            name: "demo".to_string(),
            include_mods: true,
            include_config: true,
            ..Default::default()
        };
        let report = export(&paths, &options, "demo").unwrap();
        assert_eq!(report.files, 3, "mods + config + options.txt 都该进包");

        let file = std::fs::File::open(&report.path).unwrap();
        let mut zip = zip::ZipArchive::new(file).unwrap();
        // jar 已经是压缩包，必须 Stored：size == compressed_size
        {
            let name = "overrides/mods/sodium.jar";
            let entry = zip.by_name(name).unwrap();
            assert_eq!(entry.size(), entry.compressed_size(), "{name} 应该原样存储");
        }
        {
            let mut entry = zip.by_name("overrides/config/sodium.json").unwrap();
            assert!(
                entry.compressed_size() < entry.size(),
                "配置文本应该被压缩"
            );
            let mut text = String::new();
            entry.read_to_string(&mut text).unwrap();
            assert_eq!(text, config);
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}

