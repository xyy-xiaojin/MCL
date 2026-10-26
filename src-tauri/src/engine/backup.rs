//! 存档备份：`versions/<id>/game/saves` ↔ `backups/<id>/<时间戳>.zip`。
//!
//! 设计原则（对齐"简洁高性能"的定位）：
//! - **手动入口只有一个按钮**：版本卡上点「备份」，弹层里列出历史、可还原；
//! - **删除版本前自动备份**：玩家点过确认了，但多一层后悔药总能救回手滑；
//! - 备份是纯 zip（Stored 压缩对已压缩存档没增益，直接压缩比 Deflated 快得多），
//!   保留最近 [`KEEP`] 份，多了自动清——不让备份目录变成第二个存档黑洞；
//! - 还原前先把**当前**存档备一份：还原也是一次写操作，翻车了还能再还原回来。

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::{Error, Result};
use crate::state::Paths;

/// 每个版本保留的备份份数。
const KEEP: usize = 10;

/// 一份备份的元信息（前端展示用）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupInfo {
    /// zip 文件名（还原/删除时的标识）
    pub file: String,
    /// 备份时刻（unix 毫秒）
    pub at: i64,
    /// zip 体积（字节）
    pub bytes: u64,
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn version_backups_dir(paths: &Paths, version_id: &str) -> Result<PathBuf> {
    let id = sanitize_id(version_id)?;
    Ok(paths.backups().join(id))
}

/// 版本 id 只允许安全字符：它要拼进路径，防目录穿越。
fn sanitize_id(id: &str) -> Result<&str> {
    let id = id.trim();
    if id.is_empty()
        || id.contains("..")
        || id.contains('/')
        || id.contains('\\')
        || id.contains(':')
    {
        return Err(Error::msg(format!("版本名不合法：{id}")));
    }
    Ok(id)
}

fn zip_name_from_millis(millis: i64) -> String {
    format!("saves-{millis}.zip")
}

fn millis_from_zip_name(name: &str) -> Option<i64> {
    let body = name.strip_prefix("saves-")?.strip_suffix(".zip")?;
    // 冲突名形如 saves-<ms>-<n>.zip：时间戳取第一个 '-' 之前
    body.split('-').next()?.parse().ok()
}

/// 把一个目录递归打成 zip。空目录返回 `Ok(None)`（没东西可备）。
fn zip_dir(source: &Path, target: &Path) -> Result<Option<u64>> {
    let mut files: Vec<PathBuf> = Vec::new();
    collect_files(source, source, &mut files, 0)?;
    if files.is_empty() {
        return Ok(None);
    }
    let file = std::fs::File::create(target)?;
    let mut zip = zip::ZipWriter::new(file);
    // 存档里多是已压缩的区域文件（Anvil），Deflated 收益趋近于零，
    // Stored 拷贝更快、还原也更快
    let stored = zip::write::FileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .unix_permissions(0o644);
    let mut total = 0u64;
    for path in &files {
        let relative = path
            .strip_prefix(source)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        let mut source_file = std::fs::File::open(path)?;
        zip.start_file(&relative, stored)
            .map_err(|e| Error::msg(format!("写备份失败：{e}")))?;
        let copied = std::io::copy(&mut source_file, &mut zip)?;
        total += copied;
    }
    zip.finish()
        .map_err(|e| Error::msg(format!("备份收尾失败：{e}")))?;
    Ok(Some(total))
}

/// 收集要备份的文件。深度有上限防异常目录；跳过符号链接防环。
/// 数量超上限**报错**而不是静默截断——一份"看起来成功"的半截备份，
/// 玩家还原时才发现存档丢了一半，比备份失败更糟。
fn collect_files(root: &Path, dir: &Path, out: &mut Vec<PathBuf>, depth: usize) -> Result<()> {
    const MAX_DEPTH: usize = 16;
    const MAX_FILES: usize = 60_000;
    if depth > MAX_DEPTH {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        // file_type 不跟随链接：指向目录的 junction/symlink 不会被当目录递归
        let Ok(file_type) = entry.file_type() else { continue };
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            collect_files(root, &path, out, depth + 1)?;
        } else if file_type.is_file() {
            out.push(path);
            if out.len() > MAX_FILES {
                return Err(Error::msg(format!(
                    "存档文件超过 {} 个，备份中止。存档目录可能指错了地方",
                    MAX_FILES
                )));
            }
        }
    }
    Ok(())
}

