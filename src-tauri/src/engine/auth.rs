//! 皮肤站登录（authlib-injector / yggdrasil 协议）。
//!
//! 第三方皮肤站（LittleSkin 等）都实现了 yggdrasil API：
//!
//! 1. **登录**：`POST <yggdrasil根>/authserver/authenticate`（邮箱 + 密码）
//!    → `accessToken` + 角色列表。一个账号可以有多个角色，多角色时让玩家选；
//! 2. **启动注入**：`-javaagent:authlib-injector.jar=<yggdrasil根>` +
//!    真实的 `${auth_access_token}` / `${auth_uuid}`，游戏内的验证和皮肤
//!    就都走皮肤站了；
//! 3. **令牌续期**：token 有有效期。启动前先 `validate`，失效就 `refresh`
//!    换新的——两个都失败才让玩家重新登录，避免"动不动就要重输密码"；
//! 4. **皮肤**：皮肤站的 sessionserver 响应格式与官方兼容，
//!    [`skin`] 那套解析直接复用（见 `skin::resolve` 的 `endpoint` 参数）。
//!
//! 账户落盘 `root/account.json`。和 frp 的访问密钥一样是明文凭据，
//! 但**密码从不落盘**——落盘的只有登录换来的 token。

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::{Error, Result};
use crate::state::{AppState, Paths};

/// 离线模式（现状，默认值）。
pub const KIND_OFFLINE: &str = "offline";
/// 皮肤站模式。
pub const KIND_AUTHLIB: &str = "authlib";
/// 微软正版模式。
pub const KIND_MICROSOFT: &str = "microsoft";

/// 预置的皮肤站在前端硬编码（后端不需要）。

/// authlib-injector 的版本元数据。jar 本体从这里下载。
const ARTIFACT_API: &str = "https://authlib-injector.yushi.moe/artifact/latest.json";

/// 当前登录的账户。`kind == offline` 时其余字段无意义。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Account {
    /// [`KIND_OFFLINE`] / [`KIND_AUTHLIB`] / [`KIND_MICROSOFT`]
    pub kind: String,
    /// yggdrasil API 根（形如 `https://littleskin.cn/api/yggdrasil`）
    pub server_url: String,
    /// 皮肤站名字（来自元数据的 `serverName`），只用于展示
    pub server_name: String,
    /// 登录用的邮箱。只存邮箱不存密码。
    pub login_email: String,
    pub access_token: String,
    /// 角色 UUID（带连字符）。登录后有多个角色且未选择时为空。
    pub profile_id: String,
    /// 角色名（游戏 ID）
    pub profile_name: String,
    /// 微软账号的 refresh_token（正版模式专用）。MC 的 access_token 只活
    /// 24 小时，靠它走刷新链换新的；密码和微软密码从不落盘。
    pub ms_refresh_token: String,
}

impl Default for Account {
    fn default() -> Self {
        Self {
            kind: KIND_OFFLINE.to_string(),
            server_url: String::new(),
            server_name: String::new(),
            login_email: String::new(),
            access_token: String::new(),
            profile_id: String::new(),
            profile_name: String::new(),
            ms_refresh_token: String::new(),
        }
    }
}

impl Account {
    pub fn load(paths: &Paths) -> Self {
        std::fs::read(paths.root.join("account.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// 是不是已就绪的登录账户（有 token、选好了角色）。
    pub fn active(&self) -> bool {
        (self.kind == KIND_AUTHLIB || self.kind == KIND_MICROSOFT)
            && !self.access_token.is_empty()
            && !self.profile_id.is_empty()
    }

    /// 给前端的视图：token 绝不回传。
    pub fn view(&self) -> AccountView {
        AccountView {
            kind: self.kind.clone(),
            server_url: self.server_url.clone(),
            server_name: self.server_name.clone(),
            login_email: self.login_email.clone(),
            profile_id: self.profile_id.clone(),
            profile_name: self.profile_name.clone(),
            logged_in: self.active(),
        }
    }
}

// ---------------------------------------------------------------------------
// 多账户存储：accounts.json（列表 + 活跃下标）
//
// 离线模式不进列表——它就是"没有账户"的兜底（用户名在 settings 里），
// 列表里只放皮肤站/微软这类有凭据的账户。切换到离线 = active 置空。
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AccountStore {
    pub accounts: Vec<Account>,
    /// 当前活跃账户的下标；None = 离线模式
    pub active: Option<usize>,
}

/// 给前端的状态视图（脱敏）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthStatusView {
    pub accounts: Vec<AccountView>,
    pub active_index: Option<usize>,
}

impl AccountStore {
    fn file(paths: &Paths) -> std::path::PathBuf {
        paths.root.join("accounts.json")
    }

    /// 读 + 旧版单账户文件（account.json）自动迁移。
    pub fn load(paths: &Paths) -> Self {
        let mut store: AccountStore = std::fs::read(Self::file(paths))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();

        // 迁移：0.03 时代是单账户 account.json
        if !std::fs::read(Self::file(paths))
            .map(|b| !b.is_empty())
            .unwrap_or(false)
        {
            let legacy = Account::load(paths);
            if legacy.active() {
                store.accounts = vec![legacy.clone()];
                store.active = Some(0);
                let _ = store.save(paths);
                // 旧文件改名留档，不再被读
                let _ = std::fs::rename(
                    paths.root.join("account.json"),
                    paths.root.join("account.json.migrated"),
                );
            }
        }
        store.repair();
        store
    }

    pub fn save(&self, paths: &Paths) -> Result<()> {
        paths.ensure()?;
        let bytes = serde_json::to_vec_pretty(self)?;
        std::fs::write(Self::file(paths), bytes)?;
        Ok(())
    }

    /// 下标越界（手动编辑/删除账户后）时收敛到合法范围。
    fn repair(&mut self) {
        if self.accounts.is_empty() {
            self.active = None;
        } else if let Some(index) = self.active {
            if index >= self.accounts.len() {
                self.active = Some(self.accounts.len() - 1);
            }
        }
    }

    /// 当前活跃账户（None 时返回离线兜底）。
    pub fn active_account(&self) -> Account {
        match self.active.and_then(|i| self.accounts.get(i)) {
            Some(account) => account.clone(),
            None => Account::default(),
        }
    }

    /// 插入或更新一个账户并设为活跃。同一账户（kind + 服务器 + 角色）只留一份。
    /// 返回它在列表里的下标。
    pub fn upsert_active(&mut self, account: Account) -> usize {
        let index = self
            .accounts
            .iter()
            .position(|a| {
                a.kind == account.kind
                    && a.server_url == account.server_url
                    && a.profile_id == account.profile_id
            })
            .unwrap_or_else(|| {
                self.accounts.push(account.clone());
                self.accounts.len() - 1
            });
        self.accounts[index] = account;
        self.active = Some(index);
        index
    }

    /// 移除活跃账户，活跃自动落到列表剩余的最后一个（空了就是离线）。
    pub fn remove_active(&mut self) {
        if let Some(index) = self.active {
            if index < self.accounts.len() {
                self.accounts.remove(index);
            }
        }
        self.repair();
        if self.accounts.is_empty() {
            self.active = None;
        }
    }

    pub fn view(&self) -> AuthStatusView {
        AuthStatusView {
            accounts: self.accounts.iter().map(|a| a.view()).collect(),
            active_index: self.active,
        }
    }
}

/// 给前端的账户视图（脱敏：没有 access_token）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountView {
    pub kind: String,
    pub server_url: String,
    pub server_name: String,
    pub login_email: String,
    pub profile_id: String,
    pub profile_name: String,
    pub logged_in: bool,
}

/// 皮肤站元数据里前端要用的部分。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerInfo {
    /// 规范化后的 yggdrasil 根
    pub url: String,
    pub name: String,
}

