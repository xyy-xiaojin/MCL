/**
 * 轻量全局状态。
 *
 * 演示项目没必要上 Pinia：一个 `reactive` 对象 + 几个 action 就够了，
 * 页面只是读它、调 action。
 */
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { reactive, watch } from "vue";

import { api, errorText, inTauri } from "./api";
import type {
  DownloadSection,
  GameExit,
  JavaInfo,
  LocalVersion,
  LogEntry,
  MemoryView,
  ProgressEvent,
  QueueItem,
  RemoteVersion,
  RunningGame,
  Settings,
  ViewKey,
} from "./types";

const MAX_LOGS = 400;

export const state = reactive({
  view: "home" as ViewKey,
  /** 下载页当前分类。分类直接挂在一级导航里，所以状态提到全局。 */
  downloadSection: "vanilla" as DownloadSection,
  ready: false,
  settings: {
    source: "bmclapi",
    maxMemoryMb: 4096,
    minMemoryMb: 512,
    ramType: "auto",
    gcMode: "auto",
    javaPath: "",
    username: "Player",
    theme: "dark",
    skinPath: "",
    downloadConcurrency: 10,
  } as Settings,
  /** 后端算好的内存方案，进首页 / 改设置 / 换版本时刷新 */
  memory: null as MemoryView | null,
  /** 是否正有文件拖在窗口上（控制全屏提示层） */
  dragging: false,
  /** 这次拖进来的文件数，用于提示文案 */
  dragCount: 0,
  /** 模组列表的版本号，拖拽导入后自增，让模组页重新拉一次 */
  modsVersion: 0,
  java: null as JavaInfo | null,
  remote: [] as RemoteVersion[],
  latest: { release: "", snapshot: "" },
  local: [] as LocalVersion[],
  queue: [] as QueueItem[],
  /** 下载队列抽屉是否展开（顶栏"队列"按钮和下载页共用） */
  queueOpen: false,
  logs: [] as LogEntry[],
  paths: {} as Record<string, string>,
  /** 当前选中的版本（首页下拉 / 版本页点击都会改它） */
  selectedVersion: "" as string,
  /**
   * 玩家头像。`image` 为空表示用前端内置贴图（默认 Steve；皮肤声明 slim 才是 Alex），
   * 离线账号永远走这条路（没有正版 UUID，查不到皮肤）。
   */
  avatar: {
    uuid: "",
    model: "steve" as "steve" | "alex",
    image: null as string | null,
    source: "offline",
  },
  launching: false,
  paused: false,
  installing: "" as string,
  toast: { text: "", tone: "info" as "info" | "error" | "success" },
  /** 正在运行的游戏进程（首页「结束游戏」按钮的数据源） */
  runningGames: [] as RunningGame[],
  /**
   * 非零退出的游戏现场。后端会随 `game://exit` 附上日志路径与崩溃初判，
   * 前端弹一张卡片把建议摊开，玩家不用再截图求助。
   */
  crash: null as GameExit | null,
  /** 正在自动下载 Java（禁用按钮、改按钮文案用） */
  downloadingJava: false,
  /**
   * Forge / NeoForge 官方安装器的实时状态。
   *
   * `running` 由 `installer://log` 的第一行点亮、`installer://done` 熄灭——
   * 处理器管线要跑几分钟，没有这个面板朋友会以为卡死。
   * 放全局而不是版本页局部：中途切页进度不丢。
   */
  installer: {
    loader: "",
    lines: [] as string[],
    running: false,
    ok: null as boolean | null,
  },
});

/**
 * 下载进度事件频率较高，不能每次都在几千条队列里线性查找。
 * 这里缓存响应式条目的代理对象；每次整表替换后统一重建索引。
 */
const queueItemById = new Map<string, QueueItem>();
function rebuildQueueIndex() {
  queueItemById.clear();
  for (const item of state.queue) queueItemById.set(item.id, item);
}

let toastTimer: number | undefined;

export function notify(text: string, tone: "info" | "error" | "success" = "info") {
  state.toast.text = text;
  state.toast.tone = tone;
  if (toastTimer) window.clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => {
    state.toast.text = "";
  }, 4200);
}

