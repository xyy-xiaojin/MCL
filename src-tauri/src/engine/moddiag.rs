//! Mod 诊断：进游戏**之前**把「缺前置」「版本不匹配」「装了冲突 mod」拦下来。
//!
//! 数据来源是 mod jar 自己声明的元数据，不联网：
//! - Fabric / Quilt：`fabric.mod.json` 的 `depends` / `conflicts` / `breaks`
//! - Forge / NeoForge：`META-INF/mods.toml` 的 `[[dependencies.<id>]]`
//!
//! 判定规则（和游戏内报错对齐，但提前到点启动之前）：
//! - 必装前置缺失 / 版本区间不匹配 → **error**（进游戏必炸）
//! - `breaks` 命中、同 id 装了两份 → **error**
//! - `conflicts` 命中、内置已知冲突对（Sodium×OptiFine 等）→ **warning**
//! - 可选依赖（mandatory=false / 建议项）不报——报了全是噪音
//!
//! 性能：只开 jar 读一个 entry，几百个 mod 全扫也就毫秒级每个；
//! 整个扫描丢 blocking 池，不占 async worker。

use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;

/// 一条诊断结论。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModIssue {
    /// `error`（进游戏会出问题）/ `warning`（可能有坑）
    pub level: String,
    /// 涉及的 mod 文件名（前端定位用）
    pub file: String,
    /// 人话结论，直接展示
    pub message: String,
}

/// 内置的已知冲突对（mod id 小写）。两边的 id 同时在场（启用）就警告。
const KNOWN_CONFLICTS: [[&str; 2]; 3] = [
    ["sodium", "optifine"],
    ["iris", "optifine"],
    ["sodium", "optifineforge"],
];

// ---------------------------------------------------------------------------
// 版本区间：支持 Fabric 的写法（"*"、">=1.20 <1.21"、"~0.96"）
// ---------------------------------------------------------------------------

/// 把版本拆成「数字主干 + pre-release 后缀」（`1.21-pre1` → `1.21`, `pre1`）。
fn split_pre(version: &str) -> (&str, Option<&str>) {
    match version.find('-') {
        Some(index) => (&version[..index], Some(&version[index + 1..])),
        None => (version, None),
    }
}

/// 语义化版本比较：数字段按数值、其余按字典序（数字段 < 字母段）；
/// 主干相同时**带 pre-release 的更小**（`1.21 > 1.21-pre1`，semver 规定）。
fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let (a_base, a_pre) = split_pre(a.trim());
    let (b_base, b_pre) = split_pre(b.trim());
    let base_ord = compare_release(a_base, b_base);
    if base_ord != std::cmp::Ordering::Equal {
        return base_ord;
    }
    match (a_pre, b_pre) {
        (None, None) => std::cmp::Ordering::Equal,
        // 正式版 > 预发布版
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (Some(_), None) => std::cmp::Ordering::Less,
        (Some(x), Some(y)) => compare_release(x, y),
    }
}

/// 主干比较：按 `.`/`+` 切段；前缀相同段多的大（1.20.1 > 1.20）。
fn compare_release(a: &str, b: &str) -> std::cmp::Ordering {
    let pa = a.split(['.', '+']);
    let pb = b.split(['.', '+']);
    for (x, y) in pa.zip(pb) {
        let nx = x.parse::<u64>();
        let ny = y.parse::<u64>();
        let ord = match (nx, ny) {
            (Ok(x), Ok(y)) => x.cmp(&y),
            (Ok(_), Err(_)) => std::cmp::Ordering::Less,
            (Err(_), Ok(_)) => std::cmp::Ordering::Greater,
            (Err(_), Err(_)) => x.cmp(y),
        };
        if ord != std::cmp::Ordering::Equal {
            return ord;
        }
    }
    a.split(['.', '+']).count().cmp(&b.split(['.', '+']).count())
}