/// 一个可选的游戏角色。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileChoice {
    pub id: String,
    pub name: String,
}

/// 登录结果。`profiles` 非空表示账号下有多个角色、还没选，
/// 前端列出后调 [`super::super::commands::auth_select_profile`] 完成。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginOutcome {
    pub account: AccountView,
    pub profiles: Vec<ProfileChoice>,
}

/// 启动注入需要的东西。
#[derive(Debug, Clone)]
pub struct Injector {
    /// authlib-injector jar 的本地路径
    pub jar: PathBuf,
    /// yggdrasil 根，`-javaagent:` 的等号后面就是它
    pub server_url: String,
}

// ---------------------------------------------------------------------------
// 服务器地址与元数据
// ---------------------------------------------------------------------------

/// 把用户输入统一成 yggdrasil API 根。
///
/// 三种写法都收：`littleskin.cn`、`https://littleskin.cn`、
/// `https://littleskin.cn/api/yggdrasil`。裸域名按惯例补 `/api/yggdrasil`
/// （authlib-injector 生态的标准部署位置）；已经带路径的自定义部署尊重原样。
pub fn normalize_server_url(input: &str) -> Result<String> {
    let text = input.trim();
    if text.is_empty() {
        return Err(Error::msg("请填写皮肤站地址"));
    }
    let text = if text.starts_with("http://") || text.starts_with("https://") {
        text.to_string()
    } else {
        format!("https://{text}")
    };
    let text = text.trim_end_matches('/').to_string();
    if text.contains("api/yggdrasil") {
        return Ok(text);
    }
    let after_scheme = text.split("://").nth(1).unwrap_or(&text);
    if after_scheme.contains('/') {
        Ok(text)
    } else {
        Ok(format!("{text}/api/yggdrasil"))
    }
}

#[derive(Debug, Deserialize)]
struct ServerMetadata {
    #[serde(rename = "serverName", default)]
    server_name: String,
}

/// 拉皮肤站元数据，顺便验证地址可用。前端"测试连接"和登录前都会走这里。
pub async fn server_info(client: &reqwest::Client, server_url: &str) -> Result<ServerInfo> {
    let url = normalize_server_url(server_url)?;
    let response = client
        .get(&url)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| Error::msg(format!("连不上皮肤站：{e}")))?;
    if !response.status().is_success() {
        return Err(Error::msg(format!(
            "皮肤站返回了 HTTP {}，这不是一个可用的 yggdrasil 接口",
            response.status()
        )));
    }
    let meta: ServerMetadata = response
        .json()
        .await
        .map_err(|_| Error::msg("这个地址不像是皮肤站的 yggdrasil 接口"))?;
    let name = if meta.server_name.trim().is_empty() {
        url.split("//")
            .nth(1)
            .and_then(|rest| rest.split('/').next())
            .unwrap_or("皮肤站")
            .to_string()
    } else {
        meta.server_name
    };
    Ok(ServerInfo { url, name })
}

// ---------------------------------------------------------------------------
// 登录 / 续期 / 登出
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct AuthPayload<'a> {
    agent: Agent,
    username: &'a str,
    password: &'a str,
}

#[derive(Serialize)]
struct Agent {
    name: &'static str,
    /// yggdrasil 规范要求 agent 带 version，固定 1
    version: u32,
}

#[derive(Debug, Deserialize)]
struct AuthResponse {
    #[serde(rename = "accessToken", default)]
    access_token: String,
    #[serde(rename = "availableProfiles", default)]
    available_profiles: Vec<ProfileChoice>,
    #[serde(rename = "selectedProfile", default)]
    selected_profile: Option<ProfileChoice>,
}

#[derive(Debug, Deserialize)]
struct ApiError {
    #[serde(rename = "errorMessage", default)]
    message: String,
}

