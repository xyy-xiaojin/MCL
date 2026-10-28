/**
 * 浏览器预览用的假数据。
 *
 * 只有在**没有 Tauri 运行时**（直接用 `vite preview` 打开）时才会命中，
 * 一旦跑在 Tauri 窗口里，`window.__TAURI_INTERNALS__` 存在，这里完全不会执行。
 *
 * 目的只有一个：改样式时不用起后端就能看到带数据的页面。
 * 真实数据一律来自 Rust 侧，这里不参与任何业务判断。
 */

const pick = <T,>(list: T[], n: number) => list.slice(0, n);

const mods = [
  {
    id: "aax0x1",
    slug: "sodium",
    title: "钠 · Sodium",
    description:
      "A high-performance rendering engine replacement for Minecraft, which greatly improves frame rates while fixing graphical issues.",
    iconUrl: "",
    downloads: 252_000_000,
    follows: 24_000,
    updated: new Date(Date.now() - 7 * 86_400_000).toISOString(),
    categories: ["fabric", "neoforge", "optimization"],
    versions: ["1.21.4", "1.21.1", "1.20.1"],
    projectType: "mod",
    author: "CaffeineMC",
  },
  {
    id: "iris01",
    slug: "iris",
    title: "Iris Shaders",
    description:
      "A modern shader pack loader for Minecraft intended to be compatible with existing OptiFine shader packs.",
    iconUrl: "",
    downloads: 195_000_000,
    follows: 18_500,
    updated: new Date(Date.now() - 23 * 86_400_000).toISOString(),
    categories: ["fabric", "neoforge", "decoration"],
    versions: ["1.21.4", "1.20.1"],
    projectType: "mod",
    author: "IrisShaders",
  },
  {
    id: "entityc1",
    slug: "entityculling",
    title: "实体渲染机制优化 · Entity Culling",
    description:
      "Using async path-tracing to hide Block-/Entities that are not visible through walls. No OptiFine required.",
    iconUrl: "",
    downloads: 189_000_000,
    follows: 6_200,
    updated: new Date(Date.now() - 14 * 86_400_000).toISOString(),
    categories: ["fabric", "forge", "optimization"],
    versions: ["1.16.5", "1.12.2"],
    projectType: "mod",
    author: "tr7zw",
  },
  {
    id: "cloth01",
    slug: "cloth-config",
    title: "Cloth Config API",
    description: "Configuration Library for Minecraft Mods. 提供一套通用的配置界面。",
    iconUrl: "",
    downloads: 184_000_000,
    follows: 3_100,
    updated: new Date(Date.now() - 30 * 86_400_000).toISOString(),
    categories: ["fabric", "forge", "library"],
    versions: ["1.21.4", "1.21.1", "1.20.1"],
    projectType: "mod",
    author: "shedaniel",
  },
];

const packs = [
  {
    id: "fabopt",
    slug: "fabulously-optimized",
    title: "Fabulously Optimized",
    description:
      "A lightweight, Fabric-based modpack focused on performance, with Sodium, Iris and a pile of QoL mods.",
    iconUrl: "",
    downloads: 12_400_000,
    follows: 2_900,
    updated: new Date(Date.now() - 3 * 86_400_000).toISOString(),
    categories: ["fabric", "optimization"],
    versions: ["1.21.4", "1.20.1"],
    projectType: "modpack",
    author: "Robotkoer",
  },
  {
    id: "bettermc",
    slug: "better-mc",
    title: "Better MC",
    description: "Populated, adventure-flavoured pack built around exploration and combat.",
    iconUrl: "",
    downloads: 8_100_000,
    follows: 1_400,
    updated: new Date(Date.now() - 11 * 86_400_000).toISOString(),
    categories: ["forge", "adventure"],
    versions: ["1.20.1", "1.19.2"],
    projectType: "modpack",
    author: "Luna Pixel Studios",
  },
];

const shaders = [
  {
    id: "complementary",
    slug: "complementary-reimagined",
    title: "Complementary Reimagined",
    description: "一款兼顾性能与观感的光影，默认设置下就能有很好的画面表现。",
    iconUrl: "",
    downloads: 42_000_000,
    follows: 9_800,
    updated: new Date(Date.now() - 5 * 86_400_000).toISOString(),
    categories: ["iris", "optifine"],
    versions: ["1.21.4", "1.20.1"],
    projectType: "shader",
    author: "EminGT",
  },
];

const resources = [
  {
    id: "faithful",
    slug: "faithful-32x",
    title: "Faithful 32x",
    description: "原版风格的 32 倍材质，保留原味同时提升清晰度。",
    iconUrl: "",
    downloads: 88_000_000,
    follows: 12_000,
    updated: new Date(Date.now() - 9 * 86_400_000).toISOString(),
    categories: ["32x", "vanilla-like"],
    versions: ["1.21.4", "1.20.1"],
    projectType: "resourcepack",
    author: "Faithful Team",
  },
];

