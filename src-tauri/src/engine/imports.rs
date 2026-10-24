//! 拖拽导入：判断拖进来的文件是什么，放进对应目录。
//!
//! 判断依据是**文件内容**而不是扩展名：材质包、光影包、整合包都可能叫 `.zip`，
//! 只看后缀会把光影包塞进 mods。判定顺序（先命中先返回）：
//!
//! | 包内特征 | 归类 |
//! |---|---|
//! | `modrinth.index.json` | 整合包 |
//! | `shaders/` 下的文件 | 光影包 |
//! | `pack.mcmeta` | 材质包 |
//! | `fabric.mod.json` / `META-INF/mods.toml` / `mcmod.info` | Mod |
//! | 以上都没有的 `.zip` | 当整合包（交给 mrpack/普通 zip 流程） |
//!
//! `.png` 一律按皮肤处理，落盘前校验尺寸（64×64 / 64×32 及其整数倍），
//! 免得把一张截图塞进 `mods/`。
//!
//! 重名不覆盖：`sodium.jar` 已存在时新文件叫 `sodium-1.jar`。
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::engine::{modpack, skin};
use crate::error::Result;
use crate::state::AppState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Modpack,
    Mod,
    Resourcepack,
    Shaderpack,
    /// 离线皮肤（PNG），不落到任何版本目录，直接成为玩家头像
    Skin,
}

impl FileKind {
    pub fn as_str(self) -> &'static str {
        match self {
            FileKind::Modpack => "modpack",
            FileKind::Mod => "mod",
            FileKind::Resourcepack => "resourcepack",
            FileKind::Shaderpack => "shaderpack",
            FileKind::Skin => "skin",
        }
    }
}

/// 一次拖入里单个文件的处理结果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportOutcome {
    /// 原始文件名
    pub file: String,
    pub kind: String,
    pub ok: bool,
    /// 成功时是落地目录，失败时是原因
    pub message: String,
    /// 整合包新建出来的版本 id —— 前端据此切到新版本
    pub version_id: Option<String>,
}

/// 看压缩包内部结构判断类型。打不开（不是 zip）就返回 None。
fn inspect_archive(path: &Path) -> Option<FileKind> {
    let file = std::fs::File::open(path).ok()?;
    let zip = zip::ZipArchive::new(file).ok()?;
    let names: Vec<String> = zip.file_names().map(|n| n.to_ascii_lowercase()).collect();

    let any = |pred: fn(&str) -> bool| names.iter().any(|n| pred(n));

    if any(|n| n == "modrinth.index.json") {
        return Some(FileKind::Modpack);
    }
    // 光影包没有 pack.mcmeta，标志就是 shaders/ 目录
    if any(|n| n.starts_with("shaders/") && !n.ends_with('/')) {
        return Some(FileKind::Shaderpack);
    }
    if any(|n| n == "pack.mcmeta") {
        return Some(FileKind::Resourcepack);
    }
    if any(|n| {
        n == "fabric.mod.json"
            || n.ends_with("/fabric.mod.json")
            || n == "meta-inf/mods.toml"
            || n == "meta-inf/neoforge.mods.toml"
            || n == "mcmod.info"
    }) {
        return Some(FileKind::Mod);
    }
    None
}

/// 判断一个文件该怎么处理。
pub fn classify(path: &Path) -> FileKind {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    match ext.as_str() {
        "mrpack" => FileKind::Modpack,
        "litemod" => FileKind::Mod,
        // png 一律按皮肤处理，交给 `import_local_skin` 校验尺寸：
        // 这样拖错图会得到"尺寸 1920×1080 不是皮肤"，而不是把截图塞进 mods/
        "png" => FileKind::Skin,
        // jar 与 zip 都要看内容：光影包常是 zip，材质包也可能被改名成 jar
        "jar" | "zip" => inspect_archive(path).unwrap_or_else(|| {
            if ext == "zip" {
                FileKind::Modpack
            } else {
                // 读不出结构的 jar 按 mod 处理，玩家最可能是这个意图
                FileKind::Mod
            }
        }),
        _ => FileKind::Mod,
    }
}

/// 复制到目标目录，重名时加 `-1`、`-2` 后缀，绝不覆盖已有文件。
fn copy_into(src: &Path, dir: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let name = src
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "unnamed".to_string());

    let mut target = dir.join(&name);
    if target.exists() {
        let path = Path::new(&name);
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| name.clone());
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_string())
            .unwrap_or_default();
        for index in 1..1000u32 {
            let candidate = if ext.is_empty() {
                format!("{stem}-{index}")
            } else {
                format!("{stem}-{index}.{ext}")
            };
            target = dir.join(candidate);
            if !target.exists() {
                break;
            }
        }
        // 重名号用到 999 还是没找到空位：宁可报错也不能覆盖（见函数契约）
        if target.exists() {
            return Err(crate::error::Error::msg(format!(
                "「{name}」的重名号已用尽，请先清理 {} 下的旧文件",
                dir.display()
            )));
        }
    }

    std::fs::copy(src, &target)?;
    Ok(target)
}