/// 把 yggdrasil 的错误响应翻译成人话。错误体的 `errorMessage` 是服务端
/// 自己写的（LittleSkin 是中文），优先用它。
async fn api_error(response: reqwest::Response) -> Error {
    let status = response.status();
    // yggdrasil 规范：错误信息也会通过这个响应头传递——错误体不是 JSON
    // （反代 502 的 HTML、限流页）时，它是唯一的信息来源
    let header_message = response
        .headers()
        .get("X-Auth-ErrorMessage")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let text = response.text().await.unwrap_or_default();
    let server_message = serde_json::from_str::<ApiError>(&text)
        .ok()
        .map(|e| e.message)
        .filter(|m| !m.trim().is_empty());
    let fallback = match status.as_u16() {
        400 | 403 => "邮箱或密码不对",
        404 => "皮肤站没有这个接口，可能不是标准 yggdrasil 部署",
        429 => "尝试次数太多，等几分钟再试",
        _ => "皮肤站请求失败",
    };
    match server_message.or(header_message) {
        Some(message) => Error::msg(format!("{message}（HTTP {status}）")),
        None => Error::msg(format!("{fallback}（HTTP {status}）")),
    }
}

/// 登录。成功就把账户写进 account.json（密码只活在这一屏）。
/// 全程持账户锁：登录的「读 → 网络 → 写」期间不许有别的写者。
pub async fn login(
    state: &AppState,
    server_url: &str,
    email: &str,
    password: &str,
) -> Result<LoginOutcome> {
    let _guard = state.account_lock.lock().await;
    let client = &state.client;
    let paths = &state.paths;
    let email = email.trim();
    if email.is_empty() || password.is_empty() {
        return Err(Error::msg("请填写皮肤站的邮箱和密码"));
    }
    let info = server_info(client, server_url).await?;

    let response = client
        .post(format!("{}/authserver/authenticate", info.url))
        .timeout(Duration::from_secs(15))
        .json(&AuthPayload {
            agent: Agent {
                name: "Minecraft",
                version: 1,
            },
            username: email,
            password,
        })
        .send()
        .await
        .map_err(|e| Error::msg(format!("连不上皮肤站：{e}")))?;
    if !response.status().is_success() {
        return Err(api_error(response).await);
    }
    let body: AuthResponse = response
        .json()
        .await
        .map_err(|_| Error::msg("皮肤站返回的登录结果看不懂"))?;
    if body.access_token.is_empty() {
        return Err(Error::msg("皮肤站没有返回令牌，登录没有生效"));
    }

    let mut account = Account {
        kind: KIND_AUTHLIB.to_string(),
        server_url: info.url,
        server_name: info.name,
        login_email: email.to_string(),
        access_token: body.access_token,
        profile_id: String::new(),
        profile_name: String::new(),
        ms_refresh_token: String::new(),
    };

    // 单角色（或服务端已指定）：直接选中，玩家少点一步
    let profiles = body.available_profiles;
    let selected = body.selected_profile.or_else(|| {
        if profiles.len() == 1 {
            profiles.first().cloned()
        } else {
            None
        }
    });
    match selected {
        Some(profile) => {
            account.profile_id = crate::engine::skin::normalize_uuid(&profile.id);
            account.profile_name = profile.name;
            let mut store = AccountStore::load(paths);
            store.upsert_active(account.clone());
            store.save(paths)?;
            Ok(LoginOutcome {
                account: account.view(),
                profiles: Vec::new(),
            })
        }
        None if profiles.is_empty() => {
            // token 已经到手但没有角色，也照常保存——账号本身没问题，
            // 只是得去皮肤站注册一个角色再回来刷新
            let mut store = AccountStore::load(paths);
            store.upsert_active(account.clone());
            store.save(paths)?;
            Err(Error::msg(
                "登录成功，但这个账号还没有游戏角色。先去皮肤站创建一个角色",
            ))
        }
        None => {
            // 多角色：先存 token，选完角色再补 profile 字段
            let mut store = AccountStore::load(paths);
            store.upsert_active(account.clone());
            store.save(paths)?;
            Ok(LoginOutcome {
                account: account.view(),
                profiles,
            })
        }
    }
}

/// 多角色登录后的第二步：把选中的角色补进已保存的 token 会话里。
pub async fn select_profile(
    state: &AppState,
    profile_id: &str,
    profile_name: &str,
) -> Result<AccountView> {
    let _guard = state.account_lock.lock().await;
    let mut store = AccountStore::load(&state.paths);
    let mut account = store.active_account();
    if account.kind != KIND_AUTHLIB || account.access_token.is_empty() {
        return Err(Error::msg("还没有登录皮肤站"));
    }
    if profile_id.trim().is_empty() {
        return Err(Error::msg("请先选择一个角色"));
    }
    account.profile_id = crate::engine::skin::normalize_uuid(profile_id);
    account.profile_name = profile_name.trim().to_string();
    if account.profile_name.is_empty() {
        return Err(Error::msg("角色名不能为空"));
    }
    let index = store.upsert_active(account.clone());
    store.save(&state.paths)?;
    let _ = index;
    Ok(account.view())
}

/// 登出：移除当前活跃账户（尽力通知皮肤站作废 token，失败不拦）。
/// 列表里还有别的账户就自动切过去，空了回到离线模式。
pub async fn logout(state: &AppState) -> Result<()> {
    let _guard = state.account_lock.lock().await;
    let mut store = AccountStore::load(&state.paths);
    let account = store.active_account();
    if account.kind == KIND_AUTHLIB && !account.access_token.is_empty() {
        let _ = state
            .client
            .post(format!(
                "{}/authserver/invalidate",
                account.server_url
            ))
            .timeout(Duration::from_secs(10))
            .json(&json!({ "accessToken": account.access_token }))
            .send()
            .await;
    }
    store.remove_active();
    store.save(&state.paths)?;
    Ok(())
}