/** 切到下载页的某个分类（原版游戏 / Mod / 整合包 …）。 */
export function openDownload(section: DownloadSection) {
  state.downloadSection = section;
  state.view = "downloads";
}

/** 顶栏「队列」按钮：切到下载页并直接把队列抽屉拉开。 */
export function openQueue() {
  state.queueOpen = true;
  state.view = "downloads";
}

/** 拖拽导入结果里的分类中文名。 */
const KIND_LABEL: Record<string, string> = {
  modpack: "整合包",
  mod: "模组",
  resourcepack: "材质包",
  shaderpack: "光影包",
  skin: "皮肤",
  unknown: "未知",
};

/**
 * 处理拖进窗口的文件。
 *
 * 分类由后端按包内结构判断——材质包、光影包、整合包都可能叫 `.zip`，
 * 只看扩展名会把光影包塞进 mods。
 *
 * 落地规则（版本隔离）：
 * - 整合包 → **新建一个独立版本**，成功后自动切过去
 * - mod / 材质包 / 光影包 → 进**当前选中版本**的私有目录
 */
export async function importDropped(paths: string[]) {
  if (paths.length === 0) return;
  try {
    const results = await api.importFiles(paths, state.selectedVersion || undefined);
    const done = results.filter((r) => r.ok);
    const failed = results.filter((r) => !r.ok);

    if (done.length > 0) {
      const detail = done
        .map((r) => `${KIND_LABEL[r.kind] ?? r.kind}「${r.file}」`)
        .join("、");
      notify(`已导入 ${done.length} 个：${detail}`, "success");
      state.modsVersion += 1;

      // 整合包新建了版本：刷新列表并切过去，玩家马上能启动
      const created = done.find((r) => r.versionId);
      if (created?.versionId) {
        await refreshLocal();
        state.selectedVersion = created.versionId;
        await refreshMemory();
        notify(`已切到新版本「${created.versionId}」`, "info");
      } else {
        // mod 数量变了，内存建议值要跟着重算
        await refreshMemory();
      }

      // 拖进来的是皮肤：头像立刻换，顺带同步后端写好的 skinPath
      if (done.some((r) => r.kind === "skin")) {
        await refreshAvatar();
        state.settings = await api.getSettings();
      }
    }
    if (failed.length > 0) {
      const detail = failed.map((r) => `${r.file}：${r.message}`).join("；");
      notify(`${failed.length} 个没导入成功 —— ${detail}`, "error");
    }
  } catch (e) {
    notify(errorText(e), "error");
  }
}

export function pushLog(entry: LogEntry) {  state.logs.push(entry);
  if (state.logs.length > MAX_LOGS) {
    state.logs.splice(0, state.logs.length - MAX_LOGS);
  }
}

