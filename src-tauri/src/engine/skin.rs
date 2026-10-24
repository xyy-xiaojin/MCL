//! 玩家皮肤：UUID 派生、Steve/Alex 判定、皮肤贴图获取与缓存。
//!
//! 这一套是照着 PCL2 的做法实现的（`ModMinecraft.vb` + `MySkin.xaml.vb`），
//! 只是把 WPF 的 `Clip` 换成了前端的 `background-position`：
//!
//! 1. **地址**：`sessionserver.mojang.com/session/minecraft/profile/<uuid>`
//!    → `properties` 里 `name="textures"` 的 base64 → 解码出 `textures.SKIN.url`；
//! 2. **缓存**：索引落 `cache/skins/index.json`（uuid → url/文件/时间），
//!    贴图按 **URL 的 SHA-1** 存成 `cache/skins/<hash>.png`。
//!    缓存键是 URL 而不是 UUID —— 玩家换了皮肤，URL 变了自然会重新下；
//! 3. **本地皮肤**：玩家可以自己导一张 PNG（`<root>/skins/offline.png`），
//!    离线账号优先用它，手臂模型单独存 `settings.skin_model`；
//! 4. **兜底**：离线账号、没皮肤、请求失败、超时，一律回落到内置的
//!    Steve / Alex 贴图，绝不因为头像拉不到就让界面报错或者卡住。
//!
//! 失败也会写缓存（10 分钟内不再重试），否则断网时每次刷新界面都要等一次超时。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::state::{AppState, Paths};

/// 官方会话服务器。皮肤只在正版账号下才有，镜像也没有这个端点。
const PROFILE_API: &str = "https://sessionserver.mojang.com/session/minecraft/profile";
/// 成功结果缓存多久（秒）：皮肤换得不勤，6 小时足够，也不会永远看不到新皮肤。
const OK_TTL: u64 = 6 * 3600;
/// 失败结果缓存多久（秒）：断网时别每次都等一次超时。
const FAIL_TTL: u64 = 10 * 60;

const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

/// 前端画头像需要的信息。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Avatar {
    /// 实际使用的 UUID（离线账号是按用户名派生的）
    pub uuid: String,
    /// `steve`（宽手臂）/ `alex`（细手臂）
    pub model: String,
    /// 皮肤贴图的 data URL；`None` 表示用前端内置贴图
    pub image: Option<String>,
    /// 数据来源：`offline` / `cache` / `mojang`
    pub source: String,
}