/// 启动前确认 token 还能用，不能就续。离线账户原样返回。
/// 持账户锁跑完整个「读 → validate/refresh → 写回」事务。
pub async fn ensure_token(state: &AppState) -> Result<Account> {
    let _guard = state.account_lock.lock().await;
    let mut store = AccountStore::load(&state.paths);
    let account = store.active_account();
    // ensure_microsoft 也在锁内跑（本函数已持锁，不能重入）
    if account.kind == KIND_MICROSOFT {
        return ensure_microsoft(state, &mut store, account).await;
    }
    if account.kind != KIND_AUTHLIB {
        return Ok(account);
    }
    if account.access_token.is_empty() || account.profile_id.is_empty() {
        return Err(Error::msg(
            "皮肤站账号还没有完成登录（缺角色），请到首页重新登录",
        ));
    }

    let client = &state.client;
    let root = &account.server_url;

    // validate：204 = 还有效
    let validate = client
        .post(format!("{root}/authserver/validate"))
        .timeout(Duration::from_secs(10))
        .json(&json!({ "accessToken": account.access_token }))
        .send()
        .await;
    if matches!(&validate, Ok(resp) if resp.status().is_success()) {
        return Ok(account);
    }

    // refresh：换新 token 并回写
    let refresh = client
        .post(format!("{root}/authserver/refresh"))
        .timeout(Duration::from_secs(15))
        .json(&json!({
            "accessToken": account.access_token,
            "selectedProfile": {
                "id": account.profile_id.replace('-', ""),
                "name": account.profile_name,
            }
        }))
        .send()
        .await
        .map_err(|_| Error::msg("连不上皮肤站，无法验证登录状态。请检查网络后再启动"))?;
    if !refresh.status().is_success() {
        return Err(Error::msg(
            "皮肤站登录已过期，请到首页重新登录一次",
        ));
    }
    let body: AuthResponse = refresh
        .json()
        .await
        .map_err(|_| Error::msg("皮肤站的续期结果看不懂，请重新登录"))?;
    if body.access_token.is_empty() {
        return Err(Error::msg("皮肤站没有发回新令牌，请重新登录"));
    }
    let mut renewed = account;
    renewed.access_token = body.access_token;
    store.upsert_active(renewed.clone());
    store.save(&state.paths)?;
    Ok(renewed)
}

// ---------------------------------------------------------------------------
// authlib-injector jar：下载与缓存
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct InjectorArtifact {
    /// 真实接口（实测 2026-10）返回的是单数 `download_url`；
    /// 旧版接口曾是 `urls` 数组，两个都认
    #[serde(default)]
    download_url: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    urls: Vec<String>,
    #[serde(default)]
    checksums: Option<Checksums>,
    /// 有的元数据把 sha256 放在顶层，两种都认
    #[serde(default)]
    sha256: String,
}

#[derive(Debug, Deserialize)]
struct Checksums {
    #[serde(default)]
    sha256: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct JarRecord {
    version: String,
    file: String,
    sha256: String,
}

impl Default for JarRecord {
    fn default() -> Self {
        Self {
            version: String::new(),
            file: String::new(),
            sha256: String::new(),
        }
    }
}

fn injector_dir(paths: &Paths) -> PathBuf {
    paths.cache().join("authlib-injector")
}

/// 确保 authlib-injector jar 就位，返回它的本地路径。
///
/// 缓存语义：jar 是可重建资源，落 `cache/authlib-injector/`。
/// 记录文件 `current.json` 记住已下载的版本，文件还在就直接用——
/// 不必每次启动都去问一遍上游（jar 才 200 KB，但网络不是每次都通）。
pub async fn ensure_jar(state: &AppState) -> Result<PathBuf> {
    let dir = injector_dir(&state.paths);
    let record: JarRecord = std::fs::read(dir.join("current.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    if !record.file.is_empty() {
        let jar = dir.join(&record.file);
        if jar.is_file() {
            return Ok(jar);
        }
    }

    // 元数据 → 下载 → sha256 校验 → 原子落盘
    let meta: InjectorArtifact = state
        .client
        .get(ARTIFACT_API)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| Error::msg(format!("拿不到 authlib-injector 的版本信息：{e}")))?
        .json()
        .await
        .map_err(|_| Error::msg("authlib-injector 的版本信息看不懂"))?;

    let url = if !meta.download_url.is_empty() {
        meta.download_url.clone()
    } else {
        meta.urls
            .first()
            .cloned()
            .ok_or_else(|| Error::msg("authlib-injector 的元数据里没有下载地址"))?
    };
    let expected_sha = meta
        .checksums
        .map(|c| c.sha256)
        .filter(|s| !s.is_empty())
        .or_else(|| if meta.sha256.is_empty() { None } else { Some(meta.sha256) });

    let bytes = state
        .client
        .get(url)
        .timeout(Duration::from_secs(60))
        .send()
        .await
        .map_err(|e| Error::msg(format!("下载 authlib-injector 失败：{e}")))?
        .error_for_status()
        .map_err(|e| Error::msg(format!("下载 authlib-injector 失败：{e}")))?
        .bytes()
        .await
        .map_err(|e| Error::msg(format!("下载 authlib-injector 失败：{e}")))?;

    let version = if meta.version.trim().is_empty() {
        "unknown".to_string()
    } else {
        meta.version.trim().to_string()
    };
    let file = format!("authlib-injector-{version}.jar");

    if let Some(expected) = expected_sha.clone().filter(|s| !s.is_empty()) {
        use sha2::Digest;
        let mut hasher = sha2::Sha256::new();
        hasher.update(&bytes);
        let actual: String = hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        if !actual.eq_ignore_ascii_case(&expected) {
            return Err(Error::msg(
                "下载的 authlib-injector 校验不符，可能被篡改或下载不完整，请重试",
            ));
        }
    }

    std::fs::create_dir_all(&dir)?;
    let temp = dir.join(format!("{file}.tmp"));
    std::fs::write(&temp, &bytes)?;
    let jar = dir.join(&file);
    std::fs::rename(&temp, &jar)?;
    let record = JarRecord {
        version,
        file,
        sha256: expected_sha.clone().unwrap_or_default(),
    };
    let _ = std::fs::write(
        dir.join("current.json"),
        serde_json::to_vec_pretty(&record).unwrap_or_default(),
    );
    Ok(jar)
}

/// 账户是皮肤站模式时，把注入所需的 jar + 地址备好；离线返回 `None`。
pub async fn prepare_injector(state: &AppState, account: &Account) -> Result<Option<Injector>> {
    if account.kind != KIND_AUTHLIB {
        return Ok(None);
    }
    let jar = ensure_jar(state).await?;
    Ok(Some(Injector {
        jar,
        server_url: account.server_url.clone(),
    }))
}

// ---------------------------------------------------------------------------
// 微软正版登录（设备码流程）
//
// 完整链条（Minecraft Wiki "Microsoft authentication" 公开文档）：
//   设备码 → 微软 token → XBL → XSTS → Minecraft 服务 → 角色档案
// 不需要回调地址和本地端口：启动器只显示一个码，玩家去
// microsoft.com/link 输入，轮询 token 端点直到授权完成。
// 唯一的前置条件是下面这个 client_id。
// ---------------------------------------------------------------------------

/// Azure 应用的 client_id。
///
/// 自行注册（免费，5 分钟）：
/// 1. portal.azure.com → Microsoft Entra ID → 应用注册 → 新注册；
/// 2. 受支持的账户类型选「个人 Microsoft 账户」；
/// 3. 重定向 URI 留空（设备码流程不需要）；
/// 4. 应用「身份验证」里把「允许公共客户端流」设为「是」；
/// 5. 把「应用程序(客户端) ID」填到这里。
/// 每个启动器必须用自己的 client_id，不要借用其他启动器的。
pub const MS_CLIENT_ID: &str = "";

const MS_DEVICECODE_API: &str =
    "https://login.microsoftonline.com/consumers/oauth2/v2.0/devicecode";
const MS_TOKEN_API: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/token";
const MS_SCOPE: &str = "XboxLive.signin offline_access";
const XBL_API: &str = "https://user.auth.xboxlive.com/user/authenticate";
const XSTS_API: &str = "https://xsts.auth.xboxlive.com/xsts/authorize";
const MC_LOGIN_API: &str = "https://api.minecraftservices.com/authentication/login_with_xbox";
const MC_PROFILE_API: &str = "https://api.minecraftservices.com/minecraft/profile";

/// 设备码流程第一步的结果：把 `user_code` 展示给玩家，
/// 让他打开 `verification_uri` 输入，然后前端按 `interval` 秒轮询。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceCodeStart {
    pub user_code: String,
    pub verification_uri: String,
    pub device_code: String,
    pub interval: u64,
    pub expires_in: u64,
}