/** 启动时订阅后端事件并拉取初始数据。只调用一次。 */
export async function bootstrap() {
  if (state.ready) return;

  // 允许用 ?view=versions 直接打开某一页：截图 / 演示时不用手动点。
  const requested = new URLSearchParams(location.search).get("view");
  const known: ViewKey[] = [
    "home",
    "versions",
    "downloads",
    "modpacks",
    "mods",
    "netplay",
    "settings",
  ];
  if (requested && (known as string[]).includes(requested)) {
    state.view = requested as ViewKey;
  }
  // ?section=mod 直接落到下载页的某个分类
  const requestedSection = new URLSearchParams(location.search).get("section");
  const sections: DownloadSection[] = [
    "vanilla",
    "mod",
    "modpack",
    "datapack",
    "resourcepack",
    "shader",
  ];
  if (requestedSection && (sections as string[]).includes(requestedSection)) {
    state.downloadSection = requestedSection as DownloadSection;
    state.view = "downloads";
  }

  // 预览用后门：浏览器里没有真实拖放事件，`?drag=1` 强制显示提示层便于截图
  if (!inTauri && new URLSearchParams(location.search).get("drag") === "1") {
    state.dragging = true;
    state.dragCount = 3;
  }

  // 浏览器预览（vite preview）没有事件通道，跳过订阅直接用快照
  if (inTauri) {
    // 四个订阅并行注册：串行 await 会让首屏多等两个来回
    await Promise.all([
      listen<QueueItem[]>("queue://changed", (event) => {
        state.queue = event.payload;
        rebuildQueueIndex();
      }),
      // 单文件进度：只更新对应条目，不整表替换，避免列表抖动
      listen<ProgressEvent>("queue://progress", (event) => {
        const item = queueItemById.get(event.payload.id);
        if (!item) return;
        item.done = event.payload.done;
        if (event.payload.total > 0) item.total = event.payload.total;
        item.state = "running";
      }),
      listen<{ stream: string; line: string }>("log://line", (event) => {
        pushLog({ ...event.payload, at: Date.now() });
      }),
      listen<GameExit>("game://exit", (event) => {
        state.launching = false;
        pushLog({
          stream: "sys",
          line: `游戏已退出（退出码 ${event.payload.code}）`,
          at: Date.now(),
        });
        void refreshRunningGames();
        if (event.payload.code !== 0) {
          // 非零退出：把崩溃现场弹出来。tips 是后端对日志尾部的初判，
          // logPath 让"打开日志"一键直达，小白不用满盘找 logs 文件夹。
          state.crash = event.payload;
          if (event.payload.tips?.length) {
            for (const tip of event.payload.tips) {
              pushLog({ stream: "sys", line: `诊断：${tip}`, at: Date.now() });
            }
          }
        } else {
          notify(`游戏已正常退出`, "info");
        }
      }),
      // 加载器安装器的实时输出（后端逐行转发，这里只留最后 30 行）
      listen<{ loader: string; line: string }>("installer://log", (event) => {
        const inst = state.installer;
        if (!inst.running || inst.loader !== event.payload.loader) {
          inst.running = true;
          inst.loader = event.payload.loader;
          inst.lines = [];
          inst.ok = null;
        }
        inst.lines.push(event.payload.line);
        if (inst.lines.length > 30) {
          inst.lines.splice(0, inst.lines.length - 30);
        }
      }),
      listen<{ loader: string; ok: boolean }>("installer://done", (event) => {
        const inst = state.installer;
        if (event.payload.loader !== inst.loader) return;
        inst.running = false;
        inst.ok = event.payload.ok;
      }),
      // 拖拽导入。Tauri 接管了文件拖放，HTML5 的 drop 事件拿不到真实路径，
      // 必须走 webview 的 onDragDropEvent。
      getCurrentWebview().onDragDropEvent((event) => {
        const payload = event.payload;
        if (payload.type === "enter") {
          state.dragCount = payload.paths.length;
          state.dragging = true;
        } else if (payload.type === "over") {
          // over 会高频触发，已经显示时不做多余更新
          if (!state.dragging) state.dragging = true;
        } else if (payload.type === "drop") {
          state.dragging = false;
          void importDropped(payload.paths);
        } else {
          state.dragging = false;
        }
      }),
    ]);
  }

  try {
    const [settings, queue, paths] = await Promise.all([
      api.getSettings(),
      api.queueSnapshot(),
      api.appPaths(),
    ]);
    state.settings = settings;
    state.queue = queue;
    rebuildQueueIndex();
    state.paths = paths;
    // 头像单独拉，不参与首屏等待：离线模式几乎瞬时，正版要查网络
    void refreshAvatar();
    await Promise.all([refreshLocal(), refreshJava()]);
    if (!state.selectedVersion && state.local.length > 0) {
      state.selectedVersion = state.local[0].id;
    }
    // 路径展示依赖选中的版本（隔离后 game 目录按版本走）
    await refreshPaths();
    // 内存方案依赖选中的版本（OptiFine 的基线不同），要在版本定下来之后再算
    await refreshMemory();

    // 运行中的游戏列表：拉一次，之后每 5 秒跟一次
    // （游戏可能在启动器窗口之外被关掉，事件收不到的兜底）
    void refreshRunningGames();
    if (inTauri) {
      window.setInterval(() => void refreshRunningGames(), 5000);
    }
  } catch (e) {
    notify(errorText(e), "error");
  }

  state.ready = true;
}