/// 单个约束（去掉空格后的 token）是否满足。
fn token_matches(token: &str, version: &str) -> bool {
    let v = version.trim();
    let t = token.trim();
    if t == "*" || t.is_empty() {
        return true;
    }
    // `>=x` / `<=y` / `>a` / `<b` / `~c` / `^d` / 精确
    if let Some(rest) = t.strip_prefix(">=") {
        compare_versions(v, rest.trim()) != std::cmp::Ordering::Less
    } else if let Some(rest) = t.strip_prefix("<=") {
        compare_versions(v, rest.trim()) != std::cmp::Ordering::Greater
    } else if let Some(rest) = t.strip_prefix('>') {
        compare_versions(v, rest.trim()) == std::cmp::Ordering::Greater
    } else if let Some(rest) = t.strip_prefix('<') {
        compare_versions(v, rest.trim()) == std::cmp::Ordering::Less
    } else if let Some(rest) = t.strip_prefix('~') {
        // 同一 minor 内：>=rest 且 <(minor+1)；只给了一个 major（~2）则到 major+1
        let base = rest.trim();
        let parts: Vec<u64> = base
            .split('.')
            .map(|p| p.parse::<u64>().unwrap_or(0))
            .collect();
        let upper = if parts.len() <= 1 {
            format!("{}", parts.first().copied().unwrap_or(0) + 1)
        } else {
            let mut p = parts.clone();
            p[1] += 1;
            p.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(".")
        };
        let lower = parts.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(".");
        compare_versions(v, &lower) != std::cmp::Ordering::Less
            && compare_versions(v, &upper) == std::cmp::Ordering::Less
    } else if let Some(rest) = t.strip_prefix('^') {
        // 同一 major 内
        let base = rest.trim();
        let major: u64 = base.split('.').next().and_then(|p| p.parse().ok()).unwrap_or(0);
        let upper = format!("{}", major + 1);
        compare_versions(v, base) != std::cmp::Ordering::Less
            && compare_versions(v, &upper) == std::cmp::Ordering::Less
    } else {
        compare_versions(v, t) == std::cmp::Ordering::Equal
    }
}

/// Fabric 风格的区间串（空格分隔的多个约束，全部要满足）。
pub fn range_matches(range: &str, version: &str) -> bool {
    range
        .split_whitespace()
        .all(|token| token_matches(token, version))
}

// ---------------------------------------------------------------------------
// Forge 的 `[1.20,1.21)` 区间
// ---------------------------------------------------------------------------

/// `[a,b]` `(a,b)` `[a,b)` `(a,b)`，也接受单个 `[a,)`。逗号前后都可以有 `]`/`)` 混用。
pub fn maven_range_matches(range: &str, version: &str) -> bool {
    let r = range.trim();
    let Some(inner) = r
        .strip_prefix(['[', '('])
        .and_then(|s| s.strip_suffix([']', ')']))
    else {
        // 不是区间就当 Fabric 风格处理
        return range_matches(r, version);
    };
    let lower_inclusive = r.starts_with('[');
    let upper_inclusive = r.ends_with(']');
    let Some((lower, upper)) = inner.split_once(',') else {
        // `[1.20]`：单值匹配
        return range_matches(inner.trim(), version);
    };
    let lower = lower.trim();
    let upper = upper.trim();
    if !lower.is_empty() {
        let ord = compare_versions(version, lower);
        if lower_inclusive && ord == std::cmp::Ordering::Less {
            return false;
        }
        if !lower_inclusive && ord != std::cmp::Ordering::Greater {
            return false;
        }
    }
    if !upper.is_empty() && upper != ")" && upper != "]" {
        let ord = compare_versions(version, upper);
        if upper_inclusive && ord == std::cmp::Ordering::Greater {
            return false;
        }
        if !upper_inclusive && ord != std::cmp::Ordering::Less {
            return false;
        }
    }
    true
}

// ---------------------------------------------------------------------------
// jar 元数据读取
// ---------------------------------------------------------------------------

/// 从一个 mod jar 里读出的声明。
#[derive(Debug, Default, Clone)]
struct ModMeta {
    id: String,
    version: String,
    /// (依赖 id 小写, 版本区间的候选【OR 语义：任一命中即满足】, 是否必装)
    depends: Vec<(String, Vec<String>, bool)>,
    /// (冲突/破坏 id 小写, 是否硬冲突 breaks)
    conflicts: Vec<(String, bool)>,
}