/// 轮询结果：还没授权（继续轮）、被要求放慢（间隔 +5s 再轮）、已完成（账户就绪）。
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum MsPoll {
    Pending,
    SlowDown,
    Done { account: AccountView },
}

/// 发起设备码流程。
pub async fn ms_begin(client: &reqwest::Client) -> Result<DeviceCodeStart> {
    if MS_CLIENT_ID.trim().is_empty() {
        return Err(Error::msg(
            "还没有配置微软登录的 client_id。到 portal.azure.com 注册一个应用（个人账户类型、\
             打开「允许公共客户端流」），把客户端 ID 填进 auth.rs 的 MS_CLIENT_ID",
        ));
    }
    let response = client
        .post(MS_DEVICECODE_API)
        .timeout(Duration::from_secs(15))
        .form(&[("client_id", MS_CLIENT_ID), ("scope", MS_SCOPE)])
        .send()
        .await
        .map_err(|e| Error::msg(format!("连不上微软的授权服务：{e}")))?;
    if !response.status().is_success() {
        return Err(Error::msg(format!(
            "微软授权服务返回 HTTP {}（多半是 client_id 不对，或者没开「允许公共客户端流」）",
            response.status()
        )));
    }
    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|_| Error::msg("微软设备码响应看不懂"))?;
    parse_device_code(&body)
}

/// 解析设备码响应（拆出来给单测）。
fn parse_device_code(body: &serde_json::Value) -> Result<DeviceCodeStart> {
    let get = |key: &str| body.get(key).and_then(|v| v.as_str()).unwrap_or("");
    let user_code = get("user_code");
    let device_code = get("device_code");
    if user_code.is_empty() || device_code.is_empty() {
        return Err(Error::msg("微软没有返回设备码，请重试"));
    }
    Ok(DeviceCodeStart {
        user_code: user_code.to_string(),
        verification_uri: body
            .get("verification_uri")
            .and_then(|v| v.as_str())
            .or_else(|| body.get("verification_url").and_then(|v| v.as_str()))
            .unwrap_or("https://www.microsoft.com/link")
            .to_string(),
        device_code: device_code.to_string(),
        interval: body.get("interval").and_then(|v| v.as_u64()).unwrap_or(5),
        expires_in: body.get("expires_in").and_then(|v| v.as_u64()).unwrap_or(900),
    })
}

/// 轮询一次设备码。授权中返回 `Pending`，完成则走完整条换票链并保存账户。
pub async fn ms_poll(state: &AppState, device_code: &str) -> Result<MsPoll> {
    if MS_CLIENT_ID.trim().is_empty() {
        return Err(Error::msg("还没有配置微软登录的 client_id"));
    }
    let response = state
        .client
        .post(MS_TOKEN_API)
        .timeout(Duration::from_secs(15))
        .form(&[
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ("client_id", MS_CLIENT_ID),
            ("device_code", device_code),
        ])
        .send()
        .await
        .map_err(|e| Error::msg(format!("连不上微软的授权服务：{e}")))?;

    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|_| Error::msg("微软令牌响应看不懂"))?;
    let error = body.get("error").and_then(|v| v.as_str()).unwrap_or("");
    match error {
        // 还没在浏览器里确认：前端继续轮
        "authorization_pending" => return Ok(MsPoll::Pending),
        // RFC 8628：收到 slow_down 要把轮询间隔加长（前端 +5s）
        "slow_down" => return Ok(MsPoll::SlowDown),
        "expired_token" => return Err(Error::msg("设备码过期了，请重新开始登录")),
        "authorization_declined" => return Err(Error::msg("你在浏览器里拒绝了授权")),
        "" => {}
        other => {
            let desc = body
                .get("error_description")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            return Err(Error::msg(format!(
                "微软授权失败（{other}）：{desc}"
            )));
        }
    }

    let ms_token = body
        .get("access_token")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let refresh_token = body
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if ms_token.is_empty() {
        return Err(Error::msg("微软没有返回令牌，请重试"));
    }

    // 从换票到保存是「写账户」的临界区：轮询本身（上面的 pending 分支）
    // 不持锁，免得每次轮询都把登录/登出堵上十几秒
    let _guard = state.account_lock.lock().await;
    let (mc_token, profile_id, profile_name) =
        ms_exchange_chain(&state.client, &ms_token).await?;

    let account = Account {
        kind: KIND_MICROSOFT.to_string(),
        access_token: mc_token,
        profile_id: crate::engine::skin::normalize_uuid(&profile_id),
        profile_name: profile_name.clone(),
        ms_refresh_token: refresh_token,
        ..Account::default()
    };
    let mut store = AccountStore::load(&state.paths);
    store.upsert_active(account.clone());
    store.save(&state.paths)?;
    Ok(MsPoll::Done {
        account: account.view(),
    })
}

