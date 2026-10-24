/** 与 Rust 侧 `#[serde(rename_all = "camelCase")]` 一一对应。 */

export type ViewKey =
  | "home"
  | "versions"
  | "netplay"
  | "downloads"
  | "modpacks"
  | "mods"
  | "settings";

export interface Settings {
  /** `bmclapi` | `official` */
  source: string;
  maxMemoryMb: number;
  minMemoryMb: number;
  /** `auto` 按 mod 数量与可用内存推荐；`manual` 用手动值 */
  ramType: string;
  /** `auto` | `g1` | `g1-optimized` | `custom` */
  gcMode: string;
  /** 留空表示自动探测 */
  javaPath: string;
  username: string;
  theme: string;
  /** 导入的离线皮肤 PNG 路径，空串表示用内置 Steve */
  skinPath: string;
}

export interface JavaInfo {
  path: string;
  major: number;
  source: string;
}

export interface RemoteVersion {
  id: string;
  kind: string;
  installed: boolean;
}

export interface VersionList {
  latestRelease: string;
  latestSnapshot: string;
  versions: RemoteVersion[];
}

export interface LocalVersion {
  id: string;
  kind: string;
  javaMajor: number;
  dir: string;
  /** json 与 jar 都在；false 表示安装中断，不能直接启动 */
  complete: boolean;
  /** 版本目录占用的字节数 */
  size: number;
  /** 这个版本的私有存档目录里有没有东西——删之前要警告 */
  hasSaves: boolean;
}

export interface DeleteReport {
  version: string;
  freed: number;
  removed: string[];
}

export type ItemState = "pending" | "running" | "done" | "failed" | "cancelled";

export interface QueueItem {
  id: string;
  label: string;
  kind: string;
  /** Tasks created by the same queue enqueue call share a group. */
  groupId: string;
  groupLabel: string;
  state: ItemState;
  total: number;
  done: number;
  error: string | null;
}

export interface ProgressEvent {
  id: string;
  label: string;
  kind: string;
  done: number;
  total: number;
}

export interface InstallReport {
  version: string;
  files: number;
  bytes: number;
  natives: number;
  /** 老版本按原名铺开的资源数，新版本为 0 */
  legacy: number;
}

export interface LaunchReport {
  pid: number;
  java: string;
  javaMajor: number;
  argCount: number;
}

export interface ModpackHit {
  id: string;
  slug: string;
  title: string;
  description: string;
  iconUrl: string;
  downloads: number;
  versions: string[];
}

export interface ImportReport {
  instance: string;
  /** 新建出来的版本 id —— 导入后切到这个版本 */
  versionId: string;
  /** 基底版本（原版号），例如 1.21.1 */
  gameVersion: string;
  /** 加载器描述，例如 `Fabric 0.16.9` / `原版` / `继承 26.3` */
  loader: string;
  dir: string;
  overrides: number;
  files: number;
  /** 目标已存在而跳过的文件数 */
  skipped: number;
  /** 需要玩家注意的事（比如这个包要 Forge、暂不能自动装） */
  note: string;
}

/** 装完加载器后的结果。 */
export interface LoaderReport {
  versionId: string;
  loader: string;
  loaderVersion: string;
  gameVersion: string;
  libraries: number;
  /** 有类库没下下来时的提示 */
  warning?: string;
}

/** 玩家头像（后端解出的皮肤信息）。 */
export interface AvatarView {
  /** 实际使用的 UUID：离线账号是按用户名派生的 */
  uuid: string;
  /** `steve` 宽手臂 / `alex` 细手臂 */
  model: "steve" | "alex";
  /** 皮肤贴图的 data URL；null 表示用内置默认皮肤（Steve） */
  image: string | null;
  /** 数据来源：offline / cache / mojang / authlib（皮肤站）/ local（玩家导入的离线皮肤） */
  source: string;
}

// ---------------------------------------------------------------------------
// 皮肤站账户（authlib-injector / yggdrasil）
// ---------------------------------------------------------------------------

/** 皮肤站元数据。 */
export interface AuthServerInfo {
  /** 规范化后的 yggdrasil 根 */
  url: string;
  name: string;
}

/** 皮肤站账号下的一个游戏角色。 */
export interface AuthProfileChoice {
  id: string;
  name: string;
}

/** 当前账户（后端已脱敏，没有 token）。 */
export interface AuthAccountView {
  kind: "offline" | "authlib" | string;
  serverUrl: string;
  serverName: string;
  loginEmail: string;
  profileId: string;
  profileName: string;
  loggedIn: boolean;
}

/** 登录结果。`profiles` 非空 = 账号下有多个角色，先选一个。 */
export interface AuthLoginResult {
  account: AuthAccountView;
  profiles: AuthProfileChoice[];
}

/** 微软设备码登录第一步：展示 code，让玩家去 verificationUri 输入。 */
export interface MsDeviceCode {
  userCode: string;
  verificationUri: string;
  deviceCode: string;
  /** 建议的轮询间隔（秒） */
  interval: number;
  /** 设备码有效期（秒） */
  expiresIn: number;
}