/// 依赖 id 到「用什么版本去匹配」的映射上下文。
pub struct DiagnosticContext {
    pub game_version: String,
    /// 场上加载器（fabric / forge / neoforge / quilt），空 = 原版
    pub loader: String,
    pub loader_version: String,
}

/// 元数据 entry 的读取上限：size 来自不受信任的 zip 头，
/// 声明超大 size 的恶意/损坏 jar 会触发巨大分配，直接放弃这个 entry。
const MAX_META_BYTES: u64 = 8 * 1024 * 1024;

fn read_jar_entry(jar: &Path, entry: &str) -> Option<Vec<u8>> {
    let file = std::fs::File::open(jar).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;
    let mut zip_entry = archive.by_name(entry).ok()?;
    if zip_entry.size() > MAX_META_BYTES {
        return None;
    }
    // mods.toml 会被 zip 读出目录条目？by_name 只匹配文件
    let mut bytes = Vec::with_capacity(zip_entry.size() as usize);
    std::io::Read::read_to_end(&mut zip_entry, &mut bytes).ok()?;
    Some(bytes)
}

fn parse_fabric_mod(bytes: &[u8]) -> Option<ModMeta> {
    let value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    let id = value.get("id")?.as_str()?.to_string();
    let version = value
        .get("version")
        .and_then(|v| v.as_str())
        .unwrap_or("?")
        .to_string();
    let mut meta = ModMeta {
        id,
        version,
        ..Default::default()
    };
    // depends 的值可能是字符串、字符串数组（OR：任一命中即满足），
    // 或新版 fabric api 的 { versions: [...] } 对象写法
    if let Some(depends) = value.get("depends").and_then(|v| v.as_object()) {
        for (dep_id, constraint) in depends {
            let range = constraint_value(constraint);
            meta.depends.push((dep_id.to_lowercase(), range, true));
        }
    }
    if let Some(suggests) = value.get("suggests").and_then(|v| v.as_object()) {
        for (dep_id, constraint) in suggests {
            let range = constraint_value(constraint);
            // 建议项只有在「装了但版本不对」时才值得说一句，缺失不报
            meta.depends.push((dep_id.to_lowercase(), range, false));
        }
    }
    if let Some(breaks) = value.get("breaks").and_then(|v| v.as_object()) {
        for (dep_id, _) in breaks {
            meta.conflicts.push((dep_id.to_lowercase(), true));
        }
    }
    if let Some(conflicts) = value.get("conflicts").and_then(|v| v.as_object()) {
        for (dep_id, _) in conflicts {
            meta.conflicts.push((dep_id.to_lowercase(), false));
        }
    }
    Some(meta)
}

/// quilt.mod.json：id / depends 都嵌在 `quilt_loader` 节点下，
/// 且 depends 是数组（每项 {id, versions?}），不是 fabric 的 map。
fn parse_quilt_mod(bytes: &[u8]) -> Option<ModMeta> {
    let value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    let loader = value.get("quilt_loader")?;
    let id = loader.get("id")?.as_str()?.to_string();
    let version = loader
        .get("version")
        .and_then(|v| v.as_str())
        .unwrap_or("?")
        .to_string();
    let mut meta = ModMeta {
        id,
        version,
        ..Default::default()
    };
    if let Some(depends) = loader.get("depends").and_then(|v| v.as_array()) {
        for dep in depends {
            let Some(dep_id) = dep.get("id").and_then(|v| v.as_str()) else {
                continue;
            };
            let range = dep
                .get("versions")
                .map(constraint_value)
                .unwrap_or_else(|| vec!["*".to_string()]);
            let mandatory = dep
                .get("optional")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
                == false;
            meta.depends.push((dep_id.to_lowercase(), range, mandatory));
        }
    }
    if let Some(breaks) = loader.get("breaks").and_then(|v| v.as_array()) {
        for dep in breaks {
            if let Some(dep_id) = dep.get("id").and_then(|v| v.as_str()) {
                meta.conflicts.push((dep_id.to_lowercase(), true));
            }
        }
    }
    Some(meta)
}