/// 按用户名派生离线 UUID。
///
/// MC 生态的约定是 `MD5("OfflinePlayer:<name>")`，再套上 RFC 4122 的
/// 版本位/变体位。用 SHA-1 也能得到一个"看起来像"的 UUID，但和皮肤站、
/// 统计插件算出来的对不上，存档里的玩家数据会变成另一个人。
pub fn offline_uuid(username: &str) -> String {
    let digest = md5::compute(format!("OfflinePlayer:{username}"));
    let mut bytes = digest.0;
    bytes[6] = (bytes[6] & 0x0f) | 0x30;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// 只有 32 位十六进制（可带连字符）才当作真 UUID，否则按用户名派生。
pub fn is_uuid_like(text: &str) -> bool {
    let digits: String = text.chars().filter(|c| *c != '-').collect();
    digits.len() == 32 && digits.chars().all(|c| c.is_ascii_hexdigit())
}

/// 统一成小写、带连字符的写法。
pub(crate) fn normalize_uuid(text: &str) -> String {
    let digits: String = text
        .chars()
        .filter(|c| *c != '-')
        .collect::<String>()
        .to_lowercase();
    if digits.len() != 32 {
        return offline_uuid(text);
    }
    format!(
        "{}-{}-{}-{}-{}",
        &digits[0..8],
        &digits[8..12],
        &digits[12..16],
        &digits[16..20],
        &digits[20..32]
    )
}

/// 没有皮肤信息时的兜底模型：**永远是 Steve**。
///
/// 以前这里按 UUID 的第 7/15/23/31 个十六进制位做异或奇偶来"猜" Alex
/// （照抄 PCL 的 `McSkinSex`），结果是离线账号一半左右会被判成 Alex——
/// 玩家看到的就是"默认皮肤怎么不是史蒂夫"。模型只应由皮肤自己声明
/// （`metadata.model == "slim"`），猜不出来就按官方默认的 Steve。
const DEFAULT_MODEL: &str = "steve";

/// 缓存索引，存在 `cache/skins/index.json`。
#[derive(Debug, Default, Serialize, Deserialize)]
struct Index {
    #[serde(default)]
    entries: BTreeMap<String, Entry>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Entry {
    /// 皮肤 URL；空串表示"查过了但没有皮肤/查不到"
    #[serde(default)]
    url: String,
    /// 贴图文件名（`<hash>.png`），空串表示没有
    #[serde(default)]
    file: String,
    /// 皮肤自己声明的模型（`steve` / `alex`），空串表示没有声明
    #[serde(default)]
    model: String,
    /// 记录时间（unix 秒）
    #[serde(default)]
    at: u64,
}

impl Entry {
    fn fresh(&self, now: u64) -> bool {
        let ttl = if self.file.is_empty() { FAIL_TTL } else { OK_TTL };
        now.saturating_sub(self.at) < ttl
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn cache_dir(state: &AppState) -> PathBuf {
    state.paths.cache().join("skins")
}

fn read_index(dir: &Path) -> Index {
    std::fs::read(dir.join("index.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// 写索引：写临时文件再改名，避免进程被杀时留下半截 JSON。
fn write_index(dir: &Path, index: &Index) {
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let Ok(text) = serde_json::to_vec_pretty(index) else {
        return;
    };
    let temp = dir.join("index.json.tmp");
    if std::fs::write(&temp, text).is_ok() {
        let _ = std::fs::rename(&temp, dir.join("index.json"));
    }
}

/// 把皮肤文件读成 data URL。
///
/// 不走 Tauri 的 asset 协议：那需要在 `tauri.conf.json` 里开
/// `assetProtocol` 并配 scope，而一张 64×64 的皮肤只有几 KB，
/// base64 塞进 IPC 更省事（几十 KB 的字符串对 IPC 毫无压力）。
fn data_url(bytes: &[u8]) -> String {
    format!("data:image/png;base64,{}", b64_encode(bytes))
}

fn sha1_hex(bytes: &[u8]) -> String {
    use sha1::{Digest, Sha1};
    let mut hasher = Sha1::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// 解析头像。**不会返回错误**：拿不到就用内置贴图，这是头像该有的降级方式。
///
/// `session_api` 是皮肤站的 sessionserver 根（yggdrasil 部署），
/// 传了就走皮肤站查皮肤（响应格式与官方兼容）；`None` 走官方。
pub async fn resolve(
    state: &AppState,
    name: &str,
    uuid: Option<&str>,
    session_api: Option<&str>,
) -> Avatar {
    let name = name.trim();
    let name = if name.is_empty() { "Player" } else { name };

    let (uuid, official) = match uuid.map(str::trim).filter(|u| !u.is_empty()) {
        Some(raw) if is_uuid_like(raw) => (normalize_uuid(raw), true),
        // 离线模式：没有正版 UUID，皮肤无从查起，按名字派生一个稳定的
        _ => (offline_uuid(name), false),
    };
    let endpoint = session_api.unwrap_or(PROFILE_API);
    let source = if session_api.is_some() { "authlib" } else { "mojang" };

    if !official {
        // 离线账号本来查不到皮肤，玩家自己导的那张优先
        return local_skin(state, &uuid).unwrap_or_else(|| fallback_avatar(&uuid, "offline"));
    }

    let dir = cache_dir(state);
    let now = now_secs();
    let mut index = read_index(&dir);

    // 1) 命中缓存：连失败的记录也算命中（10 分钟内不再重试）
    if let Some(entry) = index.entries.get(&uuid) {
        if entry.fresh(now) {
            if entry.file.is_empty() {
                return fallback_avatar(&uuid, "cache");
            }
            if let Ok(bytes) = std::fs::read(dir.join(&entry.file)) {
                return Avatar {
                    model: if entry.model.is_empty() {
                        DEFAULT_MODEL.to_string()
                    } else {
                        entry.model.clone()
                    },
                    uuid,
                    image: Some(data_url(&bytes)),
                    source: "cache".to_string(),
                };
            }
        }
    }

    // 2) 查会话服务器
    let (url, model) = match fetch_skin(&state.client, endpoint, &uuid).await {
        Ok(found) => found,
        Err(_) => {
            remember(&dir, &mut index, &uuid, "", "", "", &now);
            return fallback_avatar(&uuid, source);
        }
    };
    // 皮肤自己声明了 model 就以它为准（正版细手臂玩家）；
    // 没声明（老皮肤 / 第三方服务）就按官方默认的 Steve，绝不靠 UUID 去猜
    let model = model.unwrap_or_else(|| DEFAULT_MODEL.to_string());

    // 3) 下载贴图。URL 是缓存键：换了皮肤 URL 就变，自然会重下
    let bytes = match download_skin(&state.client, &url).await {
        Ok(bytes) => bytes,
        // 地址拿到了但贴图下不动：把 url 记下来（file 留空），下次重试下载
        Err(_) => {
            remember(&dir, &mut index, &uuid, &url, "", &model, &now);
            return fallback_avatar(&uuid, source);
        }
    };

    let file = format!("{}.png", sha1_hex(url.as_bytes()));
    let mut stored = false;
    if std::fs::create_dir_all(&dir).is_ok() {
        let temp = dir.join(format!("{file}.tmp"));
        if std::fs::write(&temp, &bytes).is_ok()
            && std::fs::rename(&temp, dir.join(&file)).is_ok()
        {
            stored = true;
        }
    }
    // 落盘失败也照样把这次的贴图显示出来，只是下次启动还得重下
    remember(
        &dir,
        &mut index,
        &uuid,
        &url,
        if stored { file.as_str() } else { "" },
        &model,
        &now,
    );

    Avatar {
        model,
        uuid,
        image: Some(data_url(&bytes)),
        source: source.to_string(),
    }
}

/// 用内置贴图（前端渲染 Steve / Alex）。
fn fallback_avatar(uuid: &str, source: &str) -> Avatar {
    Avatar {
        model: DEFAULT_MODEL.to_string(),
        uuid: uuid.to_string(),
        image: None,
        source: source.to_string(),
    }
}

// ---------------------------------------------------------------------------
// 离线皮肤：玩家自己导入的 PNG
// ---------------------------------------------------------------------------

/// 本地皮肤上限 1 MB。正常的 64×64 皮肤只有 1~3 KB，HD 也就一两百 KB，
/// 超过这个数几乎肯定是选错了文件（比如整张截图），提前挡住比存进去好。
const MAX_SKIN_BYTES: usize = 1024 * 1024;

/// 导入的皮肤落在哪：`<root>/skins/offline.png`。
///
/// 固定文件名是有意的：一个玩家只有一张离线皮肤，固定名省掉了"哪个文件是
/// 当前皮肤"的记录，覆盖写天然就是"换皮肤"。
pub fn local_skin_file(paths: &Paths) -> PathBuf {
    paths.skins().join("offline.png")
}

/// 从 PNG 的 IHDR 读尺寸。
///
/// 只认标准 PNG：签名 + 第一个块必须是 IHDR（规范要求）。读不出来就当不是皮肤，
/// 比"先解码整张图"省得多——这里只需要 24 个字节。
fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 24 || !bytes.starts_with(&PNG_SIGNATURE) || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    Some((width, height))
}

/// 是不是一张能当皮肤用的尺寸。
///
/// 合法的是 64×64（现代皮肤）与 64×32（1.8 之前的老皮肤），
/// 以及它们的整数倍（HD 皮肤 128×128、256×256…）。
/// 别的图强行裁出来是花的，不如直接告诉玩家尺寸不对。
pub fn is_skin_size(width: u32, height: u32) -> bool {
    (64..=1024).contains(&width) && width % 64 == 0 && (height == width || height == width / 2)
}

/// 当前配置的本地皮肤。文件被删掉/损坏时当作没配置，静默回落内置贴图。
fn local_skin(state: &AppState, uuid: &str) -> Option<Avatar> {
    let settings = state.settings();
    let path = settings.skin_path.trim();
    if path.is_empty() {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    if !bytes.starts_with(&PNG_SIGNATURE) {
        return None;
    }
    Some(Avatar {
        uuid: uuid.to_string(),
        // 头像是从贴图里裁的 8×8 脸，宽/细手臂在这里看不出差别，
        // 所以本地皮肤一律按官方默认的 Steve 标（`metadata.model` 是官方
        // textures 属性才有的东西，本地文件里没有）。
        model: DEFAULT_MODEL.to_string(),
        image: Some(data_url(&bytes)),
        source: "local".to_string(),
    })
}

/// 校验并保存玩家挑的皮肤，返回可以直接显示的头像。
pub fn save_local_skin(state: &AppState, bytes: &[u8]) -> Result<Avatar> {
    if bytes.len() > MAX_SKIN_BYTES {
        return Err(Error::msg(format!(
            "皮肤文件 {}，超过 1 MB —— 是不是选错文件了？",
            human_size(bytes.len())
        )));
    }
    let (width, height) =
        png_size(bytes).ok_or_else(|| Error::msg("这不是有效的 PNG 文件，请挑一张皮肤图"))?;
    if !is_skin_size(width, height) {
        return Err(Error::msg(format!(
            "尺寸 {width}×{height} 不是皮肤，需要 64×64（或 64×32 的老皮肤）及其整数倍"
        )));
    }

    // 先落盘再报成功：写不进去就当导入失败，别让玩家以为换上了
    write_local_skin(&state.paths, &state.settings, bytes)?;

    Ok(Avatar {
        uuid: offline_uuid(&state.settings().username),
        model: DEFAULT_MODEL.to_string(),
        image: Some(data_url(bytes)),
        source: "local".to_string(),
    })
}

/// 真正的落盘：写 PNG + 记住路径。
///
/// 拆出来是为了能脱离 `AppState` 单测——`AppState` 里的队列需要 `AppHandle`，
/// 在 `cargo test` 里造不出来。这里只依赖目录和设置，够测"文件写没写进去、
/// 设置有没有持久化"这两件最容易出错的事。
fn write_local_skin(
    paths: &Paths,
    settings: &std::sync::RwLock<crate::state::Settings>,
    bytes: &[u8],
) -> Result<PathBuf> {
    let dir = paths.skins();
    std::fs::create_dir_all(&dir)?;
    let target = local_skin_file(paths);
    // 先写临时文件再改名：中途失败不会把旧皮肤写坏
    let temp = dir.join("offline.png.tmp");
    std::fs::write(&temp, bytes)?;
    std::fs::rename(&temp, &target)?;

    let mut updated = settings.read().map(|s| s.clone()).unwrap_or_default();
    updated.skin_path = target.to_string_lossy().to_string();
    updated.save(paths)?;
    if let Ok(mut guard) = settings.write() {
        *guard = updated;
    }
    Ok(target)
}

/// 从磁盘文件导入（拖拽走这条）。
pub fn import_local_skin(state: &AppState, path: &Path) -> Result<Avatar> {
    let meta = std::fs::metadata(path)?;
    if meta.len() > MAX_SKIN_BYTES as u64 {
        return Err(Error::msg("皮肤文件超过 1 MB —— 是不是选错文件了？"));
    }
    let bytes = std::fs::read(path)?;
    save_local_skin(state, &bytes)
}

/// 恢复默认皮肤：删掉导入的文件并清空设置。
///
/// 文件删不掉也不报错：设置已经清了，界面立刻回到 Steve，残留文件下次导入会被覆盖。
pub fn reset_local_skin(state: &AppState) -> Result<()> {
    let mut settings = state.settings();
    settings.skin_path = String::new();
    settings.save(&state.paths)?;
    let file = local_skin_file(&state.paths);
    if let Ok(mut guard) = state.settings.write() {
        *guard = settings;
    }
    let _ = std::fs::remove_file(file);
    Ok(())
}

/// 前端用 `<input type="file">` 读出来是 data URL，也允许直接传裸 base64。
pub fn decode_skin_data(input: &str) -> Result<Vec<u8>> {
    let payload = input.trim();
    let payload = match payload.find("base64,") {
        Some(index) if payload.starts_with("data:") => &payload[index + "base64,".len()..],
        _ => payload,
    };
    b64_decode(payload).ok_or_else(|| Error::msg("皮肤数据不是合法的 base64"))
}

fn human_size(bytes: usize) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{} KB", bytes / 1024)
    }
}

/// 更新索引并落盘。写失败也不影响这次返回，缓存本来就是"下次更快"。
fn remember(
    dir: &Path,
    index: &mut Index,
    uuid: &str,
    url: &str,
    file: &str,
    model: &str,
    at: &u64,
) {
    index.entries.insert(
        uuid.to_string(),
        Entry {
            url: url.to_string(),
            file: file.to_string(),
            model: if model.is_empty() {
                DEFAULT_MODEL.to_string()
            } else {
                model.to_string()
            },
            at: *at,
        },
    );
    write_index(dir, index);
}

#[derive(Debug, Deserialize)]
struct Profile {
    #[serde(default)]
    properties: Vec<Property>,
}

#[derive(Debug, Deserialize)]
struct Property {
    name: String,
    value: String,
}

#[derive(Debug, Default, Deserialize)]
struct Textures {
    #[serde(default)]
    textures: TextureSet,
}

#[derive(Debug, Default, Deserialize)]
struct TextureSet {
    /// 注意：官方 JSON 里键名是**大写**的 `SKIN`，serde 区分大小写
    #[serde(rename = "SKIN", default)]
    skin: Option<TextureRef>,
}

#[derive(Debug, Deserialize)]
struct TextureRef {
    url: String,
    #[serde(default)]
    metadata: Option<TextureMeta>,
}

#[derive(Debug, Deserialize)]
struct TextureMeta {
    #[serde(default)]
    model: Option<String>,
}

/// 查皮肤地址 + 模型（`classic` / `slim`）。
///
/// `endpoint` 是 sessionserver 根（官方或皮肤站的 yggdrasil 部署）。
async fn fetch_skin(
    client: &reqwest::Client,
    endpoint: &str,
    uuid: &str,
) -> std::result::Result<(String, Option<String>), String> {
    let id = uuid.replace('-', "");
    let response = client
        .get(format!("{endpoint}/{id}"))
        .timeout(Duration::from_secs(8))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    // 204 = 这个 UUID 没有皮肤（也常见于从未登录过的账号）
    if response.status() == reqwest::StatusCode::NO_CONTENT {
        return Err("没有皮肤".to_string());
    }
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status()));
    }

    let profile: Profile = response.json().await.map_err(|e| e.to_string())?;
    let property = profile
        .properties
        .iter()
        .find(|p| p.name == "textures")
        .ok_or_else(|| "缺少 textures 属性".to_string())?;

    let raw = b64_decode(&property.value).ok_or_else(|| "textures 不是合法 base64".to_string())?;
    let decoded: Textures = serde_json::from_slice(&raw).map_err(|e| e.to_string())?;
    let skin = decoded
        .textures
        .skin
        .ok_or_else(|| "没有皮肤贴图".to_string())?;

    let model = skin
        .metadata
        .and_then(|m| m.model)
        .map(|m| if m == "slim" { "alex" } else { "steve" }.to_string());

    // 官方给的地址有时是 http://，直接升到 https，别在明文连接上下载
    let url = match skin.url.strip_prefix("http://") {
        Some(rest) => format!("https://{rest}"),
        None => skin.url,
    };

    Ok((url, model))
}

/// 下载贴图。超时会失败，但失败也不影响界面（回落内置贴图）。
async fn download_skin(
    client: &reqwest::Client,
    url: &str,
) -> std::result::Result<Vec<u8>, String> {
    let response = client
        .get(url)
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status()));
    }
    let bytes = response.bytes().await.map_err(|e| e.to_string())?;
    if !bytes.starts_with(&PNG_SIGNATURE) {
        return Err("不是 PNG".to_string());
    }
    Ok(bytes.to_vec())
}

