/**
 * IPC 封装层。
 *
 * 前端**只**通过这里跟 Rust 通信：没有任何 `fetch`、`fs` 或本地计算逻辑，
 * 下载、哈希、文件写入、启动进程全部在后端完成。
 *
 * 例外只有一处：用 `vite preview` 在浏览器里看样式时没有 Tauri 运行时，
 * 这时退回 `mock.ts` 的演示数据（仅影响预览，跑在窗口里永远不会走到）。
 */
import { invoke } from "@tauri-apps/api/core";
import type {
  AvatarView,
  AuthAccountView,
  AuthLoginResult,
  AuthServerInfo,
  AuthStatusView,
  BrowseHit,
  DeleteReport,
  SavesBackup,
  MsDeviceCode,
  MsPollResult,
  ExportOptions,
  ExportReport,
  ImportReport,
  ImportOutcome,
  InstallReport,
  JavaInfo,
  LaunchReport,
  LoaderReport,
  LocalVersion,
  MemoryView,
  ModEntry,
  ModIssue,
  FrpTunnel,
  LanInfo,
  ModpackHit,
  NetplayChannel,
  NetplayView,
  ProjectInstallReport,
  QueueItem,
  RunningGame,
  Settings,
  SourceProbe,
  VersionList,
} from "./types";
import { browserMock } from "./mock";

/** 跑在 Tauri 窗口里时才有这个内部对象。 */
export const inTauri =
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (!inTauri) {
    const handler = browserMock[cmd];
    if (handler) return Promise.resolve(handler(args) as T);
    // 没有演示数据的命令在浏览器里静默返回空，避免预览页面满屏报错
    return Promise.resolve(undefined as T);
  }
  return invoke<T>(cmd, args);
}