/// 手动/自动创建一份备份。没有存档（空目录/不存在）时返回 `Ok(None)`，
/// 调用方据此给出"这个版本没有存档"的提示而不是报错。
pub fn create(paths: &Paths, version_id: &str) -> Result<Option<BackupInfo>> {
    create_impl(paths, version_id, true)
}

/// `prune = false` 给「还原前快照」用：还原目标可能是较旧的备份，
/// 快照后立刻 prune 会把总数压回上限、可能恰好删掉正要还原的那份。
/// 快照超一份上限无害，下次正常备份时会收敛。
fn create_impl(paths: &Paths, version_id: &str, prune: bool) -> Result<Option<BackupInfo>> {
    let saves = paths.saves(version_id);
    if !saves.is_dir() {
        return Ok(None);
    }
    let dir = version_backups_dir(paths, version_id)?;
    std::fs::create_dir_all(&dir)?;

    let millis = now_millis();
    let mut name = zip_name_from_millis(millis);
    // 同一毫秒连点两次：加序号防覆盖
    if dir.join(&name).exists() {
        for index in 2..1000u32 {
            let candidate = format!("saves-{millis}-{index}.zip");
            if !dir.join(&candidate).exists() {
                name = candidate;
                break;
            }
        }
    }
    let target = dir.join(&name);
    let temp = dir.join(format!("{name}.tmp"));
    let zipped = match zip_dir(&saves, &temp) {
        Ok(zipped) => zipped,
        Err(err) => {
            let _ = std::fs::remove_file(&temp);
            return Err(err);
        }
    };
    let Some(bytes) = zipped else {
        let _ = std::fs::remove_file(&temp);
        return Ok(None);
    };
    std::fs::rename(&temp, &target)?;
    if prune {
        prune_backups(&dir);
    }

    Ok(Some(BackupInfo {
        file: name,
        at: millis,
        bytes,
    }))
}

/// 只保留最近 [`KEEP`] 份，其余删除。
fn prune_backups(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut backups: Vec<(i64, PathBuf)> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            millis_from_zip_name(&name).map(|m| (m, e.path()))
        })
        .collect();
    if backups.len() <= KEEP {
        return;
    }
    backups.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, path) in backups.iter().skip(KEEP) {
        let _ = std::fs::remove_file(path);
    }
}

/// 列出某版本的全部备份，新的在前。
pub fn list(paths: &Paths, version_id: &str) -> Result<Vec<BackupInfo>> {
    let dir = version_backups_dir(paths, version_id)?;
    let mut out: Vec<BackupInfo> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            let at = millis_from_zip_name(&name)?;
            let bytes = entry.metadata().ok()?.len();
            Some(BackupInfo { file: name, at, bytes })
        })
        .collect();
    out.sort_by(|a, b| b.at.cmp(&a.at));
    Ok(out)
}