// ---------------------------------------------------------------------------
// base64：只为了这两处用途，不值得多引一个依赖
// ---------------------------------------------------------------------------

fn b64_encode(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[(triple >> 18) as usize & 0x3f] as char);
        out.push(TABLE[(triple >> 12) as usize & 0x3f] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(triple >> 6) as usize & 0x3f] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[triple as usize & 0x3f] as char
        } else {
            '='
        });
    }
    out
}

fn b64_decode(input: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut buffer: u32 = 0;
    let mut bits = 0u32;
    for byte in input.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            // URL-safe 变体也一起认了，服务端哪天换了编码不至于直接失败
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => break,
            b'\r' | b'\n' | b' ' | b'\t' => continue,
            _ => return None,
        };
        buffer = (buffer << 6) | value as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_uuid_matches_md5_convention() {
        let uuid = offline_uuid("Steve");
        assert_eq!(uuid.len(), 36);
        // 版本位必须是 3、变体位必须是 8/9/a/b
        assert_eq!(&uuid[14..15], "3");
        assert!(matches!(&uuid[19..20], "8" | "9" | "a" | "b"));
        assert_eq!(uuid, offline_uuid("Steve"));
        assert_ne!(uuid, offline_uuid("Alex"));
    }

    #[test]
    fn uuid_detection_and_normalization() {
        assert!(is_uuid_like("069a79f444e94726a5befca90e38aaf5"));
        assert!(is_uuid_like("069a79f4-44e9-4726-a5be-fca90e38aaf5"));
        assert!(!is_uuid_like("Steve"));
        assert_eq!(
            normalize_uuid("069A79F444E94726A5BEFCA90E38AAF5"),
            "069a79f4-44e9-4726-a5be-fca90e38aaf5"
        );
    }

    #[test]
    fn fallback_avatar_is_always_steve() {
        // 没有皮肤信息时不能"猜" Alex：离线账号一律官方默认的 Steve
        for name in ["Steve", "Alex", "Notch", "Player", "张三"] {
            let uuid = offline_uuid(name);
            let avatar = fallback_avatar(&uuid, "offline");
            assert_eq!(avatar.model, "steve", "{name} 的兜底模型应该是 Steve");
            assert!(avatar.image.is_none());
        }
    }

    /// 造一个只有文件头的 PNG：判断尺寸只需要前 24 字节。
    fn png_header(width: u32, height: u32) -> Vec<u8> {
        let mut out = Vec::from(PNG_SIGNATURE);
        out.extend_from_slice(&13u32.to_be_bytes()); // IHDR 数据长度
        out.extend_from_slice(b"IHDR");
        out.extend_from_slice(&width.to_be_bytes());
        out.extend_from_slice(&height.to_be_bytes());
        out.extend_from_slice(&[8, 6, 0, 0, 0]); // 位深 / 颜色类型 / 压缩 / 滤波 / 隔行
        out
    }

    #[test]
    fn skin_sizes_are_validated() {
        // 现代皮肤、1.8 之前的老皮肤、HD 皮肤都合法
        for (w, h) in [(64, 64), (64, 32), (128, 64), (128, 128), (256, 256), (1024, 1024)] {
            assert!(is_skin_size(w, h), "{w}×{h} 应该是合法皮肤");
        }
        // 截图、头像、比例不对的一律拒绝
        for (w, h) in [(16, 16), (100, 100), (64, 16), (256, 64), (2048, 2048), (0, 0)] {
            assert!(!is_skin_size(w, h), "{w}×{h} 不该被当成皮肤");
        }
    }

    #[test]
    fn png_size_reads_ihdr() {
        assert_eq!(png_size(&png_header(64, 64)), Some((64, 64)));
        assert_eq!(png_size(&png_header(256, 128)), Some((256, 128)));
        // 不是 PNG、或者头被截断 → 认不出来
        assert_eq!(png_size(b"not a png at all"), None);
        assert_eq!(png_size(&png_header(64, 64)[..20]), None);
    }

    #[test]
    fn skin_data_accepts_data_url_and_raw_base64() {
        let bytes = png_header(64, 64);
        let encoded = b64_encode(&bytes);
        assert_eq!(decode_skin_data(&encoded).unwrap(), bytes);
        assert_eq!(
            decode_skin_data(&format!("data:image/png;base64,{encoded}")).unwrap(),
            bytes
        );
        // 中文字符不是 base64，应当报错而不是静默产出垃圾字节
        assert!(decode_skin_data("这不是 base64").is_err());
    }

    #[test]
    fn local_skin_is_written_and_remembered() {
        let root = std::env::temp_dir().join(format!("mcl-skin-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let paths = Paths { root: root.clone() };
        let settings = std::sync::RwLock::new(crate::state::Settings::default());

        let bytes = png_header(64, 64);
        let target = write_local_skin(&paths, &settings, &bytes).expect("写皮肤");
        assert_eq!(target, root.join("skins").join("offline.png"));
        assert_eq!(std::fs::read(&target).unwrap(), bytes);
        // 临时文件不能留在目录里
        assert!(!root.join("skins").join("offline.png.tmp").exists());

        // 路径要进内存里的设置，也要落盘 —— 只进内存的话重启就丢了
        let remembered = settings.read().unwrap().skin_path.clone();
        assert_eq!(remembered, target.to_string_lossy());
        assert_eq!(crate::state::Settings::load(&paths).skin_path, remembered);

        // 再导一次（换皮肤）应该原地覆盖，不是报错或另存一份
        let other = png_header(128, 128);
        write_local_skin(&paths, &settings, &other).expect("覆盖皮肤");
        assert_eq!(std::fs::read(&target).unwrap(), other);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn base64_roundtrip() {
        let cases: [&[u8]; 5] = [b"", b"f", b"fo", b"foo", b"foobarbaz?"];
        for case in cases {
            let encoded = b64_encode(case);
            assert_eq!(b64_decode(&encoded).as_deref(), Some(case));
        }
        // 标准向量
        assert_eq!(b64_encode(b"Man"), "TWFu");
        assert_eq!(b64_encode(b"Ma"), "TWE=");
        assert_eq!(b64_encode(b"M"), "TQ==");
    }

    /// 真的要联网，默认不跑。手动验证用：
    /// `cargo test --lib skin -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "需要访问 sessionserver.mojang.com"]
    async fn real_profile_yields_a_png_skin() {
        let client = crate::engine::net::client().expect("client");
        // Notch 的 UUID，官方账号一定有皮肤
        let (url, model) = fetch_skin(&client, PROFILE_API, "069a79f4-44e9-4726-a5be-fca90e38aaf5")
            .await
            .expect("查皮肤地址");
        assert!(url.starts_with("http"), "皮肤地址异常：{url}");
        let bytes = download_skin(&client, &url).await.expect("下载皮肤");
        assert!(bytes.len() > 100, "贴图太小：{} 字节", bytes.len());
        println!("url={url} model={model:?} bytes={}", bytes.len());
    }
}