const datapacks = [
  {
    id: "vanillatweaks",
    slug: "vanilla-tweaks",
    title: "Vanilla Tweaks",
    description: "一组纯原版数据包，修修补补又不会破坏存档兼容性。",
    iconUrl: "",
    downloads: 5_400_000,
    follows: 1_100,
    updated: new Date(Date.now() - 20 * 86_400_000).toISOString(),
    categories: ["utility"],
    versions: ["1.21.4", "1.20.1"],
    projectType: "datapack",
    author: "Vanilla Tweaks",
  },
];

const byKind: Record<string, typeof mods> = {
  mod: mods,
  modpack: packs,
  shader: shaders,
  resourcepack: resources,
  datapack: datapacks,
};

const remoteVersions = {
  latestRelease: "1.21.4",
  latestSnapshot: "25w03a",
  versions: [
    { id: "1.21.4", kind: "release", installed: true },
    { id: "1.21.3", kind: "release", installed: false },
    { id: "1.21.1", kind: "release", installed: true },
    { id: "1.21", kind: "release", installed: false },
    { id: "1.20.6", kind: "release", installed: false },
    { id: "1.20.1", kind: "release", installed: true },
    { id: "1.19.4", kind: "release", installed: false },
    { id: "1.18.2", kind: "release", installed: false },
    { id: "1.16.5", kind: "release", installed: false },
    { id: "1.12.2", kind: "release", installed: false },
    { id: "1.8.9", kind: "release", installed: false },
    { id: "25w03a", kind: "snapshot", installed: false },
    { id: "25w02a", kind: "snapshot", installed: false },
    { id: "24w50a", kind: "snapshot", installed: false },
  ],
};