/// 还原一份备份。
///
/// 还原前把当前存档自动备成一份"还原前快照"——玩家还原错了还能再还原回来。
/// 当前没有存档时跳过快照直接解压。
pub fn restore(paths: &Paths, version_id: &str, file: &str) -> Result<()> {
    let dir = version_backups_dir(paths, version_id)?;
    // file 是我们列出来的文件名，仍然校验一下，不接受路径分隔符
    sanitize_id(file)?;
    let zip_path = dir.join(file);
    // 先把目标 zip 打开握在手里再做快照：快照会让总数超过保留上限触发
    // prune，如果目标恰好是最旧的，晚一步打开它就被自己人删了
    let zip_handle = std::fs::File::open(&zip_path)
        .map_err(|_| Error::msg("这份备份不存在，可能已被删除"))?;

    let saves = paths.saves(version_id);
    if saves.is_dir() && std::fs::read_dir(&saves).map(|mut d| d.next().is_some()).unwrap_or(false)
    {
        create_impl(paths, version_id, false)?;
    }

    // 解压到暂存目录，成功后整体替换，中途失败不伤现有存档。
    // 目录名带原子计数：同一毫秒并发还原不会共用一个暂存目录互删。
    // 顺带清扫上次失败/被杀进程留下的旧暂存目录。
    let parent = saves
        .parent()
        .ok_or_else(|| Error::msg("存档目录没有父目录"))?;
    std::fs::create_dir_all(parent)?;
    static RESTORE_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let seq = RESTORE_SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    if let Ok(entries) = std::fs::read_dir(parent) {
        for entry in entries.flatten() {
            if entry
                .file_name()
                .to_string_lossy()
                .starts_with(".saves-restore-")
            {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
    let staged = parent.join(format!(".saves-restore-{seq}-{}", now_millis()));
    let _ = std::fs::remove_dir_all(&staged);
    std::fs::create_dir_all(&staged)?;
    drop(zip_handle);

    let result = extract_zip(&zip_path, &staged);
    if let Err(err) = result {
        let _ = std::fs::remove_dir_all(&staged);
        return Err(err);
    }
    if saves.exists() {
        std::fs::remove_dir_all(&saves)?;
    }
    std::fs::rename(&staged, &saves)?;
    Ok(())
}

/// 解压备份 zip。所有条目都必须落在目标目录内（enclosed_name 拒绝 `../`）。
fn extract_zip(zip_path: &Path, target: &Path) -> Result<()> {
    let file = std::fs::File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(file)?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        if entry.is_dir() {
            continue;
        }
        let Some(relative) = entry.enclosed_name() else {
            continue;
        };
        let out = target.join(relative);
        if let Some(dir) = out.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut bytes = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut bytes)?;
        let mut dest = std::fs::File::create(&out)?;
        dest.write_all(&bytes)?;
    }
    Ok(())
}

/// 删除一份备份。
pub fn remove(paths: &Paths, version_id: &str, file: &str) -> Result<()> {
    let dir = version_backups_dir(paths, version_id)?;
    sanitize_id(file)?;
    let zip_path = dir.join(file);
    if !zip_path.is_file() {
        return Err(Error::msg("这份备份不存在"));
    }
    std::fs::remove_file(&zip_path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> (Paths, PathBuf) {
        let root = std::env::temp_dir().join(format!("mcl-backup-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let paths = Paths { root: root.clone() };
        (paths, root)
    }

    fn make_saves(paths: &Paths, version: &str, marker: &str) {
        let saves = paths.saves(version);
        std::fs::create_dir_all(saves.join("world/datapacks")).unwrap();
        std::fs::write(saves.join("world/level.dat"), marker.as_bytes()).unwrap();
        std::fs::write(
            saves.join("world/datapacks/test.json"),
            format!("{{\"marker\":\"{marker}\"}}").as_bytes(),
        )
        .unwrap();
    }

    #[test]
    fn backup_list_restore_roundtrip() {
        let (paths, root) = scratch("roundtrip");
        make_saves(&paths, "1.20.1", "first");

        let first = create(&paths, "1.20.1").unwrap().expect("第一份备份");
        // 内容改掉再备第二份
        make_saves(&paths, "1.20.1", "second");
        std::thread::sleep(std::time::Duration::from_millis(5));
        let second = create(&paths, "1.20.1").unwrap().expect("第二份备份");

        let history = list(&paths, "1.20.1").unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].file, second.file, "新的在前");

        // 还原到第一份
        restore(&paths, "1.20.1", &first.file).unwrap();
        let restored = std::fs::read(paths.saves("1.20.1").join("world/level.dat")).unwrap();
        assert_eq!(restored, b"first");
        // 还原前做了当前存档的快照
        assert_eq!(list(&paths, "1.20.1").unwrap().len(), 3);

        // 删一份
        let victim = list(&paths, "1.20.1").unwrap().remove(0);
        remove(&paths, "1.20.1", &victim.file).unwrap();
        assert_eq!(list(&paths, "1.20.1").unwrap().len(), 2);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn no_saves_means_no_backup() {
        let (paths, root) = scratch("empty");
        assert!(create(&paths, "1.20.1").unwrap().is_none());
        assert!(list(&paths, "1.20.1").unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn prune_keeps_recent_ten() {
        let (paths, root) = scratch("prune");
        make_saves(&paths, "v", "x");
        for index in 0..13i64 {
            let dir = version_backups_dir(&paths, "v").unwrap();
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(zip_name_from_millis(index)), b"fake").unwrap();
        }
        prune_backups(&version_backups_dir(&paths, "v").unwrap());
        let left = list(&paths, "v").unwrap();
        assert_eq!(left.len(), KEEP);
        // 留下的是时间戳最大的那些
        assert_eq!(left.last().unwrap().at, 3, "最小时间戳应为 3");
        assert_eq!(left.first().unwrap().at, 12);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn bad_version_ids_are_rejected() {
        let (paths, root) = scratch("sanitize");
        assert!(version_backups_dir(&paths, "../evil").is_err());
        assert!(version_backups_dir(&paths, "a/b").is_err());
        assert!(version_backups_dir(&paths, "  ").is_err());
        let _ = std::fs::remove_dir_all(&root);
    }
}