/// 处理一批拖进来的路径。单个失败不影响其它文件。
///
/// `target_version` 是当前选中的版本 id：mod / 材质包 / 光影包都会装进
/// `versions/<id>/game/` 下按版本隔离的目录。整合包例外 —— 它会新建一个
/// 独立版本，不用这个参数。
pub async fn import_files(
    state: &AppState,
    paths: &[String],
    target_version: &str,
) -> Vec<ImportOutcome> {
    let mut out = Vec::with_capacity(paths.len());
    for raw in paths {
        let path = PathBuf::from(raw);
        let file = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| raw.clone());

        if !path.exists() {
            out.push(failed(file, "unknown", "文件不存在"));
            continue;
        }
        // 暂时只处理文件：拖文件夹进来的语义太模糊（是整合包还是存档？）
        if path.is_dir() {
            out.push(failed(file, "unknown", "暂不支持文件夹，请拖入压缩包"));
            continue;
        }

        let kind = classify(&path);
        if kind == FileKind::Modpack {
            // 整合包会新建一个独立版本；普通 zip 用当前版本当基底
            match modpack::import_any(state, path.to_string_lossy().as_ref(), target_version).await
            {
                Ok(report) => {
                    let skipped = if report.skipped > 0 {
                        format!("，跳过 {} 个已存在文件", report.skipped)
                    } else {
                        String::new()
                    };
                    let note = if report.note.is_empty() {
                        String::new()
                    } else {
                        format!("；{}", report.note)
                    };
                    out.push(imported(
                        file,
                        kind.as_str(),
                        format!(
                            "已新建版本「{}」（{} + {}），解压 {} 个文件{skipped}{note}",
                            report.version_id, report.game_version, report.loader, report.overrides
                        ),
                        Some(report.version_id),
                    ));
                }
                Err(e) => out.push(failed(file, kind.as_str(), e.to_string())),
            }
            continue;
        }

        // 皮肤和版本无关：直接成为当前玩家的头像，不落进 versions/
        if kind == FileKind::Skin {
            match skin::import_local_skin(state, &path) {
                Ok(_) => out.push(imported(
                    file.clone(),
                    kind.as_str(),
                    "已设为离线皮肤，头像立即生效".to_string(),
                    None,
                )),
                Err(e) => out.push(failed(file, kind.as_str(), e.to_string())),
            }
            continue;
        }

        // 非整合包要落到某个版本的私有目录里，没有目标版本就无处可放
        if target_version.trim().is_empty() {
            out.push(failed(
                file,
                kind.as_str(),
                "请先在「版本管理」里选择一个版本",
            ));
            continue;
        }
        if !state.paths.version_dir(target_version).is_dir() {
            out.push(failed(
                file,
                kind.as_str(),
                format!("版本 {target_version} 不存在，请重新选择"),
            ));
            continue;
        }

        let dir = match kind {
            FileKind::Mod => state.paths.mods(target_version),
            FileKind::Resourcepack => state.paths.resourcepacks(target_version),
            FileKind::Shaderpack => state.paths.shaderpacks(target_version),
            FileKind::Modpack | FileKind::Skin => unreachable!("上面已经处理"),
        };
        match copy_into(&path, &dir) {
            Ok(target) => out.push(imported(
                file,
                kind.as_str(),
                format!(
                    "已放入 {}",
                    target
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default()
                ),
                None,
            )),
            Err(e) => out.push(failed(file, kind.as_str(), e.to_string())),
        }
    }
    out
}

fn failed(file: String, kind: &str, message: impl Into<String>) -> ImportOutcome {
    ImportOutcome {
        file,
        kind: kind.to_string(),
        ok: false,
        message: message.into(),
        version_id: None,
    }
}

fn imported(
    file: String,
    kind: &str,
    message: String,
    version_id: Option<String>,
) -> ImportOutcome {
    ImportOutcome {
        file,
        kind: kind.to_string(),
        ok: true,
        message,
        version_id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn make_zip(name: &str, entries: &[(&str, &str)]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mcl-imports-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        let file = std::fs::File::create(&path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::FileOptions::default();
        for (entry, body) in entries {
            zip.start_file(*entry, opts).unwrap();
            zip.write_all(body.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
        path
    }

    #[test]
    fn resourcepack_is_detected_by_pack_mcmeta() {
        let path = make_zip("pack.zip", &[("pack.mcmeta", "{}"), ("assets/minecraft/x.png", "")]);
        assert_eq!(classify(&path), FileKind::Resourcepack);
    }

    #[test]
    fn shaderpack_is_detected_by_shaders_dir() {
        let path = make_zip("bsl.zip", &[("shaders/final.fsh", "void main(){}")]);
        assert_eq!(classify(&path), FileKind::Shaderpack);
    }

    #[test]
    fn mod_is_detected_by_fabric_json() {
        let path = make_zip(
            "sodium.jar",
            &[("fabric.mod.json", "{}"), ("sodium.class", "")],
        );
        assert_eq!(classify(&path), FileKind::Mod);
    }

    #[test]
    fn mrpack_wins_over_other_markers() {
        let path = make_zip(
            "pack.mrpack",
            &[("modrinth.index.json", "{}"), ("overrides/mods/a.jar", "")],
        );
        assert_eq!(classify(&path), FileKind::Modpack);
    }

    #[test]
    fn unknown_zip_falls_back_to_modpack() {
        let path = make_zip("bundle.zip", &[("overrides/mods/a.jar", "")]);
        assert_eq!(classify(&path), FileKind::Modpack);
    }

    /// 重名时加后缀，不能覆盖玩家已经装好的 mod
    #[test]
    fn copy_never_overwrites() {
        let dir = std::env::temp_dir().join(format!("mcl-copy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let src = dir.join("sodium.jar");
        std::fs::write(&src, b"new").unwrap();
        let dest = dir.join("mods");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("sodium.jar"), b"old").unwrap();

        let target = copy_into(&src, &dest).unwrap();
        assert_eq!(target.file_name().unwrap(), "sodium-1.jar");
        // 原文件内容不能被改掉
        assert_eq!(std::fs::read(dest.join("sodium.jar")).unwrap(), b"old");
        assert_eq!(std::fs::read(&target).unwrap(), b"new");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