/** 微软设备码轮询结果：pending = 继续轮，slowDown = 间隔加 5 秒再轮，done = 登录完成。 */
export interface MsPollResult {
  state: "pending" | "slowDown" | "done";
  account?: AuthAccountView;
}

/** 后端算好的内存方案，对应 Rust 的 `memory::RamPlan` + 生效值。 */
export interface RamPlan {
  modCount: number;
  availableMb: number;
  totalMb: number;
  recommendMb: number;
  minimumMb: number;
  modable: boolean;
  reason: string;
}

export interface MemoryView {
  plan: RamPlan;
  /** 这次启动实际用的 -Xmx */
  effectiveMaxMb: number;
  /** 这次启动实际用的 -Xms */
  effectiveMinMb: number;
  /** 手动模式下设置里的值 */
  manualMaxMb: number;
  /** 物理内存 70%，超过就在界面上标红 */
  safeLimitMb: number;
  gcMode: string;
  gcArgs: string[];
}

export interface ExportOptions {
  name: string;
  includeMods: boolean;
  includeConfig: boolean;
  includeSaves: boolean;
  includeResourcepacks: boolean;
  includeShaderpacks: boolean;
}

export interface ExportReport {
  path: string;
  files: number;
  bytes: number;
}

/** 下载源测速结果。 */
export interface SourceProbe {
  /** `official` | `bmclapi` */
  source: string;
  /** 成功时是毫秒数 */
  ms: number | null;
  bytes: number;
  error: string;
}

/** 拖拽导入时单个文件的处理结果。 */
export interface ImportOutcome {
  file: string;
  /** `modpack` | `mod` | `resourcepack` | `shaderpack` | `unknown` */
  kind: string;
  ok: boolean;
  /** 成功时是落地文件名，失败时是原因 */
  message: string;
  /** 整合包新建出来的版本 id，前端据此切过去 */
  versionId: string | null;
}

/** 下载页的社区资源分类，对应 Modrinth 的 project_type。 */
export type BrowseKind = "mod" | "modpack" | "datapack" | "resourcepack" | "shader";

/** 下载页左侧/主导航里的分类：原版游戏 + 五种社区资源。 */
export type DownloadSection = "vanilla" | BrowseKind;

export interface BrowseHit {
  id: string;
  slug: string;
  title: string;
  description: string;
  iconUrl: string;
  downloads: number;
  follows: number;
  /** ISO 时间，前端自己算「几天前」 */
  updated: string;
  /** 加载器 + 功能标签（fabric / optimization …） */
  categories: string[];
  versions: string[];
  projectType: string;
  author: string;
}

export interface ProjectInstallReport {
  title: string;
  version: string;
  files: number;
  dir: string;
  instance: string | null;
  /** 自动补装的前置 mod 名字（装 mod 时才有） */
  dependencies?: string[];
}

export interface ModEntry {
  fileName: string;
  name: string;
  enabled: boolean;
  size: number;
}

export interface LogLine {
  stream: string;
  line: string;
}

export interface LogEntry extends LogLine {
  at: number;
}

export interface GameExit {
  version: string;
  code: number;
  /** 完整日志文件路径（打开日志文件夹用）；旧版后端没有这个字段 */
  logPath?: string;
  /** 崩溃初判的建议列表；正常退出时为空 */
  tips?: string[];
}

/** 正在运行的游戏进程（「结束游戏」按钮的数据源）。 */
export interface RunningGame {
  pid: number;
  version: string;
  startedAtMs: number;
}

// ---------------------------------------------------------------------------
// 联机
// ---------------------------------------------------------------------------

/** 联机页「名称 / 值」两列里的一行。 */
export interface NetplayFact {
  label: string;
  value: string;
}

/** 一条联机通路的运行时状态（Rust `netplay::Slot`）。 */
export interface NetplaySlot {
  /** idle / starting / running / error */
  state: "idle" | "starting" | "running" | "error";
  summary: string;
  /** 给朋友填进游戏的地址，空串表示还没拿到 */
  address: string;
  localPort: number;
  error: string;
  facts: NetplayFact[];
}

export type NetplayChannel = "easytier" | "portmap" | "frp";

/** 联机页一次性拉的状态。 */
export interface NetplayView {
  easytier: NetplaySlot;
  portmap: NetplaySlot;
  frp: NetplaySlot;
  /** 已经存过 SakuraFrp 访问密钥 */
  hasToken: boolean;
  lastTunnel: number | null;
  /** 缺哪些随包组件（正常应为空） */
  missing: string[];
  /** EasyTier 建虚拟网卡需要管理员 */
  isAdmin: boolean;
}

/** 樱花 FRP 的一条隧道。 */
export interface FrpTunnel {
  id: number;
  name: string;
  kind: string;
  node: number;
  online: boolean;
  banned: boolean;
  localPort: number;
  /** 公网地址，形如 `frp-act.top:50887` */
  remote: string;
  note: string;
}

/** `netplay://log` 事件。 */
export interface NetplayLog {
  channel: string;
  stream: string;
  line: string;
}

export interface LanInfo {
  lanIp: string;
  portOpen: boolean;
}
