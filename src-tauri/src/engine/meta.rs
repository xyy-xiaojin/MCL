//! 版本元数据：清单、version.json、资源索引，以及 rules 判定与 `inheritsFrom` 合并。
//!
//! 字段命名按 Mojang 的原始 JSON（camelCase），内部一律 snake_case。
//! 所有结构体都 `#[serde(default)]`，因为继承合并过程中会出现大量"只写了几个字段"的版本。
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::{Error, Result};

// ---------------------------------------------------------------------------
// version_manifest.json
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct VersionManifest {
    pub latest: Latest,
    pub versions: Vec<VersionEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Latest {
    pub release: String,
    pub snapshot: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct VersionEntry {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    /// 该版本 version.json 的地址（镜像模式下会被重写）。
    pub url: String,
}

// ---------------------------------------------------------------------------
// version.json
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Deserialize)]
pub struct VersionJson {
    #[serde(default)]
    pub id: String,
    #[serde(rename = "inheritsFrom", default)]
    pub inherits_from: Option<String>,
    #[serde(rename = "mainClass", default)]
    pub main_class: Option<String>,
    #[serde(rename = "assetIndex", default)]
    pub asset_index: Option<AssetIndexRef>,
    #[serde(default)]
    pub assets: Option<String>,
    #[serde(default)]
    pub libraries: Vec<Library>,
    #[serde(default)]
    pub arguments: Option<Arguments>,
    #[serde(rename = "minecraftArguments", default)]
    pub minecraft_arguments: Option<String>,
    #[serde(rename = "javaVersion", default)]
    pub java_version: Option<JavaVersion>,
    #[serde(rename = "type", default)]
    pub version_type: Option<String>,
    #[serde(default)]
    pub downloads: Option<BTreeMap<String, DownloadInfo>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AssetIndexRef {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub sha1: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    /// 索引 json 自身的字节数（下载校验用）。
    #[serde(default)]
    pub size: Option<u64>,
    /// 索引里所有资源对象的字节总数，只用于显示，绝不能当成文件大小。
    #[serde(rename = "totalSize", default)]
    #[allow(dead_code)] // 纯展示字段，保留下来方便排查"大小对不上"的问题
    pub total_size: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct JavaVersion {
    #[serde(rename = "majorVersion", default)]
    pub major_version: Option<i64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct DownloadInfo {
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub sha1: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Library {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub downloads: Option<LibraryDownloads>,
    /// 平台名 → 分类器名，例如 `{"windows": "natives-windows-${arch}"}`。
    #[serde(default)]
    pub natives: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub rules: Option<Vec<Rule>>,
    #[serde(default)]
    pub extract: Option<Extract>,
    /// 第三方仓库根地址（Forge / Fabric 等只给仓库不给 downloads 的情况）。
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct LibraryDownloads {
    #[serde(default)]
    pub artifact: Option<Artifact>,
    #[serde(default)]
    pub classifiers: BTreeMap<String, Artifact>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Artifact {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub sha1: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Extract {
    /// 解压时跳过的前缀，通常是 `["META-INF/"]`。
    #[serde(default)]
    pub exclude: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Arguments {
    #[serde(default)]
    pub game: Vec<Argument>,
    #[serde(default)]
    pub jvm: Vec<Argument>,
}

/// 参数元素：老版本是纯字符串，新版本是带 rules 的对象。
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Argument {
    Plain(String),
    Conditional {
        #[serde(default)]
        rules: Option<Vec<Rule>>,
        value: ArgValue,
    },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum ArgValue {
    One(String),
    Many(Vec<String>),
}

#[derive(Debug, Clone, Deserialize)]
pub struct Rule {
    pub action: RuleAction,
    #[serde(default)]
    pub os: Option<OsRule>,
    #[serde(default)]
    pub features: Option<BTreeMap<String, bool>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuleAction {
    Allow,
    Disallow,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct OsRule {
    #[serde(default)]
    pub name: Option<String>,
    /// 正则，匹配操作系统版本号。
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub arch: Option<String>,
}

// ---------------------------------------------------------------------------
// 平台与 rules
// ---------------------------------------------------------------------------

/// 当前运行环境。抽成结构体是为了让 rules 判定能在测试里构造不同平台。
#[derive(Debug, Clone)]
pub struct Platform {
    pub os: String,
    pub arch: String,
    pub os_version: String,
}

impl Platform {
    pub fn current() -> Self {
        let os = if cfg!(target_os = "windows") {
            "windows"
        } else if cfg!(target_os = "macos") {
            "osx"
        } else {
            "linux"
        };
        Self {
            os: os.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            os_version: detect_os_version(),
        }
    }

    /// 只在测试里用：构造"另一台机器"的环境来验证 rules。
    #[cfg(test)]
    pub fn new(os: &str, arch: &str) -> Self {
        Self {
            os: os.to_string(),
            arch: arch.to_string(),
            os_version: String::new(),
        }
    }
}

/// 系统版本号，给 rules 里的 `os.version` 用。
///
/// Mojang 规范里这个字段是"操作系统版本"，最常见的用法是用 `^10\.`
/// 这类正则把库限制在 Windows 10 及以上。Linux 没有统一的版本号语义，
/// 取发行版的 `VERSION_ID`（Ubuntu 的 `24.04`、Fedora 的 `40`）最接近。
///
/// 拿不到就返回空串：空串匹配不上任何 version 正则，带该规则的库会被判为
/// 不匹配——这与"这台机器不满足该规则"的语义一致，是安全的方向。
fn detect_os_version() -> String {
    read_os_release_version()
}

/// 解析 `/etc/os-release` 的 `VERSION_ID`。
///
/// 非 Linux 平台返回空串：Windows 要拿 build 号得引 winapi，macOS 要解析
/// plist，而这两处的 `os.version` 规则只影响极少数老库，不值得为它加依赖。
#[cfg(target_os = "linux")]
fn read_os_release_version() -> String {
    match std::fs::read_to_string("/etc/os-release") {
        Ok(text) => parse_os_release(&text),
        Err(_) => String::new(),
    }
}

#[cfg(not(target_os = "linux"))]
fn read_os_release_version() -> String {
    String::new()
}

/// 从 `/etc/os-release` 的内容里取 `VERSION_ID`，去掉引号。
///
/// 单独抽出来是为了能在任何平台上测：真实文件只在 Linux 上有。
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_os_release(text: &str) -> String {
    text.lines()
        .find_map(|line| line.strip_prefix("VERSION_ID="))
        .map(|value| value.trim().trim_matches('"').to_string())
        .unwrap_or_default()
}

/// 判定一组 rules 是否放行。
///
/// 语义与 Mojang 规范一致：没有 rules 就放行；有 rules 时**默认拒绝**，
/// 逐条匹配、由最后一条命中的规则决定。
pub fn allowed(rules: Option<&[Rule]>, platform: &Platform) -> bool {
    let Some(rules) = rules else { return true };
    if rules.is_empty() {
        return true;
    }
    let mut result = false;
    for rule in rules {
        if rule_matches(rule, platform) {
            result = matches!(rule.action, RuleAction::Allow);
        }
    }
    result
}

fn rule_matches(rule: &Rule, platform: &Platform) -> bool {
    if let Some(os) = &rule.os {
        if let Some(name) = &os.name {
            // `unknown` 在规范里几乎不出现，按"不限制"处理更安全。
            if !name.eq_ignore_ascii_case("unknown") && !name.eq_ignore_ascii_case(&platform.os) {
                return false;
            }
        }
        if let Some(pattern) = &os.version {
            match regex::Regex::new(pattern) {
                Ok(re) => {
                    if !re.is_match(&platform.os_version) {
                        return false;
                    }
                }
                // 正则写坏了按"不匹配"处理，避免整条规则退化成放行。
                Err(_) => return false,
            }
        }
        if let Some(arch) = &os.arch {
            if !arch_matches(arch, &platform.arch) {
                return false;
            }
        }
    }
    if let Some(features) = &rule.features {
        for (key, expected) in features {
            // 0.01 没有开启任何可选 feature（demo 模式、自定义分辨率等）。
            if *expected {
                return false;
            }
            let _ = key;
        }
    }
    true
}

fn arch_matches(want: &str, actual: &str) -> bool {
    let want = want.to_ascii_lowercase();
    let actual = actual.to_ascii_lowercase();
    match want.as_str() {
        "x86" => actual == "x86",
        "x86_64" | "x64" | "amd64" => actual == "x86_64",
        other if other.contains("arm64") || other == "aarch64" => {
            actual == "aarch64" || actual == "arm64"
        }
        other if other.contains("arm") => actual.starts_with("arm"),
        // 没见过的架构名不额外限制，避免把未知平台全部过滤掉。
        _ => true,
    }
}

// ---------------------------------------------------------------------------
// 继承合并
// ---------------------------------------------------------------------------

/// 合并父/子版本。标量字段子版本优先，类库与参数**追加**（父在前）。
pub fn merge(parent: &VersionJson, child: &VersionJson) -> VersionJson {
    let mut out = child.clone();
    if out.id.is_empty() {
        out.id = parent.id.clone();
    }
    if out.main_class.is_none() {
        out.main_class = parent.main_class.clone();
    }
    if out.asset_index.is_none() {
        out.asset_index = parent.asset_index.clone();
    }
    if out.assets.is_none() {
        out.assets = parent.assets.clone();
    }
    if out.java_version.is_none() {
        out.java_version = parent.java_version.clone();
    }
    if out.version_type.is_none() {
        out.version_type = parent.version_type.clone();
    }
    if out.minecraft_arguments.is_none() {
        out.minecraft_arguments = parent.minecraft_arguments.clone();
    }
    // Fabric / Forge 不会重复写 downloads.client，漏了这行合并后就没有客户端 jar。
    if out.downloads.is_none() {
        out.downloads = parent.downloads.clone();
    }

    // 同坐标的类库只留一份，且以子版本（加载器）为准。不去重的话
    // Forge 手动安装时同一个库的两个版本会同时进 classpath，旧版本排前面先命中。
    let mut libraries = parent.libraries.clone();
    for item in &child.libraries {
        match libraries.iter().position(|l| l.name == item.name) {
            Some(pos) => libraries[pos] = item.clone(),
            None => libraries.push(item.clone()),
        }
    }
    out.libraries = libraries;

    out.arguments = match (&parent.arguments, &child.arguments) {
        (None, None) => None,
        (Some(p), None) => Some(p.clone()),
        (None, Some(c)) => Some(c.clone()),
        (Some(p), Some(c)) => {
            let mut merged = Arguments::default();
            merged.jvm.extend(p.jvm.iter().cloned());
            merged.jvm.extend(c.jvm.iter().cloned());
            merged.game.extend(p.game.iter().cloned());
            merged.game.extend(c.game.iter().cloned());
            Some(merged)
        }
    };
    out
}

/// 版本 ID 必须是单段安全目录名，防止 `../` 穿越。
pub fn is_safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id != "."
        && id != ".."
        && !id.starts_with('.')
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'+'))
}

/// 沿 `inheritsFrom` 加载并合并，返回 (完整版本, 继承链)。
///
/// 链的顺序是**子在前、最底层父版本在后**：调用方靠它判断客户端 jar 属于哪个版本目录
/// （Fabric 目录里没有 jar，jar 在底层原版目录）。
pub fn load_merged_with_chain(versions_dir: &Path, id: &str) -> Result<(VersionJson, Vec<String>)> {
    let mut chain: Vec<String> = Vec::new();
    let mut current = id.to_string();
    let mut acc: Option<VersionJson> = None;

    loop {
        if !is_safe_id(&current) {
            return Err(Error::msg(format!("版本 ID 不安全：{current:?}")));
        }
        if chain.contains(&current) {
            chain.push(current);
            return Err(Error::msg(format!(
                "版本继承出现循环：{}",
                chain.join(" → ")
            )));
        }
        if chain.len() >= 10 {
            return Err(Error::msg("版本继承层级过深（超过 10 层）"));
        }
        chain.push(current.clone());

        let path = versions_dir.join(&current).join(format!("{current}.json"));
        let text = std::fs::read_to_string(&path).map_err(|_| {
            Error::msg(format!(
                "版本 {current} 未安装：找不到 {}",
                path.display()
            ))
        })?;
        let json: VersionJson = serde_json::from_str(&text)
            .map_err(|e| Error::msg(format!("版本 {} 的元数据无法解析：{e}", path.display())))?;

        let parent = json
            .inherits_from
            .clone()
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty());

        acc = Some(match acc.take() {
            None => json,
            Some(child) => merge(&json, &child),
        });

        match parent {
            Some(next) => current = next,
            None => break,
        }
    }

    Ok((acc.expect("循环至少执行一次"), chain))
}

#[allow(dead_code)] // 只关心合并结果、不关心链的调用方用这个
pub fn load_merged(versions_dir: &Path, id: &str) -> Result<VersionJson> {
    load_merged_with_chain(versions_dir, id).map(|(v, _)| v)
}

// ---------------------------------------------------------------------------
// Maven 坐标与类库路径
// ---------------------------------------------------------------------------

/// Maven 坐标 → 仓库内相对路径（正斜杠）。
pub fn maven_to_path(coordinate: &str) -> Option<String> {
    let (coordinate, ext) = match coordinate.split_once('@') {
        Some((c, e)) => (c, e),
        None => (coordinate, "jar"),
    };
    let parts: Vec<&str> = coordinate.split(':').collect();
    if parts.len() < 3 {
        return None;
    }
    let group = parts[0].replace('.', "/");
    let artifact = parts[1];
    let version = parts[2];
    let file = match parts.get(3).filter(|c| !c.is_empty()) {
        Some(classifier) => format!("{artifact}-{version}-{classifier}.{ext}"),
        None => format!("{artifact}-{version}.{ext}"),
    };
    Some(format!("{group}/{artifact}/{version}/{file}"))
}

/// 类库在 `libraries/` 下的相对路径（优先用元数据给好的 path）。
pub fn library_path(library: &Library) -> Option<String> {
    if let Some(path) = library
        .downloads
        .as_ref()
        .and_then(|d| d.artifact.as_ref())
        .and_then(|a| a.path.clone())
    {
        return Some(path);
    }
    maven_to_path(&library.name)
}

/// 是否只是"原生库容器"（只有 classifiers，没有可进 classpath 的 artifact）。
///
/// 这类条目必须只解压不进 classpath：老版本 LWJGL 2 会把普通库和原生库拆成两条、
/// 坐标完全相同，一起塞进 classpath 会让 JVM 选错位数。
pub fn is_native_only(library: &Library, platform: &Platform) -> bool {
    let has_artifact = library
        .downloads
        .as_ref()
        .map(|d| d.artifact.is_some())
        .unwrap_or(false);
    !has_artifact && natives_classifier(library, platform).is_some()
}

/// 当前平台对应的 natives 分类器，展开 `${arch}`。
fn expand_arch(value: &str, arch: &str) -> String {
    if !value.contains("${arch}") {
        return value.to_string();
    }
    let bits = if matches!(arch, "x86_64" | "aarch64" | "arm64") {
        "64"
    } else {
        "32"
    };
    value.replace("${arch}", bits)
}

pub fn natives_classifier(library: &Library, platform: &Platform) -> Option<String> {
    let natives = library.natives.as_ref()?;
    natives
        .get(&platform.os)
        .map(|raw| expand_arch(raw, &platform.arch))
}

/// 正斜杠相对路径 → 平台路径。
pub fn rel_path(rel: &str) -> PathBuf {
    rel.split('/').collect()
}

// ---------------------------------------------------------------------------
// assets 索引
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AssetIndex {
    #[serde(default)]
    pub objects: BTreeMap<String, AssetObject>,
    /// 老版本（1.6 左右）要求把资源按原名铺到 `assets/virtual/<索引>/`。
    #[serde(rename = "virtual", default)]
    pub is_virtual: Option<bool>,
    /// 更早的版本要求铺到 `.minecraft/resources/`。
    #[serde(rename = "map_to_resources", default)]
    pub map_to_resources: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AssetObject {
    #[serde(default)]
    pub hash: String,
    #[serde(default)]
    pub size: u64,
}

/// 资源对象的磁盘路径：`objects/<前2位>/<完整哈希>`。
pub fn asset_object_path(assets_dir: &Path, hash: &str) -> Option<PathBuf> {
    if hash.len() != 40 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let hash = hash.to_ascii_lowercase();
    Some(
        assets_dir
            .join("objects")
            .join(&hash[..2])
            .join(&hash),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn linux() -> Platform {
        Platform::new("linux", "x86_64")
    }

    fn rule(action: RuleAction, os: Option<&str>) -> Rule {
        Rule {
            action,
            os: os.map(|name| OsRule {
                name: Some(name.to_string()),
                ..Default::default()
            }),
            features: None,
        }
    }

    #[test]
    fn rules_default_allow_and_last_match_wins() {
        assert!(allowed(None, &linux()));
        assert!(allowed(Some(&[]), &linux()));

        // 只有 allow(windows) → Linux 上不放行（这是照抄 PCL 会写错的地方）
        let only_windows = vec![rule(RuleAction::Allow, Some("windows"))];
        assert!(!allowed(Some(&only_windows), &linux()));
        assert!(allowed(Some(&only_windows), &Platform::new("windows", "x86_64")));

        // allow 全部 + disallow osx → Linux 放行
        let mixed = vec![
            rule(RuleAction::Allow, None),
            rule(RuleAction::Disallow, Some("osx")),
        ];
        assert!(allowed(Some(&mixed), &linux()));
        assert!(!allowed(Some(&mixed), &Platform::new("osx", "x86_64")));
    }

    #[test]
    fn os_version_rule_matches_distro_version() {
        // 这类规则最常见于"仅限 Windows 10 以上"的库
        let rules: Vec<Rule> =
            serde_json::from_str(r#"[{"action":"allow","os":{"name":"linux","version":"^24\\."}}]"#)
                .unwrap();
        let with = |v: &str| Platform {
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            os_version: v.to_string(),
        };
        assert!(allowed(Some(&rules), &with("24.04")));
        assert!(!allowed(Some(&rules), &with("22.04")));
        // 读不到版本号时按不匹配处理：宁可少放一个库，也不放错
        assert!(!allowed(Some(&rules), &with("")));
    }

    #[test]
    fn os_release_version_id_is_parsed() {
        // Ubuntu / Debian 是带引号的
        assert_eq!(
            parse_os_release("NAME=\"Ubuntu\"\nVERSION_ID=\"24.04\"\nID=ubuntu\n"),
            "24.04"
        );
        // Fedora 不带引号
        assert_eq!(parse_os_release("NAME=Fedora\nVERSION_ID=40\n"), "40");
        // Arch 之类滚动发行版根本没有这个字段
        assert_eq!(parse_os_release("NAME=Arch\nID=arch\n"), "");
    }

    #[test]
    fn asset_index_keeps_its_own_size() {
        // 真实 1.20.1 的 assetIndex：size 是索引 json 自身（413 KB），
        // totalSize 是全部资源对象的总量（651 MB）。下载校验必须用 size。
        let version: VersionJson = serde_json::from_str(
            r#"{"id":"1.20.1",
                "assetIndex":{"id":"5","sha1":"8a45","size":413067,
                              "totalSize":651728646,"url":"https://x/5.json"}}"#,
        )
        .unwrap();
        let index = version.asset_index.unwrap();
        assert_eq!(index.size, Some(413067));
        assert_eq!(index.total_size, Some(651728646));
    }

    #[test]
    fn unknown_os_name_does_not_constrain() {        let rules = vec![rule(RuleAction::Allow, Some("unknown"))];
        assert!(allowed(Some(&rules), &linux()));
        assert!(allowed(Some(&rules), &Platform::new("windows", "x86_64")));
    }

    #[test]
    fn inheritance_merges_arguments_and_client_download() {
        let parent: VersionJson = serde_json::from_str(
            r#"{"id":"1.20.1","mainClass":"a.Main","assets":"5",
                "downloads":{"client":{"url":"https://x/client.jar","sha1":"aa","size":9}},
                "arguments":{"game":["--version","${version_name}"],"jvm":["-cp","${classpath}"]},
                "libraries":[{"name":"p:1"}]}"#,
        )
        .unwrap();
        let child: VersionJson = serde_json::from_str(
            r#"{"id":"1.20.1-fabric","inheritsFrom":"1.20.1","mainClass":"b.Main",
                "libraries":[{"name":"fabric:1"}]}"#,
        )
        .unwrap();

        let merged = merge(&parent, &child);
        assert_eq!(merged.id, "1.20.1-fabric");
        assert_eq!(merged.main_class.as_deref(), Some("b.Main"));
        // 客户端下载信息来自父版本，丢了就没有 jar
        assert_eq!(
            merged
                .downloads
                .as_ref()
                .and_then(|d| d.get("client"))
                .and_then(|i| i.url.as_deref()),
            Some("https://x/client.jar")
        );
        // 类库父在前子在后
        assert_eq!(merged.libraries.len(), 2);
        assert_eq!(merged.libraries[0].name, "p:1");
        assert_eq!(merged.libraries[1].name, "fabric:1");
        let args = merged.arguments.unwrap();
        assert_eq!(args.game.len(), 2);
    }

    #[test]
    fn maven_coordinates_to_paths() {
        assert_eq!(
            maven_to_path("org.ow2.asm:asm:9.6").as_deref(),
            Some("org/ow2/asm/asm/9.6/asm-9.6.jar")
        );
        assert_eq!(
            maven_to_path("net.minecraftforge:forge:1.20.1-47.2.0:universal").as_deref(),
            Some("net/minecraftforge/forge/1.20.1-47.2.0/forge-1.20.1-47.2.0-universal.jar")
        );
        assert_eq!(maven_to_path("bad"), None);
    }

    #[test]
    fn native_only_libraries_are_detected() {
        let plain: Library = serde_json::from_str(
            r#"{"name":"org.lwjgl:lwjgl:3.3.1","downloads":{"artifact":{"path":"x.jar"}}}"#,
        )
        .unwrap();
        let native_only: Library = serde_json::from_str(
            r#"{"name":"org.lwjgl:lwjgl:3.3.1","natives":{"linux":"natives-linux"}}"#,
        )
        .unwrap();
        assert!(!is_native_only(&plain, &linux()));
        assert!(is_native_only(&native_only, &linux()));
    }

    #[test]
    fn natives_classifier_expands_arch() {
        let lib: Library = serde_json::from_str(
            r#"{"name":"a:b:1","natives":{"windows":"natives-windows-${arch}"}}"#,
        )
        .unwrap();
        assert_eq!(
            natives_classifier(&lib, &Platform::new("windows", "x86_64")).as_deref(),
            Some("natives-windows-64")
        );
        assert_eq!(
            natives_classifier(&lib, &Platform::new("windows", "x86")).as_deref(),
            Some("natives-windows-32")
        );
        assert_eq!(natives_classifier(&lib, &linux()), None);
    }

    #[test]
    fn asset_object_paths_reject_bad_hashes() {
        let dir = Path::new("/mc/assets");
        let ok = asset_object_path(dir, "A9993E364706816ABA3E25717850C26C9CD0D89D").unwrap();
        assert!(ok.ends_with(Path::new("a9").join("a9993e364706816aba3e25717850c26c9cd0d89d")));
        assert!(asset_object_path(dir, "../../etc/passwd").is_none());
        assert!(asset_object_path(dir, "abc").is_none());
    }

    #[test]
    fn load_merged_detects_cycles_and_reports_chain() {
        let root = std::env::temp_dir().join(format!(
            "mcl-meta-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        for (id, parent) in [("a", "b"), ("b", "a")] {
            let dir = root.join(id);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join(format!("{id}.json")),
                format!(r#"{{"id":"{id}","inheritsFrom":"{parent}","mainClass":"x.Main"}}"#),
            )
            .unwrap();
        }
        let err = load_merged(&root, "a").unwrap_err().to_string();
        assert!(err.contains("循环"), "应报告循环: {err}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn chain_order_is_child_first() {
        let root = std::env::temp_dir().join(format!(
            "mcl-chain-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("1.20.1")).unwrap();
        std::fs::write(
            root.join("1.20.1/1.20.1.json"),
            r#"{"id":"1.20.1","mainClass":"a.Main"}"#,
        )
        .unwrap();
        std::fs::create_dir_all(root.join("forge")).unwrap();
        std::fs::write(
            root.join("forge/forge.json"),
            r#"{"id":"forge","inheritsFrom":"1.20.1"}"#,
        )
        .unwrap();

        let (_, chain) = load_merged_with_chain(&root, "forge").unwrap();
        assert_eq!(chain, vec!["forge", "1.20.1"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn empty_classifier_maps_are_parsed() {
        let lib: Library =
            serde_json::from_str(r#"{"name":"a:b:1","downloads":{"classifiers":{}}}"#).unwrap();
        assert!(lib.downloads.unwrap().classifiers.is_empty());
        let _: BTreeMap<String, Artifact> = BTreeMap::new();
    }
}