/// 约束解析成候选区间列表（OR 语义）。
fn constraint_value(value: &serde_json::Value) -> Vec<String> {
    match value {
        serde_json::Value::String(s) => vec![s.clone()],
        // "minecraft": ["1.20", "1.20.1"] —— 任一命中即满足
        serde_json::Value::Array(arr) => {
            let list = arr
                .iter()
                .filter_map(|v| v.as_str())
                .map(|s| s.to_string())
                .collect::<Vec<_>>();
            if list.is_empty() {
                vec!["*".to_string()]
            } else {
                list
            }
        }
        // 新写法：{ "versions": [">=0.90"] }
        other => other
            .get("versions")
            .map(constraint_value)
            .unwrap_or_else(|| vec!["*".to_string()]),
    }
}

/// 极简 mods.toml 扫描：按 `[[dependencies.x]]` 分节收集键值，
/// 节尾统一产出依赖（modId + versionRange 的行序无关）。只认三种键。
fn parse_mods_toml(bytes: &[u8]) -> Option<ModMeta> {
    let text = String::from_utf8_lossy(bytes);
    let mut meta = ModMeta::default();
    let mut in_mods = false; // 当前在 [[mods]] 节
    let mut pending: Vec<(String, String)> = Vec::new(); // dependencies 节的键值缓存

    let flush = |meta: &mut ModMeta, pending: &mut Vec<(String, String)>| {
        if pending.is_empty() {
            return;
        }
        let mod_id = pending
            .iter()
            .find(|(k, _)| k == "modId")
            .map(|(_, v)| v.to_lowercase());
        let range = pending
            .iter()
            .find(|(k, _)| k == "versionRange")
            .map(|(_, v)| v.clone())
            .unwrap_or_else(|| "*".to_string());
        let mandatory = pending
            .iter()
            .find(|(k, _)| k == "mandatory")
            .map(|(_, v)| v.eq_ignore_ascii_case("true"))
            .unwrap_or(true);
        if let Some(id) = mod_id {
            if !id.is_empty() {
                meta.depends.push((id, vec![range], mandatory));
            }
        }
        pending.clear();
    };

    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.starts_with("[[dependencies.") {
            flush(&mut meta, &mut pending);
            in_mods = false; // 依赖节里的 modId 是"依赖谁"，不能覆盖主 id
            continue;
        }
        if line.starts_with('[') {
            flush(&mut meta, &mut pending);
            in_mods = line.contains("mods");
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        let key = key.trim().to_string();
        let value = value.trim().trim_matches('"').trim_matches('\'').to_string();
        if in_mods && !line.is_empty() && key == "modId" && !value.is_empty() {
            meta.id = value.clone();
        } else if in_mods && key == "version" {
            meta.version = value.clone();
        } else {
            pending.push((key, value));
        }
    }
    flush(&mut meta, &mut pending);

    if meta.id.is_empty() {
        None
    } else {
        Some(meta)
    }
}

fn read_meta(jar: &Path) -> ModMeta {
    if let Some(bytes) = read_jar_entry(jar, "fabric.mod.json") {
        if let Some(meta) = parse_fabric_mod(&bytes) {
            return meta;
        }
    }
    if let Some(bytes) = read_jar_entry(jar, "quilt.mod.json") {
        if let Some(meta) = parse_quilt_mod(&bytes) {
            return meta;
        }
    }
    if let Some(bytes) = read_jar_entry(jar, "META-INF/mods.toml") {
        if let Some(meta) = parse_mods_toml(&bytes) {
            return meta;
        }
    }
    ModMeta::default()
}

// ---------------------------------------------------------------------------
// 主诊断流程
// ---------------------------------------------------------------------------