export const api = {
  // 版本
  listRemoteVersions: () => call<VersionList>("list_remote_versions"),
  listLocalVersions: () => call<LocalVersion[]>("list_local_versions"),
  installVersion: (id: string) => call<InstallReport>("install_version", { id }),
  deleteVersion: (id: string) => call<DeleteReport>("delete_version", { id }),
  copyVersion: (id: string, newId: string) =>
    call<string>("copy_version", { id, newId }),
  renameVersion: (id: string, newId: string) =>
    call<string>("rename_version", { id, newId }),
  /** 校验并补齐缺失/损坏的游戏文件（不碰存档和 mod） */
  repairVersion: (id: string) => call<InstallReport>("repair_version", { id }),
  // 存档备份
  backupList: (versionId: string) =>
    call<SavesBackup[]>("backup_list", { versionId }),
  backupCreate: (versionId: string) =>
    call<SavesBackup | null>("backup_create", { versionId }),
  backupRestore: (versionId: string, file: string) =>
    call<void>("backup_restore", { versionId, file }),
  backupRemove: (versionId: string, file: string) =>
    call<void>("backup_remove", { versionId, file }),
  launchVersion: (id: string) => call<LaunchReport>("launch_version", { id }),

  // 运行环境
  /** `force` 为真时忽略后端缓存重新探测（"重新检测"按钮用） */
  detectJava: (force = false) => call<JavaInfo | null>("detect_java", { force }),
  /** 自动下载 Java 运行时（进度走下载队列抽屉，完成后自动写进设置） */
  downloadJava: (major: number) => call<JavaInfo>("download_java", { major }),

  // 运行中的游戏
  gameStatus: () => call<RunningGame[]>("game_status"),
  /** 强制结束游戏；不传 version 就结束全部 */
  gameKill: (version?: string) =>
    call<number>("game_kill", { version: version ?? null }),

  // 设置
  getSettings: () => call<Settings>("get_settings"),
  saveSettings: (settings: Settings) => call<Settings>("save_settings", { settings }),

  // 内存方案（自动分配 / GC 参数）
  memoryPlan: (versionId?: string) =>
    call<MemoryView>("memory_plan", { versionId: versionId ?? null }),

  // 下载队列
  queueSnapshot: () => call<QueueItem[]>("queue_snapshot"),
  queuePause: () => call<void>("queue_pause"),
  queueResume: () => call<void>("queue_resume"),
  queueCancel: (id: string) => call<void>("queue_cancel", { id }),
  queueRetryFailed: (id: string) => call<boolean>("queue_retry_failed", { id }),
  queueCancelGroup: (groupId: string) => call<void>("queue_cancel_group", { groupId }),
  queueClear: () => call<void>("queue_clear"),

  // 整合包（会新建一个独立版本）
  searchModpacks: (query: string) => call<ModpackHit[]>("search_modpacks", { query }),
  /** `baseVersion` 只在普通 zip 需要：它没有内嵌游戏版本信息 */
  importModpack: (path: string, baseVersion?: string) =>
    call<ImportReport>("import_modpack", { path, baseVersion: baseVersion ?? null }),
  exportInstance: (options: ExportOptions, versionId: string) =>
    call<ExportReport>("export_instance", { options, versionId }),

  /** 拖拽导入：mod / 材质包 / 光影包进 `targetVersion` 的私有目录，整合包新建版本 */
  importFiles: (paths: string[], targetVersion?: string) =>
    call<ImportOutcome[]>("import_files", {
      paths,
      targetVersion: targetVersion ?? null,
    }),

  // 加载器
  installLoader: (    loader: string,
    gameVersion: string,
    loaderVersion?: string,
    targetId?: string,
  ) =>
    call<LoaderReport>("install_loader", {
      loader,
      gameVersion,
      loaderVersion: loaderVersion ?? null,
      targetId: targetId ?? null,
    }),
  listLoaderVersions: (loader: string, gameVersion: string) =>
    call<string[]>("list_loader_versions", { loader, gameVersion }),

  /** 测两个下载源的实时速度 */
  testSources: () => call<SourceProbe[]>("test_sources"),

  // 分类浏览（原版游戏走上面版本那组，这里是社区资源）
  browseSearch: (params: {
    kind: string;
    query: string;
    gameVersion: string;
    loader: string;
    index: string;
    offset: number;
  }) => call<BrowseHit[]>("browse_search", params),
  browseInstall: (params: {
    kind: string;
    projectId: string;
    gameVersion: string;
    loader: string;
    title: string;
    /** 装进哪个版本；整合包不用（会自己新建） */
    targetVersion?: string;
  }) => call<ProjectInstallReport>("browse_install", params),

  // 模组（按版本隔离）
  listMods: (versionId: string) => call<ModEntry[]>("list_mods", { versionId }),
  toggleMod: (versionId: string, fileName: string) =>
    call<boolean>("toggle_mod", { versionId, fileName }),
  /** 缺前置 / 版本不匹配 / 冲突检测（读 jar 元数据，不联网） */
  modDiagnostics: (versionId: string) =>
    call<ModIssue[]>("mod_diagnostics", { versionId }),

  // 杂项
  appPaths: (versionId?: string) =>
    call<Record<string, string>>("app_paths", { versionId: versionId ?? null }),
  checkDataDirectory: () =>
    call<{ ok: boolean; path: string; message: string }>("check_data_directory"),
  /** 在系统文件管理器中打开目录（设置页「目录结构」的「打开」按钮） */
  revealInExplorer: (path: string) => call<void>("reveal_in_explorer", { path }),

  /**
   * 系统文件选择框（tauri-plugin-dialog）。
   *
   * 返回选中的真实路径列表，直接喂给 `importFiles` 就能走与拖拽相同的
   * 后端分流逻辑。浏览器预览里没有对话框可弹，返回 null。
   */
  pickFiles: async (
    title: string,
    extensions: string[],
    multiple = true,
  ): Promise<string[] | null> => {
    if (!inTauri) return null;
    const { open } = await import("@tauri-apps/plugin-dialog");
    const picked = await open({
      title,
      multiple,
      filters: [{ name: extensions.map((e) => `*.${e}`).join("，"), extensions }],
    });
    return picked === null ? null : Array.isArray(picked) ? picked : [picked];
  },

  // 玩家头像：传 uuid 才会去官方查皮肤，否则按名字派生离线 UUID
  skinAvatar: (name?: string, uuid?: string) =>
    call<AvatarView>("skin_avatar", { name: name ?? null, uuid: uuid || null }),
  /** 导入离线皮肤：`data` 是 data URL，后端校验尺寸后落盘并回新头像 */
  importSkin: (data: string) => call<AvatarView>("import_skin", { data }),
  resetSkin: () => call<null>("reset_skin"),

  // 皮肤站账户（authlib-injector / yggdrasil）
  authStatus: () => call<AuthStatusView>("auth_status"),
  /** 切换活跃账户；index 传 null 切回离线模式 */
  authSetActive: (index: number | null) =>
    call<AuthStatusView>("auth_set_active", { index }),
  /** 验证皮肤站地址并拉元数据（「测试连接」） */
  authServerInfo: (serverUrl: string) =>
    call<AuthServerInfo>("auth_server_info", { serverUrl }),
  authLogin: (serverUrl: string, email: string, password: string) =>
    call<AuthLoginResult>("auth_login", { serverUrl, email, password }),
  authSelectProfile: (profileId: string, profileName: string) =>
    call<AuthAccountView>("auth_select_profile", { profileId, profileName }),
  authLogout: () => call<void>("auth_logout"),

  // 微软正版（设备码流程）
  authMsBegin: () => call<MsDeviceCode>("auth_ms_begin"),
  authMsPoll: (deviceCode: string) =>
    call<MsPollResult>("auth_ms_poll", { deviceCode }),
  /** 系统浏览器打开验证页 */
  openInBrowser: (url: string) => call<void>("open_in_browser", { url }),

  // 联机
  netplayStatus: () => call<NetplayView>("netplay_status"),
  netplayStop: (channel: NetplayChannel) =>
    call<void>("netplay_stop", { channel }),
  /** 本机局域网地址 + 游戏端口有没有在听 */
  netplayLanInfo: (localPort?: number) =>
    call<LanInfo>("netplay_lan_info", { localPort: localPort ?? null }),
  /** EasyTier 建虚拟网卡要管理员：用管理员身份重启自己 */
  netplayRelaunchAdmin: () => call<boolean>("netplay_relaunch_admin"),
  netplayEasytierStart: (req: {
    room: string;
    password: string;
    host: boolean;
    localPort: number;
  }) => call<import("./types").NetplaySlot>("netplay_easytier_start", { req }),
  netplayPortmapStart: (req: { localPort: number; useUpnp: boolean }) =>
    call<import("./types").NetplaySlot>("netplay_portmap_start", { req }),
  /** 用已保存的密钥拉隧道列表 */
  netplayFrpLogin: () => call<FrpTunnel[]>("netplay_frp_login"),
  /** 存新密钥并立刻验证 */
  netplayFrpSaveToken: (token: string) =>
    call<FrpTunnel[]>("netplay_frp_save_token", { token }),
  netplayFrpStart: (tunnelId: number, localPort: number) =>
    call<import("./types").NetplaySlot>("netplay_frp_start", {
      tunnelId,
      localPort,
    }),
};

/** 把后端抛回来的错误统一转成可展示的文案。 */
export function errorText(e: unknown): string {
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  return String(e);
}

/** 字节数 → 人类可读。 */
export function humanSize(bytes: number): string {
  if (!bytes) return "0 B";
  const units = ["B", "KB", "MB", "GB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(unit === 0 ? 0 : 1)} ${units[unit]}`;
}