/** 重新取一次目录结构。版本隔离后 `game` 跟着选中版本变。 */
export async function refreshPaths() {
  try {
    state.paths = await api.appPaths(state.selectedVersion || undefined);
  } catch {
    // 目录展示失败不值得打断启动流程
  }
}

// 切版本时目录展示要跟着走（首页下拉、版本页点击、导入整合包都会改它）。
// 内存方案也要重算：原版和 300 mod 的整合包，推荐值差好几 GB。
watch(
  () => state.selectedVersion,
  () => {
    void refreshPaths();
    void refreshMemory();
  },
);

export async function refreshLocal() {
  state.local = await api.listLocalVersions();
  if (!state.selectedVersion && state.local.length > 0) {
    state.selectedVersion = state.local[0].id;
  }
  if (
    state.selectedVersion &&
    !state.local.some((v) => v.id === state.selectedVersion)
  ) {
    state.selectedVersion = state.local[0]?.id ?? "";
  }
}

export async function refreshRemote() {
  try {
    const list = await api.listRemoteVersions();
    state.remote = list.versions;
    state.latest = { release: list.latestRelease, snapshot: list.latestSnapshot };
  } catch (e) {
    notify(errorText(e), "error");
  }
}

export async function refreshJava(force = false) {
  try {
    state.java = await api.detectJava(force);
  } catch {
    state.java = null;
  }
}

/**
 * 拉一次玩家头像。
 *
 * 离线账号后端只做 UUID 派生，不发任何请求（也拿不到皮肤），所以改名后
 * 重算几乎没有开销。拉失败就保持现状——头像不该弹错误提示。
 */
export async function refreshAvatar() {
  try {
    state.avatar = await api.skinAvatar(state.settings.username);
  } catch {
    // 保持上一个头像
  }
}

/** 皮肤体积上限，和后端保持一致（后端还会再校验尺寸） */
const MAX_SKIN_BYTES = 1024 * 1024;

function readAsDataUrl(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result ?? ""));
    reader.onerror = () => reject(reader.error ?? new Error("读取文件失败"));
    reader.readAsDataURL(file);
  });
}

/**
 * 导入一张离线皮肤。
 *
 * 用 `<input type="file">` 而不是系统文件对话框：浏览器沙箱里拿不到真实路径，
 * 只有 `File` 对象，所以读成 data URL 交给后端落盘（一张皮肤只有几 KB）。
 * 后端会校验 PNG 签名和尺寸，不合格会抛出可直接展示的文案。
 */
export async function importSkinFile(file: File): Promise<boolean> {
  if (file.size > MAX_SKIN_BYTES) {
    notify("皮肤文件超过 1 MB —— 是不是选错文件了？", "error");
    return false;
  }
  try {
    const data = await readAsDataUrl(file);
    state.avatar = await api.importSkin(data);
    // 后端已经把 skinPath 写进 settings.json，前端这份必须跟着刷新：
    // 否则下次 saveSettings（改内存、改 Java 路径）会带着空 skinPath
    // 把刚导入的皮肤抹掉。
    state.settings = await api.getSettings();
    notify(`已应用皮肤「${file.name}」`, "success");
    return true;
  } catch (e) {
    notify(errorText(e), "error");
    return false;
  }
}

/** 恢复默认皮肤（删掉导入的 PNG）。 */
export async function resetSkin() {
  try {
    await api.resetSkin();
    state.settings = await api.getSettings();
    await refreshAvatar();
    notify("已恢复默认皮肤", "info");
  } catch (e) {
    notify(errorText(e), "error");
  }
}

let avatarTimer: number | undefined;
// 改名后头像跟着变（离线模式下模型是按用户名派生的），输入过程中防抖
watch(
  () => state.settings.username,
  () => {
    if (state.avatar.source !== "offline") return;
    if (avatarTimer) window.clearTimeout(avatarTimer);
    avatarTimer = window.setTimeout(() => void refreshAvatar(), 400);
  },
);

/**
 * 拉一次内存方案。
 *
 * 后端会现算：扫 mods 目录数 jar、读系统可用内存、按 PCL 四段阶梯给建议值。
 * 换了版本（比如从原版切到 300 mod 的整合包）要重新拉，不然显示的还是旧值。
 */