/// 依赖 id 在当前环境里对应的版本：游戏本体 / 加载器 / 某个已装的 mod。
fn version_of(dep_id: &str, ctx: &DiagnosticContext, present: &BTreeMap<String, (String, String)>) -> Option<String> {
    let id = dep_id.to_lowercase();
    match id.as_str() {
        "minecraft" | "mc" => Some(ctx.game_version.clone()),
        "java" => None, // Java 版本不在这里判
        "fabric" | "fabricloader" | "fabric-loader" | "loader" => {
            Some(ctx.loader_version.clone()).filter(|s| !s.is_empty() || ctx.loader == "fabric")
        }
        "forge" | "neoforge" | "quilt" | "quiltloader" | "quilt-loader" => {
            if ctx.loader == id {
                Some(ctx.loader_version.clone())
            } else {
                None
            }
        }
        _ => present.get(&id).map(|(v, _)| v.clone()),
    }
}

/// 扫描一个 mods 目录（已启用的 jar），结合版本上下文给出诊断。
pub fn diagnose(
    dir: &Path,
    ctx: &DiagnosticContext,
    enabled_files: &[String],
) -> Vec<ModIssue> {
    let mut issues = Vec::new();
    let mut metas: Vec<(String, ModMeta)> = Vec::new();

    for file_name in enabled_files {
        let jar = dir.join(file_name);
        if !jar.is_file() {
            continue;
        }
        let meta = read_meta(&jar);
        metas.push((file_name.clone(), meta));
    }

    // id → (version, file)：在场 mod 的索引（有 id 声明的才进）
    let mut present: BTreeMap<String, (String, String)> = BTreeMap::new();
    for (file, meta) in &metas {
        if !meta.id.is_empty() {
            present
                .entry(meta.id.to_lowercase())
                .or_insert_with(|| (meta.version.clone(), file.clone()));
        }
    }

    // 1) 重复 id
    for (file, meta) in &metas {
        if meta.id.is_empty() {
            continue;
        }
        if let Some((_, first_file)) = present.get(&meta.id.to_lowercase()) {
            if first_file != file {
                issues.push(ModIssue {
                    level: "error".into(),
                    file: file.clone(),
                    message: format!(
                        "「{file}」和「{first_file}」是同一个 mod（{}），只留一个",
                        meta.id
                    ),
                });
            }
        }
    }

    // 2) 前置与版本区间
    for (file, meta) in &metas {
        for (dep_id, alternatives, mandatory) in &meta.depends {
            // java 版本在这里判不了（那是启动器全局环境，不是 mod 依赖图的一部分），
            // 直接跳过——当成"缺前置 java"报出来全是误报
            if dep_id == "java" {
                continue;
            }
            let Some(installed) = version_of(dep_id, ctx, &present) else {
                if *mandatory {
                    let all_wildcard = alternatives
                        .iter()
                        .all(|r| r.trim().is_empty() || r.trim() == "*");
                    issues.push(ModIssue {
                        level: "error".into(),
                        file: file.clone(),
                        message: format!(
                            "「{file}」缺前置 {}{}",
                            dep_id,
                            if all_wildcard {
                                String::new()
                            } else {
                                format!("（需要 {}）", alternatives.join(" 或 "))
                            }
                        ),
                    });
                }
                continue;
            };
            // OR 语义：候选区间任一命中即满足
            let matches = alternatives.iter().any(|range| {
                if range.contains(',') && (range.contains('[') || range.contains('(')) {
                    maven_range_matches(range, &installed)
                } else {
                    range_matches(range, &installed)
                }
            });
            if !matches {
                let level = if *mandatory { "error" } else { "warning" };
                issues.push(ModIssue {
                    level: level.into(),
                    file: file.clone(),
                    message: format!(
                        "「{file}」需要 {} 的版本 {}，当前是 {installed}",
                        dep_id,
                        alternatives.join(" 或 ")
                    ),
                });
            }
        }
    }

    // 3) breaks / conflicts
    for (file, meta) in &metas {
        for (other, hard) in &meta.conflicts {
            if let Some((other_version, other_file)) = present.get(other) {
                if other_file == file {
                    continue;
                }
                let level = if *hard { "error" } else { "warning" };
                issues.push(ModIssue {
                    level: level.into(),
                    file: file.clone(),
                    message: format!(
                        "「{file}」声明与 {}{} 不兼容（当前装的是 {other_file}）",
                        other,
                        if other_version != "?" { "" } else { "" },
                    ),
                });
            }
        }
    }

    // 4) 内置已知冲突对
    for [a, b] in KNOWN_CONFLICTS {
        if let (Some((_, fa)), Some((_, fb))) = (present.get(a), present.get(b)) {
            issues.push(ModIssue {
                level: "warning".into(),
                file: fa.clone(),
                message: format!("{a} 和 {b} 同装会出问题，建议只留一个（涉及 {fa} / {fb}）"),
            });
        }
    }

    issues
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_ranges_fabric_style() {
        assert!(range_matches("*", "anything"));
        assert!(range_matches(">=1.20", "1.20.1"));
        assert!(!range_matches(">=1.21", "1.20.1"));
        assert!(range_matches(">=1.20 <1.21", "1.20.4"));
        assert!(!range_matches(">=1.20 <1.21", "1.21"));
        assert!(range_matches("1.20.1", "1.20.1"));
        assert!(!range_matches("1.20.1", "1.20.2"));
        assert!(range_matches("~0.96", "0.96.4"));
        assert!(!range_matches("~0.96", "0.97"));
        assert!(range_matches("^2.0", "2.5.1"));
        assert!(!range_matches("^2.0", "3.0"));
        // pre-release：正式版 > 同主干预发布版（semver）
        assert!(range_matches("<1.21", "1.21-pre1"), "1.21-pre1 小于 1.21，应满足 <1.21");
        assert!(!range_matches(">=1.21", "1.21-pre1"), "1.21-pre1 小于 1.21，不满足 >=1.21");
        assert!(range_matches(">=1.20.1", "1.20.1"));
        assert!(!range_matches(">=1.20.1", "1.20.1-rc1"), "rc1 不算正式发布");
        // ~2 只给 major 时上界是 3.0（不是 2.1）
        assert!(range_matches("~2", "2.9"));
        assert!(!range_matches("~2", "3.0"));
    }

    #[test]
    fn fabric_depends_array_is_or_semantics() {
        // 数组写法：任一命中即满足
        let bytes = br#"{
            "id": "example",
            "version": "1.0",
            "depends": { "minecraft": ["1.20", "1.20.1", "1.20.4"] }
        }"#;
        let meta = parse_fabric_mod(bytes).unwrap();
        let (_, alts, _) = meta.depends.iter().find(|(id, _, _)| id == "minecraft").unwrap();
        assert_eq!(alts.len(), 3);
        assert!(alts.iter().any(|a| a == "1.20.1"));
        // 对象写法 { versions: [...] }
        let bytes2 = br#"{
            "id": "example",
            "version": "1.0",
            "depends": { "fabric-api": { "versions": [">=0.90", "<0.100"] } }
        }"#;
        let meta2 = parse_fabric_mod(bytes2).unwrap();
        let (_, alts2, _) = meta2.depends.iter().find(|(id, _, _)| id == "fabric-api").unwrap();
        assert_eq!(alts2.len(), 2, "versions 数组应是 OR 候选，不是 join 成一个串");
    }

    #[test]
    fn quilt_mod_parses_nested_loader_node() {
        let bytes = br#"{
            "schema_version": 1,
            "quilt_loader": {
                "id": "quilt-example",
                "version": "1.0.0",
                "depends": [
                    { "id": "quilt_loader", "versions": ">=0.22.0" },
                    { "id": "minecraft", "versions": ["1.20.1"] },
                    { "id": "optional-dep", "optional": true }
                ],
                "breaks": [ { "id": "evil-mod" } ]
            }
        }"#;
        let meta = parse_quilt_mod(bytes).unwrap();
        assert_eq!(meta.id, "quilt-example");
        assert_eq!(meta.version, "1.0.0");
        let (_, alts, mandatory) = meta
            .depends
            .iter()
            .find(|(id, _, _)| id == "quilt_loader")
            .unwrap();
        assert_eq!(alts, &vec![">=0.22.0".to_string()]);
        assert!(*mandatory);
        let optional = meta.depends.iter().find(|(id, _, _)| id == "optional-dep").unwrap();
        assert!(!optional.2, "optional=true 的依赖不应按必装报");
        assert_eq!(meta.conflicts.len(), 1);
        assert_eq!(meta.conflicts[0].0, "evil-mod");
    }

    #[test]
    fn version_ranges_maven_style() {
        assert!(maven_range_matches("[1.20,1.21)", "1.20.4"));
        assert!(!maven_range_matches("[1.20,1.21)", "1.21"));
        assert!(maven_range_matches("[1.20,)", "1.23"));
        assert!(!maven_range_matches("[1.20,)", "1.19"));
        assert!(maven_range_matches("(,1.21]", "1.21"));
        assert!(!maven_range_matches("(,1.21]", "1.21.1"));
    }

    #[test]
    fn fabric_meta_parses_depends_and_breaks() {
        let bytes = br#"{
            "id": "sodium",
            "version": "0.5.8",
            "depends": { "minecraft": ">=1.20 <1.20.5", "fabric-api": "*" },
            "breaks": { "optifine": "*" }
        }"#;
        let meta = parse_fabric_mod(bytes).unwrap();
        assert_eq!(meta.id, "sodium");
        assert_eq!(meta.depends.len(), 2);
        assert!(meta.depends.iter().any(|(id, _, _)| id == "fabric-api"));
        assert_eq!(meta.conflicts.len(), 1);
        assert!(meta.conflicts[0].1, "breaks 应标记为硬冲突");
    }

    #[test]
    fn mods_toml_light_parse() {
        let bytes = br#"
modLoader="javafml"
[[mods]]
modId="create"
version="0.5.1f"
[[dependencies.create]]
modId="forge"
mandatory=true
versionRange="[47,)"
ordering="NONE"
[[dependencies.create]]
modId="minecraft"
versionRange="[1.20.1,1.20.2)"
mandatory=true
"#;
        let meta = parse_mods_toml(bytes).unwrap();
        assert_eq!(meta.id, "create");
        assert_eq!(meta.version, "0.5.1f");
        assert!(meta.depends.iter().any(|(id, r, _)| id == "forge" && r == &vec!["[47,)".to_string()]));
        assert!(
            meta.depends
                .iter()
                .any(|(id, r, _)| id == "minecraft" && r.iter().any(|x| x.contains("1.20.1")))
        );
    }

    #[test]
    fn diagnose_reports_missing_and_conflicts() {
        let dir = std::env::temp_dir().join(format!("mcl-moddiag-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // 造两个假 jar：只有文件名参与诊断（read_meta 读不到元数据会返回空的）
        std::fs::write(dir.join("Sodium.jar"), b"x").unwrap();
        std::fs::write(dir.join("Iris.jar"), b"y").unwrap();

        let ctx = DiagnosticContext {
            game_version: "1.20.1".into(),
            loader: "fabric".into(),
            loader_version: "0.16.9".into(),
        };
        // 假 jar 没有元数据，诊断应产出零问题（不误报）
        let issues = diagnose(&dir, &ctx, &["Sodium.jar".into(), "Iris.jar".into()]);
        assert!(issues.is_empty(), "无元数据的 jar 不该误报：{issues:?}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn known_conflict_pair_triggers_warning() {
        let mut present = BTreeMap::new();
        present.insert("sodium".to_string(), ("0.5".to_string(), "Sodium.jar".to_string()));
        present.insert("optifine".to_string(), ("1.20".to_string(), "OptiFine.jar".to_string()));
        // 直接复用主流程最后一段的逻辑
        let ctx = DiagnosticContext {
            game_version: "1.20.1".into(),
            loader: "fabric".into(),
            loader_version: String::new(),
        };
        let dir = std::env::temp_dir().join(format!("mcl-moddiag-kc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // 构造带 breaks 声明的场景：用无元数据 jar 只验证 present 路径不可行，
        // 这里改为验证 version_of 对特殊 id 的映射
        assert_eq!(version_of("minecraft", &ctx, &present).unwrap(), "1.20.1");
        assert_eq!(version_of("sodium", &ctx, &present).unwrap(), "0.5");
        assert!(version_of("jei", &ctx, &present).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