/** 命令名 → 假数据。签名和真实命令保持一致，方便直接替换。 */
export const browserMock: Record<string, (args?: any) => unknown> = {
  // 浏览器预览没有后端，头像一律回落内置贴图
  skin_avatar: () => ({
    uuid: "00000000-0000-3000-8000-000000000000",
    model: "steve",
    image: null,
    source: "offline",
  }),
  // 浏览器预览里没有 Rust 后端，导皮肤直接装作成功（不落盘）
  import_skin: (args) => ({
    uuid: "00000000-0000-3000-8000-000000000000",
    model: "steve",
    image: args?.data ?? null,
    source: "local",
  }),
  reset_skin: () => null,
  get_settings: () => ({
    source: "bmclapi",
    maxMemoryMb: 4096,
    minMemoryMb: 512,
    ramType: "auto",
    gcMode: "auto",
    javaPath: "",
    username: "Player",
    skinPath: "",
    downloadConcurrency: 10,
  }),
  memory_plan: (args) => {
    const mods = 187;
    const availableMb = 12_288;
    const totalMb = 16_384;
    const recommendMb = 5_248;
    return {
      plan: {
        modCount: mods,
        availableMb,
        totalMb,
        recommendMb,
        minimumMb: 1750,
        modable: true,
        reason: `${mods} 个 mod`,
      },
      effectiveMaxMb: recommendMb,
      effectiveMinMb: 2624,
      manualMaxMb: 4096,
      safeLimitMb: Math.round(totalMb * 0.7),
      gcMode: "auto",
      gcArgs: [
        "-XX:+UnlockExperimentalVMOptions",
        "-XX:+UseZGC",
        "-XX:+ZGenerational",
      ],
    };
  },
  queue_snapshot: () => [
    {
      id: "t1",
      label: "1.21.4.jar",
      kind: "client",
      groupId: "g1",
      groupLabel: "游戏文件下载",
      state: "running",
      total: 28_400_000,
      done: 17_600_000,
      error: null,
    },
    {
      id: "t2",
      label: "sodium 0.6.9",
      kind: "mod",
      groupId: "g2",
      groupLabel: "社区资源下载",
      state: "done",
      total: 1_240_000,
      done: 1_240_000,
      error: null,
    },
  ],
  check_data_directory: () => ({ ok: true, path: "浏览器预览模式", message: "演示模式：未执行真实目录写入测试" }),
  queue_retry_failed: () => true,
  app_paths: (args) => ({
    root: "C:/Users/Player/AppData/Roaming/MCL",
    versions: "C:/Users/Player/AppData/Roaming/MCL/versions",
    game: `C:/Users/Player/AppData/Roaming/MCL/versions/${args?.versionId ?? "1.21.4"}/game`,
  }),
  detect_java: () => ({ path: "（演示数据）C:/Program Files/Java/jdk-21/bin/javaw.exe", major: 21, source: "浏览器预览演示" }),
  list_local_versions: () => [
    {
      id: "fabulously-optimized",
      kind: "release",
      javaMajor: 21,
      dir: "C:/Users/Player/AppData/Roaming/MCL/versions/fabulously-optimized",
      complete: true,
      size: 412_356_608,
      hasSaves: true,
      modCount: 187,
    },
    {
      id: "1.21.4",
      kind: "release",
      javaMajor: 21,
      dir: "C:/Users/Player/AppData/Roaming/MCL/versions/1.21.4",
      complete: true,
      size: 268_435_456,
      hasSaves: true,
      modCount: 187,
    },
    {
      id: "1.20.1",
      kind: "release",
      javaMajor: 17,
      dir: "C:/Users/Player/AppData/Roaming/MCL/versions/1.20.1",
      complete: true,
      size: 201_326_592,
      hasSaves: false,
      modCount: 0,
    },
  ],
  delete_version: (args) => ({
    version: args?.id ?? "",
    freed: 201_326_592,
    removed: [],
  }),
  list_remote_versions: () => remoteVersions,
  browse_search: (args) => {
    const list = byKind[args?.kind] ?? mods;
    const q = String(args?.query ?? "").trim().toLowerCase();
    if (!q) return list;
    return list.filter(
      (item) =>
        item.title.toLowerCase().includes(q) || item.slug.toLowerCase().includes(q),
    );
  },
  browse_install: (args) => ({
    title: args?.title ?? "示例项目",
    version: "1.0.0",
    files: 1,
    dir: "C:/Users/Player/AppData/Roaming/MCL/versions/1.21.4/game/mods",
    instance: null,
  }),
  list_mods: () => [
    { fileName: "sodium-fabric-0.6.9.jar", name: "Sodium", enabled: true, size: 1_240_000 },
    { fileName: "iris-1.8.1.jar", name: "Iris", enabled: false, size: 8_900_000 },
  ],
  search_modpacks: () => packs,
  // 以下都是"点了没反应"的命令，给个占位避免预览时报错
  save_settings: (args) => args?.settings ?? {},
  queue_pause: () => undefined,
  queue_resume: () => undefined,
  queue_cancel: () => undefined,
  queue_cancel_group: () => undefined,
  queue_clear: () => undefined,
  toggle_mod: () => true,
  install_loader: (args) => ({
    versionId: `${args?.loader ?? "fabric"}-loader-0.16.9-${args?.gameVersion ?? "1.21.4"}`,
    loader: args?.loader ?? "fabric",
    loaderVersion: "0.16.9",
    gameVersion: args?.gameVersion ?? "1.21.4",
    libraries: 12,
  }),
  list_loader_versions: () => ["0.16.9", "0.16.8", "0.15.11"],
  import_modpack: (args) => ({
    instance: "preview",
    versionId: "preview",
    gameVersion: args?.baseVersion ?? "1.21.4",
    loader: "Fabric 0.16.9",
    dir: "C:/Users/Player/AppData/Roaming/MCL/versions/preview/game",
    overrides: 0,
    files: 0,
    skipped: 0,
    note: "",
  }),
  import_files: (args) =>
    (args?.paths ?? []).map((path: string) => ({
      file: path.split(/[\\/]/).pop() ?? path,
      kind: "mod",
      ok: true,
      message: "已放入 preview",
    })),
  export_instance: () => ({
    path: "C:/Users/Player/AppData/Roaming/MCL/exports/MCL-instance-1761234567.zip",
    files: 214,
    bytes: 386_547_200,
  }),
  install_version: (args) => ({
    version: args?.id ?? "",
    files: 0,
    bytes: 0,
    natives: 0,
    legacy: 0,
  }),
  launch_version: () => ({ pid: 4242, java: "javaw", javaMajor: 21, argCount: 42 }),
  reveal_in_explorer: () => undefined,
  // 联机：预览时给一份"端口映射已成功"的样子，方便调样式
  netplay_status: () => ({
    easytier: {
      state: "idle",
      summary: "未启动",
      address: "",
      localPort: 0,
      error: "",
      facts: [],
    },
    portmap: {
      state: "running",
      summary: "端口已映射，朋友可以直接连",
      address: "221.229.123.146:25565",
      localPort: 25565,
      error: "",
      facts: [
        { label: "内网地址", value: "192.168.1.7:25565" },
        { label: "NAT 类型", value: "锥形（可直连）" },
        { label: "UPnP", value: "已开映射" },
      ],
    },
    frp: {
      state: "idle",
      summary: "未启动",
      address: "",
      localPort: 0,
      error: "",
      facts: [],
    },
    hasToken: false,
    lastTunnel: null,
    missing: [],
    isAdmin: false,
  }),
  netplay_stop: () => undefined,
  netplay_lan_info: () => ({ lanIp: "192.168.1.7", portOpen: true }),
  netplay_relaunch_admin: () => false,
  netplay_frp_login: () => [],
};

export const mockTitles = pick(mods, 4);