export async function refreshMemory() {
  try {
    state.memory = await api.memoryPlan(state.selectedVersion || undefined);
  } catch {
    state.memory = null;
  }
}

export async function saveSettings() {
  try {
    state.settings = await api.saveSettings({ ...state.settings });
    // 自动内存 / GC 模式改了要重新算一遍
    await refreshMemory();
    void refreshAvatar();
    notify("设置已保存", "success");
  } catch (e) {
    notify(errorText(e), "error");
  }
}

export async function launchSelected() {
  if (!state.selectedVersion) {
    notify("请先选择一个版本", "error");
    return;
  }
  if (state.launching) return;
  // 后端会拦重复启动，但先在界面上给个明确提示，不用等报错转一圈
  if (state.runningGames.some((g) => g.version === state.selectedVersion)) {
    notify("这个版本已经在运行了，可以用「结束游戏」强制关闭", "error");
    return;
  }
  state.launching = true;
  try {
    // 启动读的是后端状态，所以先把界面上的改动落盘
    state.settings = await api.saveSettings({ ...state.settings });
    const report = await api.launchVersion(state.selectedVersion);
    pushLog({
      stream: "sys",
      line: `已启动 ${state.selectedVersion}，PID ${report.pid}（Java ${report.javaMajor}，${report.argCount} 个参数）`,
      at: Date.now(),
    });
    notify(`游戏已启动（PID ${report.pid}）`, "success");
    await refreshRunningGames();
  } catch (e) {
    state.launching = false;
    notify(errorText(e), "error");
    pushLog({ stream: "sys", line: errorText(e), at: Date.now() });
  }
}

/** 拉一次正在运行的游戏列表。定时器和启动/退出事件都会调它。 */
export async function refreshRunningGames() {
  if (!inTauri) return;
  try {
    state.runningGames = await api.gameStatus();
  } catch {
    // 状态刷新失败不值得报错
  }
}

/** 强制结束一个（或全部）游戏进程。 */
export async function killGame(version?: string) {
  try {
    const killed = await api.gameKill(version);
    if (killed > 0) {
      notify(`已结束 ${killed} 个游戏进程`, "info");
    }
    await refreshRunningGames();
  } catch (e) {
    notify(errorText(e), "error");
  }
}

/** 关掉崩溃卡片。日志路径已在卡片上，玩家随时可以再打开。 */
export function dismissCrash() {
  state.crash = null;
}

/** 打开崩溃日志所在的文件夹并选中日志文件。 */
export async function revealCrashLog() {
  const path = state.crash?.logPath;
  if (!path) return;
  try {
    await api.revealInExplorer(path);
  } catch (e) {
    notify(errorText(e), "error");
  }
}

/**
 * 一键下载 Java 运行时。
 *
 * 进度会出现在顶栏的下载队列里（后端走全局队列），完成后 Java 探测
 * 自动刷新——新路径已经由后端写进设置，前端只管把界面拉成最新。
 */
export async function downloadJavaRuntime(major: number) {
  if (state.downloadingJava) return;
  state.downloadingJava = true;
  try {
    notify(`正在下载 Java ${major} 运行时（约 50 MB），进度看顶栏的队列`, "info");
    await api.downloadJava(major);
    await refreshJava(true);
    notify(`Java ${major} 已就绪，可以启动游戏了`, "success");
  } catch (e) {
    notify(errorText(e), "error");
  } finally {
    state.downloadingJava = false;
  }
}

/** 暂停/继续整个下载队列。 */
export async function toggleQueuePause() {
  try {
    if (state.paused) {
      await api.queueResume();
      state.paused = false;
      notify("已继续下载", "info");
    } else {
      await api.queuePause();
      state.paused = true;
      notify("已暂停下载", "info");
    }
  } catch (e) {
    notify(errorText(e), "error");
  }
}

export async function clearFinishedQueue() {
  try {
    await api.queueClear();
  } catch (e) {
    notify(errorText(e), "error");
  }
}

export function clearLogs() {
  state.logs.splice(0, state.logs.length);
}