/// 解析 XBL / XSTS 响应里的 token 和 uhs（拆出来给单测）。
fn parse_xbox_token(body: &serde_json::Value) -> Result<(String, String)> {
    let token = body
        .get("Token")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let uhs = body
        .get("DisplayClaims")
        .and_then(|c| c.get("xui"))
        .and_then(|x| x.get(0))
        .and_then(|e| e.get("uhs"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if token.is_empty() || uhs.is_empty() {
        return Err(Error::msg("Xbox 服务的响应里没有令牌"));
    }
    Ok((token, uhs))
}

/// 微软 token → XBL → XSTS → Minecraft token → 档案。
/// 返回 `(MC access_token, 角色UUID(无连字符), 角色名)`。
async fn ms_exchange_chain(
    client: &reqwest::Client,
    ms_token: &str,
) -> Result<(String, String, String)> {
    // 1) Xbox Live
    let xbl = client
        .post(XBL_API)
        .timeout(Duration::from_secs(15))
        .json(&json!({
            "Properties": {
                "AuthMethod": "RPS",
                "SiteName": "user.auth.xboxlive.com",
                "RpsTicket": format!("d={ms_token}"),
            },
            "RelyingParty": "http://auth.xboxlive.com",
            "TokenType": "JWT"
        }))
        .send()
        .await
        .map_err(|e| Error::msg(format!("连不上 Xbox Live：{e}")))?;
    if !xbl.status().is_success() {
        return Err(Error::msg(format!(
            "Xbox Live 认证失败（HTTP {}）",
            xbl.status()
        )));
    }
    let xbl_body: serde_json::Value = xbl
        .json()
        .await
        .map_err(|_| Error::msg("Xbox Live 的响应看不懂"))?;
    let (xbl_token, _uhs) = parse_xbox_token(&xbl_body)?;

    // 2) XSTS。被拒时响应带 XErr 数字码，常见的两种直接翻译。
    // RelyingParty 必须是 Minecraft 服务的 RP——用 xboxlive.com 的话 token
    // 的 audience 不对，login_with_xbox 会 401（minecraft.wiki 的认证文档）。
    let xsts = client
        .post(XSTS_API)
        .timeout(Duration::from_secs(15))
        .json(&json!({
            "Properties": {
                "SandboxId": "RETAIL",
                "UserTokens": [xbl_token],
            },
            "RelyingParty": "rp://api.minecraftservices.com/",
            "TokenType": "JWT"
        }))
        .send()
        .await
        .map_err(|e| Error::msg(format!("连不上 XSTS 服务：{e}")))?;
    if !xsts.status().is_success() {
        let status = xsts.status();
        let body: serde_json::Value = xsts.json().await.unwrap_or_default();
        let xerr = body.get("XErr").and_then(|v| v.as_i64()).unwrap_or(0);
        return Err(Error::msg(match xerr {
            2148916233 => "这个微软账号还没有 Xbox 资料页。先去 xbox.com 用该账号登录一次创建档案"
                .to_string(),
            2148916238 => {
                "这是儿童账号，无法访问 Minecraft（需要家长在家庭组里放行）".to_string()
            }
            _ => format!("XSTS 授权失败（HTTP {status}）"),
        }));
    }
    let xsts_body: serde_json::Value = xsts
        .json()
        .await
        .map_err(|_| Error::msg("XSTS 的响应看不懂"))?;
    let (xsts_token, uhs) = parse_xbox_token(&xsts_body)?;

    // 3) Minecraft 服务换票
    let mc = client
        .post(MC_LOGIN_API)
        .timeout(Duration::from_secs(15))
        .json(&json!({
            "identityToken": format!("XBL3.0 x={uhs};{xsts_token}")
        }))
        .send()
        .await
        .map_err(|e| Error::msg(format!("连不上 Minecraft 服务：{e}")))?;
    if !mc.status().is_success() {
        return Err(Error::msg(format!(
            "Minecraft 服务换票失败（HTTP {}）",
            mc.status()
        )));
    }
    let mc_body: serde_json::Value = mc
        .json()
        .await
        .map_err(|_| Error::msg("Minecraft 服务的响应看不懂"))?;
    let mc_token = mc_body
        .get("access_token")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if mc_token.is_empty() {
        return Err(Error::msg("Minecraft 服务没有返回令牌"));
    }

    // 4) 档案。404 = 没买 Java 版
    let profile = client
        .get(MC_PROFILE_API)
        .timeout(Duration::from_secs(15))
        .bearer_auth(&mc_token)
        .send()
        .await
        .map_err(|e| Error::msg(format!("拿不到角色档案：{e}")))?;
    if profile.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(Error::msg(
            "这个微软账号没有 Minecraft: Java 版。请先购买，或改用离线/皮肤站模式",
        ));
    }
    if !profile.status().is_success() {
        return Err(Error::msg(format!(
            "拿不到角色档案（HTTP {}）",
            profile.status()
        )));
    }
    let profile_body: serde_json::Value = profile
        .json()
        .await
        .map_err(|_| Error::msg("角色档案的响应看不懂"))?;
    let profile_id = profile_body
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let profile_name = profile_body
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if profile_id.is_empty() || profile_name.is_empty() {
        return Err(Error::msg("角色档案里缺少 UUID 或角色名"));
    }
    Ok((mc_token, profile_id, profile_name))
}

/// 微软账户的启动前续期：先试现有 MC token（24 小时有效），失效就用
/// refresh_token 重走一遍换票链。密码从不参与——refresh_token 活得比会话久。
/// 调用方（ensure_token）已持账户锁并传入 store，写回直接进列表。
async fn ensure_microsoft(
    state: &AppState,
    store: &mut AccountStore,
    account: Account,
) -> Result<Account> {
    if account.access_token.is_empty() || account.profile_id.is_empty() {
        return Err(Error::msg(
            "微软正版账号还没有完成登录，请到首页重新登录",
        ));
    }
    if account.ms_refresh_token.is_empty() {
        return Err(Error::msg(
            "账户缺少刷新令牌，请到首页退出后重新登录一次微软账号",
        ));
    }

    // 现有 token 还活着就什么都不用做
    let probe = state
        .client
        .get(MC_PROFILE_API)
        .timeout(Duration::from_secs(10))
        .bearer_auth(&account.access_token)
        .send()
        .await;
    if matches!(&probe, Ok(resp) if resp.status().is_success()) {
        return Ok(account);
    }

    // 刷新链：MS refresh_token → 新 MS token → XBL → XSTS → MC
    let refreshed = state
        .client
        .post(MS_TOKEN_API)
        .timeout(Duration::from_secs(15))
        .form(&[
            ("grant_type", "refresh_token"),
            ("client_id", MS_CLIENT_ID),
            ("refresh_token", account.ms_refresh_token.as_str()),
            ("scope", MS_SCOPE),
        ])
        .send()
        .await
        .map_err(|_| Error::msg("连不上微软的授权服务，无法续期登录。请检查网络后再启动"))?;
    if !refreshed.status().is_success() {
        return Err(Error::msg(
            "微软登录已过期，请到首页重新登录一次",
        ));
    }
    let body: serde_json::Value = refreshed
        .json()
        .await
        .map_err(|_| Error::msg("微软续期的响应看不懂，请重新登录"))?;
    let ms_token = body
        .get("access_token")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let refresh_token = body
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .unwrap_or(&account.ms_refresh_token)
        .to_string();
    if ms_token.is_empty() {
        return Err(Error::msg("微软没有返回新令牌，请重新登录"));
    }

    let (mc_token, _profile_id, _profile_name) = ms_exchange_chain(&state.client, &ms_token).await?;
    let mut renewed = account;
    renewed.access_token = mc_token;
    renewed.ms_refresh_token = refresh_token;
    store.upsert_active(renewed.clone());
    store.save(&state.paths)?;
    Ok(renewed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_urls_are_normalized() {
        assert_eq!(
            normalize_server_url("littleskin.cn").unwrap(),
            "https://littleskin.cn/api/yggdrasil"
        );
        assert_eq!(
            normalize_server_url("https://littleskin.cn").unwrap(),
            "https://littleskin.cn/api/yggdrasil"
        );
        assert_eq!(
            normalize_server_url("https://littleskin.cn/api/yggdrasil/").unwrap(),
            "https://littleskin.cn/api/yggdrasil"
        );
        // 已带路径的自定义部署尊重原样
        assert_eq!(
            normalize_server_url("http://localhost:8080/yggdrasil").unwrap(),
            "http://localhost:8080/yggdrasil"
        );
        assert!(normalize_server_url("  ").is_err());
    }

    #[test]
    fn parses_authenticate_response() {
        let body = r#"{
            "accessToken": "token123",
            "availableProfiles": [
                { "id": "069a79f444e94726a5befca90e38aaf5", "name": "Notch" },
                { "id": "aaaa79f444e94726a5befca90e38aaf5", "name": "Second" }
            ]
        }"#;
        let parsed: AuthResponse = serde_json::from_str(body).unwrap();
        assert_eq!(parsed.access_token, "token123");
        assert_eq!(parsed.available_profiles.len(), 2);
        assert!(parsed.selected_profile.is_none());

        // 单角色 + 已选中的写法
        let single: AuthResponse = serde_json::from_str(
            r#"{"accessToken":"t","selectedProfile":{"id":"069a79f444e94726a5befca90e38aaf5","name":"Notch"}}"#,
        )
        .unwrap();
        assert_eq!(single.selected_profile.unwrap().name, "Notch");
    }

    #[test]
    fn parses_error_message_from_server() {
        // 同步的 serde 检查：服务端的 errorMessage 要被读出来
        let parsed: std::result::Result<ApiError, _> =
            serde_json::from_str(r#"{"error":"ForbiddenOperationException","errorMessage":"Invalid credentials"}"#);
        assert_eq!(parsed.unwrap().message, "Invalid credentials");
    }

    #[test]
    fn parses_injector_metadata() {
        // 2026-10 实测的真实响应（字段是 download_url 单字符串 + snake_case）
        let real: InjectorArtifact = serde_json::from_str(
            r#"{"build_number":56,"version":"1.2.8",
                "download_url":"https://authlib-injector.yushi.moe/artifact/56/authlib-injector-1.2.8.jar",
                "checksums":{"sha256":"9c7f4343e6c82034958ffb48c14a2cb0c85928be7283103ce17da00c6d5a7b10"}}"#,
        )
        .unwrap();
        assert_eq!(real.version, "1.2.8");
        assert!(real.download_url.ends_with("authlib-injector-1.2.8.jar"));
        assert_eq!(
            real.checksums.unwrap().sha256,
            "9c7f4343e6c82034958ffb48c14a2cb0c85928be7283103ce17da00c6d5a7b10"
        );

        // 顶层 sha256 的旧格式也认
        let top_level: InjectorArtifact = serde_json::from_str(
            r#"{"version":"1.3.0","download_url":"https://example.com/b.jar","sha256":"def456"}"#,
        )
        .unwrap();
        assert_eq!(top_level.sha256, "def456");
    }

    #[test]
    fn parses_device_code_response() {
        let body: serde_json::Value = serde_json::from_str(
            r#"{"device_code":"DEV123","user_code":"ABCD-EFGH","verification_uri":"https://www.microsoft.com/link","interval":5,"expires_in":900}"#,
        )
        .unwrap();
        let parsed = parse_device_code(&body).unwrap();
        assert_eq!(parsed.user_code, "ABCD-EFGH");
        assert_eq!(parsed.device_code, "DEV123");
        assert_eq!(parsed.interval, 5);
    }

    #[test]
    fn parses_xbox_token_chain() {
        // XBL / XSTS 的响应结构相同：Token + DisplayClaims.xui[0].uhs
        let body: serde_json::Value = serde_json::from_str(
            r#"{"IssueInstant":"2020-01-01T00:00:00Z","Token":"abc.def.ghi",
                "DisplayClaims":{"xui":[{"agt":"family","uhs":"1234567890"}]}}"#,
        )
        .unwrap();
        let (token, uhs) = parse_xbox_token(&body).unwrap();
        assert_eq!(token, "abc.def.ghi");
        assert_eq!(uhs, "1234567890");

        let empty: serde_json::Value = serde_json::from_str(r#"{"Token":"x"}"#).unwrap();
        assert!(parse_xbox_token(&empty).is_err());
    }

    #[test]
    fn account_store_upsert_dedup_and_remove() {
        let root = std::env::temp_dir().join(format!("mcl-accounts-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let paths = Paths { root: root.clone() };

        // 空库 = 离线
        let mut store = AccountStore::load(&paths);
        assert!(store.accounts.is_empty());
        assert!(store.active.is_none());
        assert_eq!(store.active_account().kind, KIND_OFFLINE);

        // 第一个账户登录 → 活跃
        let a = Account {
            kind: KIND_AUTHLIB.to_string(),
            server_url: "https://littleskin.cn/api/yggdrasil".into(),
            server_name: "LittleSkin".into(),
            login_email: "a@b.c".into(),
            access_token: "t1".into(),
            profile_id: "069a79f4-44e9-4726-a5be-fca90e38aaf5".into(),
            profile_name: "Notch".into(),
            ms_refresh_token: String::new(),
        };
        store.upsert_active(a.clone());
        // 同一账户再登录（token 换新）→ 去重更新，不新增
        let mut a2 = a.clone();
        a2.access_token = "t2".into();
        store.upsert_active(a2);
        assert_eq!(store.accounts.len(), 1);
        assert_eq!(store.accounts[0].access_token, "t2");
        assert_eq!(store.active, Some(0));

        // 第二个账户（微软）→ 活跃切到 1
        let b = Account {
            kind: KIND_MICROSOFT.to_string(),
            access_token: "ms".into(),
            profile_id: "11111111-2222-4333-8444-555555555555".into(),
            profile_name: "XboxGuy".into(),
            ms_refresh_token: "r".into(),
            ..Account::default()
        };
        store.upsert_active(b);
        assert_eq!(store.accounts.len(), 2);
        assert_eq!(store.active, Some(1));

        // 存取往返
        store.save(&paths).unwrap();
        let reloaded = AccountStore::load(&paths);
        assert_eq!(reloaded.accounts.len(), 2);
        assert_eq!(reloaded.active, Some(1));
        assert_eq!(reloaded.active_account().profile_name, "XboxGuy");

        // 移除活跃（index 1）→ 收敛到剩下的
        let mut reloaded = reloaded;
        reloaded.remove_active();
        assert_eq!(reloaded.accounts.len(), 1);
        assert_eq!(reloaded.active, Some(0));
        // 全删光 → 离线
        reloaded.remove_active();
        assert!(reloaded.accounts.is_empty());
        assert!(reloaded.active.is_none());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn legacy_single_account_migrates() {
        let root = std::env::temp_dir().join(format!("mcl-migrate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let paths = Paths { root: root.clone() };

        // 旧版单账户文件
        let legacy = r#"{
            "kind": "authlib",
            "serverUrl": "https://littleskin.cn/api/yggdrasil",
            "serverName": "LittleSkin",
            "loginEmail": "a@b.c",
            "accessToken": "tok",
            "profileId": "069a79f4-44e9-4726-a5be-fca90e38aaf5",
            "profileName": "Notch"
        }"#;
        std::fs::write(root.join("account.json"), legacy).unwrap();

        let store = AccountStore::load(&paths);
        assert_eq!(store.accounts.len(), 1, "旧账户应被迁移进列表");
        assert_eq!(store.active, Some(0));
        assert!(root.join("accounts.json").is_file());
        // 旧文件改名留档
        assert!(root.join("account.json.migrated").is_file());
        assert!(!root.join("account.json").is_file());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn account_round_trip_and_active_check() {
        let root = std::env::temp_dir().join(format!("mcl-auth-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let paths = Paths { root: root.clone() };

        // 没有旧文件时是离线
        let fresh = Account::load(&paths);
        assert_eq!(fresh.kind, KIND_OFFLINE);
        assert!(!fresh.active());

        // 旧版单账户格式能读回（迁移路径的读取端）
        let account = Account {
            kind: KIND_AUTHLIB.to_string(),
            server_url: "https://littleskin.cn/api/yggdrasil".to_string(),
            server_name: "LittleSkin".to_string(),
            login_email: "a@b.c".to_string(),
            access_token: "tok".to_string(),
            profile_id: "069a79f4-44e9-4726-a5be-fca90e38aaf5".to_string(),
            profile_name: "Notch".to_string(),
            ms_refresh_token: String::new(),
        };
        let text = serde_json::to_string(&account).unwrap();
        std::fs::write(root.join("account.json"), &text).unwrap();
        let loaded = Account::load(&paths);
        assert_eq!(loaded.login_email, "a@b.c");
        assert!(loaded.active());
        // 视图里绝不能有 token
        let view_text = serde_json::to_string(&loaded.view()).unwrap();
        assert!(!view_text.contains("tok"));

        let _ = std::fs::remove_dir_all(&root);
    }
}
