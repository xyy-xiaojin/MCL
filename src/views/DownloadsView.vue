<script setup lang="ts">
/**
 * 下载页。
 *
 * 分类不在这里渲染——它们已经挂在左侧主导航里（见 `Sidebar.vue`），
 * 当前分类存在 `state.downloadSection`，所以这个页面只剩"内容区"，
 * 宽度可以整个用满。
 *
 * - 「原版游戏」直接列 Mojang 版本清单，点一下就装；
 * - 「社区资源」按 Modrinth 的 project_type 分类，进来先给**热门**（空查询 + 按下载量排），
 *   也可以按名称 / 游戏版本 / 加载器搜索。
 *
 * 这里只做展示与筛选，搜索请求、文件解析、下载全部走后端。
 */
import { computed, onMounted, ref, watch } from "vue";

import Icon from "../components/Icon.vue";
import ProgressBar from "../components/ProgressBar.vue";
import { api, errorText, humanSize } from "../api";
import { LOADER_LABEL } from "../labels";
import {
  clearFinishedQueue,
  clearLogs,
  notify,
  openDownload,
  refreshLocal,
  refreshRemote,
  state,
  toggleQueuePause,
} from "../store";
import type { BrowseHit, QueueItem } from "../types";

/** 分类的展示元数据（导航在 Sidebar 里，这里只要标题和说明）。 */
const meta: Record<string, { label: string; icon: string; hint: string }> = {
  vanilla: { label: "原版游戏", icon: "package", hint: "点版本选择游戏类型（原版或 Fabric / Forge 等加载器）；也可到「版本管理 → 安装游戏」" },
  mod: { label: "Mod", icon: "mods", hint: "模组，需要对应加载器（Fabric / Forge / NeoForge）" },
  modpack: { label: "整合包", icon: "modpack", hint: "一键装好整套环境，本地包拖进窗口即可导入" },
  datapack: { label: "数据包", icon: "datapack", hint: "放进存档的玩法扩展" },
  resourcepack: { label: "材质包", icon: "texture", hint: "替换方块与界面贴图" },
  shader: { label: "光影包", icon: "shader", hint: "需要 Iris / OptiFine 之类的光影前置" },
};

const section = computed(() => state.downloadSection);
const current = computed(() => meta[section.value] ?? meta.vanilla);

/** 分类做成页内 tab：侧栏不再重复挂 6 个入口，宽度给内容 */
const sectionTabs = [
  { key: "vanilla", label: "原版游戏" },
  { key: "mod", label: "Mod" },
  { key: "modpack", label: "整合包" },
  { key: "datapack", label: "数据包" },
  { key: "resourcepack", label: "材质包" },
  { key: "shader", label: "光影包" },
] as const;

// ---- 原版游戏 ----

const vanillaQuery = ref("");
const installing = ref("");

const vanillaVersions = computed(() => {
  const q = vanillaQuery.value.trim().toLowerCase();
  return state.remote.filter((v) => !q || v.id.toLowerCase().includes(q));
});

const releaseList = computed(() =>
  vanillaVersions.value.filter((v) => v.kind === "release").slice(0, 120),
);
const snapshotList = computed(() =>
  vanillaVersions.value.filter((v) => v.kind === "snapshot").slice(0, 36),
);

async function installVanilla(id: string) {
  // 同一时间只允许装一个版本：否则 installing 被 B 覆盖，A 完成时会把
  // 状态清空，B 还在下载但界面已经恢复可点，可以再点出第三个
  if (installing.value) {
    notify("已有一个版本在安装，等它完成再试", "error");
    return;
  }
  installing.value = id;
  try {
    const report = await api.installVersion(id);
    await refreshLocal();
    notify(`${id} 安装完成：${report.files} 个文件`, "success");
  } catch (e) {
    notify(errorText(e), "error");
  } finally {
    installing.value = "";
  }
}

// ---- 加载器选择弹窗 ----
//
// 点原版版本不再直接装原版：先弹窗让玩家选游戏类型（原版 / 加载器），
// 和「版本管理 → 安装游戏」走同一套后端（install_version / install_loader）。
// Fabric / Quilt 几秒建好；Forge / NeoForge 跑官方安装器要几分钟，进度在队列里看。

const pickerVersion = ref("");
const pickerBusy = ref("");
const pickerChoices = [
  { id: "vanilla", name: "原版 Minecraft", desc: "纯净游戏，不安装模组加载器", icon: "package" },
  { id: "fabric", name: "Fabric", desc: "轻量快速，适合大量现代模组", icon: "bolt" },
  { id: "forge", name: "Forge", desc: "模组生态最全，老版本首选", icon: "settings" },
  { id: "neoforge", name: "NeoForge", desc: "Forge 社区分支，适合新版本", icon: "settings" },
  { id: "quilt", name: "Quilt", desc: "另一种轻量模组加载器", icon: "datapack" },
] as const;

function openPicker(id: string) {
  if (installing.value || pickerBusy.value) return;
  pickerVersion.value = id;
}

async function choosePicker(loader: string) {
  const id = pickerVersion.value;
  if (!id || pickerBusy.value) return;
  if (loader === "vanilla") {
    // 关弹窗再装，进度提示交给顶栏 toast 和下载队列
    pickerVersion.value = "";
    void installVanilla(id);
    return;
  }
  pickerBusy.value = loader;
  try {
    const report = await api.installLoader(loader, id);
    await refreshLocal();
    await refreshRemote();
    state.selectedVersion = report.versionId;
    notify(
      `已新建版本「${report.versionId}」：${id} + ${LOADER_LABEL[report.loader] ?? report.loader} ${report.loaderVersion}`,
      "success",
    );
    // 类库没下全时后端会带一条 warning，别把版本说成"可以直接玩"
    if (report.warning) notify(report.warning, "error");
    pickerVersion.value = "";
  } catch (e) {
    notify(errorText(e), "error");
  } finally {
    pickerBusy.value = "";
  }
}

// ---- 社区资源 ----

const sort = ref("downloads");
const query = ref("");
/** 空字符串 = 不筛选。`<option value="">` 显示成「全部…」 */
const loader = ref("");
const hits = ref<BrowseHit[]>([]);
const loading = ref(false);
const loadError = ref("");
const installingProject = ref("");

/** 游戏版本下拉：拿清单里的正式版，默认「全部版本」（和 PCL 一样，先让人看到热门）。 */
const gameVersions = computed(() => {
  const list = state.remote.filter((v) => v.kind === "release").map((v) => v.id);
  return ["", ...list.slice(0, 40)];
});
const gameVersion = ref("");

/** 加载器只对 mod / modpack 有意义。 */
const loaders = computed(() => {
  if (section.value === "mod" || section.value === "modpack") {
    return ["", "fabric", "forge", "neoforge", "quilt"];
  }
  return [""];
});

const isHot = computed(() => !query.value.trim() && sort.value === "downloads");

/** 换排序要重新请求：排序是后端参数，只改本地变量列表不会变。 */
function pickSort(value: string) {
  if (sort.value === value) return;
  sort.value = value;
  void load(true);
}

const sortOptions = [
  { value: "downloads", label: "最多下载" },
  { value: "follows", label: "最多关注" },
  { value: "updated", label: "最近更新" },
  { value: "newest", label: "最新发布" },
];

watch(section, () => {
  hits.value = [];
  loadError.value = "";
  // 加载器筛选只对 mod / modpack 有意义。换到别的分类时不清掉的话，
  // 残留的旧值会悄悄混进搜索和安装请求，而下拉框此时已禁用，
  // 用户没有任何办法改回来。
  if (section.value !== "mod" && section.value !== "modpack") {
    loader.value = "";
  }
  if (section.value === "vanilla") return;
  void load(true);
});

/** 后端一页给 20 条；不够 20 说明到底了，后面就不用再请求。 */
const PAGE = 20;
const exhausted = ref(false);

/**
 * 请求代际号：响应回来时号对不上就说明已经被更新的请求取代，
 * 直接丢弃。不用「加载中就拒绝新请求」——那样快速切分类时新分类
 * 的请求会被静默丢掉，界面上停在旧数据，之后再也不加载。
 */
let requestSeq = 0;

async function load(reset: boolean) {
  if (section.value === "vanilla") return;
  if (!reset && exhausted.value) return;
  const seq = ++requestSeq;
  loading.value = true;
  loadError.value = "";
  const kind = section.value;
  try {
    const list = await api.browseSearch({
      kind,
      query: query.value.trim(),
      gameVersion: gameVersion.value,
      loader: loader.value,
      index: sort.value,
      offset: reset ? 0 : hits.value.length,
    });
    // 过期响应：分类又切了、或已经发起了一次新的加载
    if (seq !== requestSeq || kind !== section.value) return;
    hits.value = reset ? list : [...hits.value, ...list];
    exhausted.value = list.length < PAGE;
  } catch (e) {
    if (seq !== requestSeq) return;
    loadError.value = errorText(e);
    if (reset) hits.value = [];
  } finally {
    if (seq === requestSeq) loading.value = false;
  }
}

/**
 * 没有「加载更多」按钮了——列表滚到底部附近就自动续一页。
 * 留着 160px 提前量，滚动时不至于戛然而止。
 */
function onListScroll(event: Event) {
  const el = event.currentTarget as HTMLElement;
  if (el.scrollHeight - el.scrollTop - el.clientHeight > 160) return;
  if (loading.value || loadError.value || exhausted.value) return;
  if (hits.value.length < PAGE) return;
  void load(false);
}

function resetFilters() {
  query.value = "";
  sort.value = "downloads";
  loader.value = "";
  gameVersion.value = "";
  void load(true);
}

async function installProject(hit: BrowseHit) {
  // 整合包会新建版本，不需要目标版本；其余资源必须知道装进哪个版本
  if (section.value !== "modpack" && !state.selectedVersion) {
    notify("请先在「版本管理」里选择一个版本，资源会装进那个版本", "error");
    return;
  }
  installingProject.value = hit.id;
  try {
    const report = await api.browseInstall({
      kind: section.value,
      projectId: hit.id,
      // 搜索时选「全部版本」不代表安装也用全部，装的时候落到最新正式版
      gameVersion: gameVersion.value || state.latest.release,
      loader: loader.value,
      title: hit.title,
      targetVersion: state.selectedVersion || undefined,
    });
    if (report.instance) {
      // 整合包新建了版本，刷新列表并切过去
      await refreshLocal();
      state.selectedVersion = report.instance;
      notify(`整合包「${report.title}」已新建为版本 ${report.instance}`, "success");
    } else if (report.dependencies?.length) {
      // 自动补了前置：说清楚补了哪些，玩家不用猜队列里多出来的是什么
      notify(
        `已加入下载队列：${report.title} ${report.version}，并自动补装前置 ${report.dependencies.join("、")}`,
        "success",
      );
    } else {
      notify(`已加入下载队列：${report.title} ${report.version}`, "success");
    }
  } catch (e) {
    notify(errorText(e), "error");
  } finally {
    installingProject.value = "";
  }
}

// ---- 展示辅助 ----

function humanCount(n: number): string {
  if (n >= 1e8) return `${(n / 1e8).toFixed(2)} 亿`;
  if (n >= 1e4) return `${(n / 1e4).toFixed(1)} 万`;
  return String(n);
}

function ago(iso: string): string {
  if (!iso) return "—";
  const diff = Date.now() - new Date(iso).getTime();
  if (!Number.isFinite(diff)) return "—";
  const day = Math.floor(diff / 86_400_000);
  if (day <= 0) return "今天";
  if (day === 1) return "昨天";
  if (day < 30) return `${day} 天前`;
  if (day < 365) return `${Math.floor(day / 30)} 个月前`;
  return `${Math.floor(day / 365)} 年前`;
}

// ---- 队列 ----

const kindLabel: Record<string, string> = {
  client: "客户端",
  library: "依赖库",
  native: "原生库",
  asset: "资源",
  "asset-index": "索引",
  mod: "模组",
  modpack: "整合包",
  datapack: "数据包",
  resourcepack: "材质包",
  shader: "光影包",
};

const active = computed(() =>
  state.queue.filter((q) => q.state === "running" || q.state === "pending"),
);
const finished = computed(() => state.queue.filter((q) => q.state === "done"));
const problems = computed(() =>
  state.queue.filter((q) => q.state === "failed" || q.state === "cancelled"),
);
const activeGroupCount = computed(() =>
  new Set(active.value.map((q) => q.groupId || q.id)).size,
);

/**
 * 先按一次入队批次聚合，再绘制组摘要；每组展开时最多渲染 40 条文件明细。
 * 这样即使原版安装有几千个资源文件，也不会一次创建几千个 DOM 节点。
 */
const expandedGroups = ref(new Set<string>());
const GROUP_DETAIL_LIMIT = 40;
const visibleGroups = computed(() => {
  const map = new Map<string, QueueItem[]>();
  for (const item of state.queue) {
    const key = item.groupId || item.id;
    const list = map.get(key) ?? [];
    list.push(item);
    map.set(key, list);
  }
  const groups = [...map.entries()].map(([id, items]) => {
    const live = items.filter((q) => q.state === "running" || q.state === "pending");
    const done = items.filter((q) => q.state === "done").length;
    const failed = items.filter((q) => q.state === "failed").length;
    const cancelled = items.filter((q) => q.state === "cancelled").length;
    const doneBytes = items.reduce((sum, q) => sum + (q.state === "done" ? (q.total || q.done) : q.done), 0);
    const totalBytes = items.reduce((sum, q) => sum + (q.total || q.done), 0);
    const detailItems = [
      ...live.slice(0, GROUP_DETAIL_LIMIT),
      ...items.filter((q) => q.state !== "running" && q.state !== "pending").slice(-GROUP_DETAIL_LIMIT).reverse(),
    ].slice(0, GROUP_DETAIL_LIMIT);
    return {
      id,
      label: items[0]?.groupLabel || (items[0] ? kindLabel[items[0].kind] : undefined) || "下载任务组",
      items,
      liveCount: live.length,
      doneCount: done,
      failedCount: failed,
      cancelledCount: cancelled,
      doneBytes,
      totalBytes,
      detailItems,
      hiddenDetails: Math.max(0, items.length - detailItems.length),
      hasActive: live.length > 0,
    };
  });
  const liveGroups = groups.filter((g) => g.hasActive).reverse();
  const settledGroups = groups.filter((g) => !g.hasActive).slice(-15).reverse();
  return [...liveGroups, ...settledGroups];
});
const hiddenGroups = computed(() => {
  const groupCount = new Set(state.queue.map((q) => q.groupId || q.id)).size;
  return Math.max(0, groupCount - visibleGroups.value.length);
});

function groupPercent(group: { doneBytes: number; totalBytes: number; liveCount: number; doneCount: number; items: QueueItem[] }): number {
  if (group.items.length > 0 && group.doneCount === group.items.length) return 1;
  if (!group.totalBytes) return group.liveCount ? 0.03 : 0;
  return Math.min(1, group.doneBytes / group.totalBytes);
}

function toggleGroup(id: string) {
  const next = new Set(expandedGroups.value);
  if (next.has(id)) next.delete(id);
  else next.add(id);
  expandedGroups.value = next;
}

/** 总进度按字节加权，文件数加权会被大量小资源带偏。 */
const totals = computed(() => {
  let done = 0;
  let total = 0;
  for (const item of state.queue) {
    if (item.state === "done") {
      done += item.total || item.done;
      total += item.total || item.done;
    } else if (item.state === "running" || item.state === "pending") {
      done += item.done;
      total += item.total;
    }
  }
  return { done, total };
});

/** 抽屉的开合状态放在全局 store 里，顶栏的「队列」按钮也能直接拉开它。 */
const queueOpen = computed({
  get: () => state.queueOpen,
  set: (value: boolean) => {
    state.queueOpen = value;
  },
});

function percent(item: QueueItem): number {
  if (item.state === "done") return 1;
  if (!item.total) return item.state === "running" ? 0.05 : 0;
  return Math.min(1, item.done / item.total);
}

async function cancel(id: string) {
  try {
    await api.queueCancel(id);
  } catch (e) {
    notify(errorText(e), "error");
  }
}

async function retryFailed(id: string) {
  try {
    const queued = await api.queueRetryFailed(id);
    if (queued) notify("已重新加入下载队列；如果有断点文件，将尽量从已有进度继续", "info");
    else notify("该任务已不处于失败状态，无法重试", "info");
  } catch (e) {
    notify(errorText(e), "error");
  }
}

/** 取消同一批次的所有文件。 */
async function cancelGroup(groupId: string, count: number) {
  try {
    await api.queueCancelGroup(groupId);
    notify(`已取消这组下载（${count} 个文件）`, "info");
  } catch (e) {
    notify(errorText(e), "error");
  }
}

/** 一次性取消所有还没结束的任务。 */
async function cancelAll() {
  const pending = state.queue.filter(
    (q) => q.state === "running" || q.state === "pending",
  );
  if (pending.length === 0) return;
  await Promise.all(pending.map((q) => api.queueCancel(q.id)));
  // 正在下载的任务不能当场掐断（HTTP 流已经在写文件了），只会标记取消、
  // 等当前文件结束再停，提示里要说清楚，不然玩家以为它们已经停了。
  const running = pending.filter((q) => q.state === "running").length;
  notify(
    running > 0
      ? `已取消 ${pending.length} 个任务`
      : `已取消 ${pending.length} 个任务`,
    "info",
  );
}

onMounted(async () => {
  if (state.remote.length === 0) await refreshRemote();
  if (section.value !== "vanilla") void load(true);
});
</script>

<template>
  <!--
    整页固定高度、三段式：页头 / 筛选 → 固定，结果列表 → 内部滚动。
    这样滚动页面时筛选条件不会被推走，列表区域也不会撑出窗口外的滚动条。
  -->
  <div class="flex h-full min-h-0 flex-col gap-2">
    <!-- 页头：分类 tab + 队列，压成一行，把纵向空间留给列表 -->
    <header class="flex shrink-0 flex-wrap items-center gap-2">
      <div class="flex shrink-0 items-center gap-0.5 rounded-xl bg-surface-sunken p-1">
        <button
          v-for="tab in sectionTabs"
          :key="tab.key"
          class="rounded-lg px-2.5 py-1.5 text-[11px] font-semibold transition-[background-color,color,transform] duration-200 ease-spring active:scale-[0.96]"
          :class="
            section === tab.key
              ? 'bg-surface text-brand-200 shadow-sm'
              : 'text-ink-500 hover:text-ink-900'
          "
          @click="openDownload(tab.key)"
        >
          {{ tab.label }}
        </button>
      </div>

      <span
        v-if="isHot && section !== 'vanilla'"
        class="chip shrink-0 bg-amber-500/[0.14] text-amber-300"
      >
        <Icon name="flame" :size="11" filled />
        热门
      </span>
      <p class="hidden min-w-0 flex-1 truncate text-[11px] text-ink-400 xl:block">
        {{ current.hint }}
      </p>

      <button
        class="ml-auto flex shrink-0 items-center gap-1.5 rounded-pill border border-line bg-veil/[0.04] px-3 py-1.5 text-[11px] font-semibold text-ink-500 transition-colors hover:bg-veil/[0.08] hover:text-ink-700"
        @click="queueOpen = !queueOpen"
      >
        <Icon name="download" :size="13" />
        <span v-if="active.length">
          {{ activeGroupCount }} 组下载
          <span class="font-normal text-ink-300">
            {{ humanSize(totals.done) }} / {{ humanSize(totals.total) }}
          </span>
        </span>
        <span v-else>队列空闲</span>
        <Icon
          name="chevron"
          :size="13"
          class="transition-transform duration-200"
          :class="queueOpen ? 'rotate-180' : ''"
        />
      </button>
    </header>

    <!--
      原版游戏：正式版 / 快照左右并排，两张卡各自铺满剩余高度、列表内部滚动。
      整页不滚动。
    -->
    <template v-if="section === 'vanilla'">
      <section class="card flex shrink-0 flex-wrap items-center gap-3 px-5 py-3">
        <span class="chip bg-brand-500/[0.16] text-brand-200">
          最新正式版 {{ state.latest.release || "—" }}
        </span>
        <span class="text-[11px] text-ink-400">
          当前下载源：{{ state.settings.source === "official" ? "Mojang 官方" : "BMCLAPI 镜像" }}
        </span>
        <div class="ml-auto flex items-center gap-2">
          <div class="relative">
            <Icon
              name="search"
              :size="14"
              class="pointer-events-none absolute left-3 top-1/2 -translate-y-1/2 text-ink-300"
            />
            <input
              v-model="vanillaQuery"
              class="field !w-60 !py-2 !pl-8 !text-xs"
              placeholder="过滤版本号，如 1.20"
            />
          </div>
          <button class="btn-ghost btn-sm" @click="refreshRemote">
            <Icon name="refresh" :size="14" />
            刷新清单
          </button>
        </div>
      </section>

      <div
        v-if="!state.remote.length"
        class="card flex min-h-0 flex-1 flex-col items-center justify-center px-4 text-center"
      >
        <div
          class="mb-3 grid h-12 w-12 place-items-center rounded-2xl bg-brand-500/[0.14] text-brand-300"
        >
          <Icon name="refresh" :size="22" />
        </div>
        <div class="text-sm font-bold text-ink-900">正在获取版本清单…</div>
        <p class="mt-1 text-xs text-ink-400">如果一直停在这里，检查下载源或网络。</p>
      </div>

      <div v-else class="grid min-h-0 flex-1 items-start gap-3 lg:grid-cols-[1.55fr_1fr]">
        <!-- 左：正式版（版本多，给更宽的一栏） -->
        <section v-if="releaseList.length" class="card flex min-h-0 flex-col overflow-hidden">
          <div
            class="flex shrink-0 items-center justify-between border-b border-line px-4 py-2.5 text-xs font-bold text-ink-500"
          >
            <span>
              正式版 <span class="font-normal text-ink-300">({{ releaseList.length }})</span>
            </span>
            <span class="font-normal text-ink-300">点版本号选游戏类型（原版 / Fabric / Forge…）</span>
          </div>
          <div
            class="grid min-h-0 max-h-[calc(100vh-21rem)] grid-cols-2 content-start gap-1 overflow-y-auto p-2 md:grid-cols-3"
          >
            <button
              v-for="v in releaseList"
              :key="v.id"
              class="group flex items-center gap-2 rounded-xl px-3 py-2 text-left transition-colors hover:bg-veil/[0.05] disabled:opacity-60 [content-visibility:auto] [contain-intrinsic-size:auto_38px]"
              :disabled="installing === v.id || Boolean(pickerBusy)"
              @click="openPicker(v.id)"
            >
              <span class="shrink-0 whitespace-nowrap text-xs font-bold text-ink-800">{{ v.id }}</span>
              <span
                v-if="v.id === state.latest.release"
                class="chip shrink-0 whitespace-nowrap !px-1.5 !py-0.5 bg-brand-500/[0.16] text-brand-200"
              >
                最新
              </span>
              <span
                v-if="v.installed"
                class="chip shrink-0 whitespace-nowrap !px-1.5 !py-0.5 bg-emerald-500/[0.14] text-emerald-300"
              >
                已装
              </span>
              <span
                class="ml-auto shrink-0 whitespace-nowrap text-[11px] font-semibold text-brand-300 opacity-70 transition-opacity group-hover:opacity-100"
              >
                {{ installing === v.id ? "安装中…" : v.installed ? "" : "安装" }}
              </span>
            </button>
          </div>
        </section>

        <!-- 右：快照 -->
        <section v-if="snapshotList.length" class="card flex min-h-0 max-h-full flex-col self-start overflow-hidden">
          <div
            class="flex shrink-0 items-center justify-between border-b border-line px-4 py-2.5 text-xs font-bold text-ink-500"
          >
            <span>
              快照 <span class="font-normal text-ink-300">({{ snapshotList.length }})</span>
            </span>
            <span class="font-normal text-ink-300">周末版本，可能不稳</span>
          </div>
          <div
            class="grid min-h-0 max-h-[calc(100vh-21rem)] grid-cols-1 content-start gap-1 overflow-y-auto p-2 md:grid-cols-2"
          >
            <button
              v-for="v in snapshotList"
              :key="v.id"
              class="flex items-center gap-2 rounded-xl px-3 py-2 text-left transition-colors hover:bg-veil/[0.05] disabled:opacity-60 [content-visibility:auto] [contain-intrinsic-size:auto_38px]"
              :disabled="installing === v.id || Boolean(pickerBusy)"
              @click="openPicker(v.id)"
            >
              <span class="shrink-0 whitespace-nowrap text-xs font-semibold text-ink-600">{{ v.id }}</span>
              <span
                v-if="v.installed"
                class="chip shrink-0 whitespace-nowrap !px-1.5 !py-0.5 bg-emerald-500/[0.14] text-emerald-300"
              >
                已装
              </span>
              <span class="ml-auto shrink-0 whitespace-nowrap text-[11px] font-semibold text-brand-300 opacity-70">
                {{ installing === v.id ? "安装中…" : "" }}
              </span>
            </button>
          </div>
        </section>
      </div>
    </template>

    <!-- 社区资源 -->
    <template v-else>
      <!-- 筛选条：横向排一行，纵向省下的高度全给下面的列表 -->
      <section class="card shrink-0 px-4 py-3">
        <div class="flex flex-wrap items-end gap-2">
          <label class="block min-w-[180px] flex-1">
            <span class="label">名称</span>
            <input
              v-model="query"
              class="field"
              :placeholder="`搜索 ${current.label}（留空看热门）`"
              @keyup.enter="load(true)"
            />
          </label>
          <label class="block w-[124px] shrink-0">
            <span class="label">游戏版本</span>
            <select v-model="gameVersion" class="field">
              <option v-for="v in gameVersions" :key="v || 'all'" :value="v">
                {{ v || "全部版本" }}
              </option>
            </select>
          </label>
          <label class="block w-[124px] shrink-0">
            <span class="label">加载器</span>
            <select v-model="loader" class="field" :disabled="loaders.length === 1">
              <option v-for="l in loaders" :key="l || 'all'" :value="l">
                {{ l || "全部加载器" }}
              </option>
            </select>
          </label>
          <button
            class="btn-primary h-[42px] shrink-0 !px-4 !py-0 !text-xs"
            :disabled="loading"
            @click="load(true)"
          >
            <Icon name="search" :size="14" />
            搜索
          </button>
          <button class="btn-outline h-[42px] shrink-0 !px-4 !py-0 !text-xs" @click="resetFilters">
            重置
          </button>
        </div>
      </section>

      <!--
        结果列表：卡片自身撑满剩余高度，只有中间的条目区滚动。
        排序挪到这张卡的标题行——放在筛选条里会把筛选挤成两行，白吃一行高度。
      -->
      <section class="card flex min-h-0 flex-1 flex-col overflow-hidden">
        <div
          class="flex shrink-0 items-center gap-3 border-b border-line px-4 py-1.5 text-xs font-bold text-ink-500"
        >
          <span class="shrink-0">
            {{ isHot ? "热门推荐" : "搜索结果" }}
            <span class="ml-1 font-normal text-ink-300">({{ hits.length }})</span>
          </span>

          <span class="min-w-0 flex-1 truncate text-[11px] font-normal text-ink-400">
            <template v-if="section === 'modpack'">
              本地包（.mrpack / .zip）直接拖进窗口即可导入
            </template>
            <template v-else>来源 Modrinth</template>
          </span>

          <div class="flex shrink-0 items-center gap-0.5 rounded-pill bg-surface-sunken p-0.5">
            <button
              v-for="opt in sortOptions"
              :key="opt.value"
              class="rounded-pill px-2.5 py-1 text-[11px] font-semibold transition-[background-color,color,transform] duration-200 ease-spring active:scale-[0.95]"
              :class="
                sort === opt.value
                  ? 'bg-surface text-brand-200 shadow-sm'
                  : 'text-ink-400 hover:text-ink-700'
              "
              @click="pickSort(opt.value)"
            >
              {{ opt.label }}
            </button>
          </div>
        </div>

        <!-- 滚动到底部附近会自动续下一页，不再放「加载更多」按钮 -->
        <div class="scroll-area min-h-0 flex-1 overflow-y-auto" @scroll.passive="onListScroll">
          <div v-if="loadError" class="px-4 py-12 text-center">
            <p class="text-xs font-semibold text-rose-300">{{ loadError }}</p>
            <button class="btn-outline mt-3 btn-sm" @click="load(true)">重试</button>
          </div>

          <div
            v-else-if="loading && hits.length === 0"
            class="space-y-3 px-1 py-4"
          >
            <div class="text-center text-xs text-ink-400">
              正在拉取 {{ current.label }}…
            </div>
            <div v-for="i in 4" :key="`sk-${i}`" class="card flex gap-3 p-3">
              <div class="skeleton h-12 w-12 shrink-0 rounded-xl"></div>
              <div class="flex-1 space-y-2">
                <div class="skeleton h-4 w-2/3"></div>
                <div class="skeleton h-3 w-1/3"></div>
              </div>
            </div>
          </div>

          <div v-else-if="hits.length === 0" class="px-4 py-16 text-center text-xs text-ink-400">
            没有匹配的项目，换个关键词或放宽筛选条件。
          </div>

          <div v-else class="divide-y divide-line">
            <article
              v-for="hit in hits"
              :key="hit.id"
              class="flex gap-3.5 px-5 py-3 transition-colors hover:bg-veil/[0.04]"
            >
              <img
                v-if="hit.iconUrl"
                :src="hit.iconUrl"
                alt=""
                class="h-12 w-12 shrink-0 rounded-xl bg-surface-sunken object-cover"
                loading="lazy"
                referrerpolicy="no-referrer"
              />
              <div
                v-else
                class="grid h-12 w-12 shrink-0 place-items-center rounded-xl bg-brand-500/[0.14] text-brand-300"
              >
                <Icon name="package" :size="22" />
              </div>

              <div class="min-w-0 flex-1">
                <div class="flex items-baseline gap-2">
                  <h3 class="truncate text-sm font-bold text-ink-900">{{ hit.title }}</h3>
                  <span class="truncate text-[11px] text-ink-300">{{ hit.slug }}</span>
                  <span v-if="hit.author" class="truncate text-[11px] text-ink-300">
                    · {{ hit.author }}
                  </span>
                </div>
                <p class="mt-1 line-clamp-2 text-xs leading-relaxed text-ink-500">
                  {{ hit.description }}
                </p>
                <div class="mt-2 flex flex-wrap items-center gap-x-3 gap-y-1 text-[10px] text-ink-400">
                  <span
                    v-for="tag in hit.categories.slice(0, 3)"
                    :key="tag"
                    class="rounded-pill bg-surface-sunken px-1.5 py-0.5 font-semibold text-ink-500"
                  >
                    {{ tag }}
                  </span>
                  <span v-if="hit.versions.length">· {{ hit.versions.join(" / ") }}</span>
                  <span class="inline-flex items-center gap-1">
                    <Icon name="download" :size="11" />
                    {{ humanCount(hit.downloads) }}
                  </span>
                  <span>更新于 {{ ago(hit.updated) }}</span>
                </div>
              </div>

              <button
                class="btn-primary shrink-0 self-center !px-4 !py-1.5 !text-[11px]"
                :disabled="installingProject === hit.id"
                @click="installProject(hit)"
              >
                {{ installingProject === hit.id ? "处理中…" : "安装" }}
              </button>
            </article>
          </div>

          <div
            v-if="loading && hits.length"
            class="py-3 text-center text-[11px] text-ink-400"
          >
            正在加载更多…
          </div>
          <div
            v-else-if="exhausted && hits.length >= PAGE"
            class="py-3 text-center text-[11px] text-ink-300"
          >
            已经到底了
          </div>
        </div>
      </section>
    </template>

    <!-- 加载器选择弹窗：点原版版本先选游戏类型，再开始安装 -->
    <Transition
      enter-active-class="transition duration-150 ease-smooth"
      enter-from-class="opacity-0"
      leave-active-class="transition duration-100"
      leave-to-class="opacity-0"
    >
      <div
        v-if="pickerVersion"
        class="fixed inset-0 z-50 grid place-items-center bg-black/55 p-6"
        @click.self="pickerVersion = ''"
      >
        <div class="card w-full max-w-[420px] p-5">
          <div class="flex items-center gap-2">
            <div class="grid h-8 w-8 shrink-0 place-items-center rounded-xl bg-brand-500/[0.16] text-brand-200">
              <Icon name="package" :size="16" />
            </div>
            <div class="min-w-0 flex-1">
              <h3 class="truncate text-sm font-extrabold text-ink-900">
                安装 Minecraft {{ pickerVersion }}
              </h3>
              <p class="text-[11px] text-ink-400">选择游戏类型；想玩模组就选一个加载器</p>
            </div>
            <button class="text-ink-300 hover:text-ink-700" aria-label="关闭" @click="pickerVersion = ''">
              <Icon name="close" :size="16" />
            </button>
          </div>

          <div class="mt-4 flex flex-col gap-1.5">
            <button
              v-for="choice in pickerChoices"
              :key="choice.id"
              class="flex items-center gap-3 rounded-xl border border-line bg-veil/[0.02] px-3.5 py-2.5 text-left transition-colors hover:bg-veil/[0.06] disabled:opacity-50"
              :disabled="Boolean(pickerBusy) || Boolean(installing)"
              @click="choosePicker(choice.id)"
            >
              <span class="grid h-8 w-8 shrink-0 place-items-center rounded-lg bg-brand-500/[0.12] text-brand-300">
                <Icon :name="choice.icon" :size="15" />
              </span>
              <span class="min-w-0 flex-1">
                <span class="block text-xs font-bold text-ink-800">{{ choice.name }}</span>
                <span class="block text-[10px] text-ink-400">{{ choice.desc }}</span>
              </span>
              <Icon
                v-if="pickerBusy === choice.id"
                name="refresh"
                :size="14"
                class="shrink-0 animate-spin text-brand-300"
              />
              <span v-else-if="pickerBusy" class="shrink-0 text-[10px] text-ink-500">处理中…</span>
            </button>
          </div>

          <p class="mt-3 text-[10px] leading-relaxed text-ink-400">
            Fabric / Quilt 几秒钟即可建好；Forge / NeoForge 需要运行官方安装器，可能要几分钟，进度见右下角下载队列。
          </p>
        </div>
      </div>
    </Transition>

    <!-- 队列抽屉 -->
    <Transition
      enter-active-class="transition duration-250 ease-smooth"
      enter-from-class="translate-y-4 opacity-0"
      leave-active-class="transition duration-150"
      leave-to-class="translate-y-3 opacity-0"
    >
      <div
        v-if="queueOpen"
        class="fixed bottom-6 right-6 z-40 flex max-h-[540px] w-[460px] flex-col overflow-hidden rounded-card border border-line bg-surface shadow-pop"
      >
        <div class="flex items-center gap-2 border-b border-line px-4 py-3">
          <span class="text-xs font-bold text-ink-700">下载队列</span>
          <span class="text-[11px] text-ink-400">
            {{ active.length }} 进行中 · {{ finished.length }} 完成
            <template v-if="problems.length"> · {{ problems.length }} 失败</template>
          </span>
          <button class="ml-auto text-ink-300 hover:text-ink-700" @click="queueOpen = false">
            <Icon name="close" :size="16" />
          </button>
        </div>

        <div class="flex items-center gap-2 border-b border-line px-4 py-2">
          <button class="btn-outline !py-1.5 !text-[11px]" @click="toggleQueuePause">
            <Icon :name="state.paused ? 'play' : 'pause'" :size="12" />
            {{ state.paused ? "继续" : "暂停" }}
          </button>
          <button
            v-if="active.length"
            class="btn-ghost !py-1.5 !text-[11px] text-rose-300 hover:bg-rose-500/[0.12]"
            @click="cancelAll"
          >
            全部取消
          </button>
          <button class="btn-ghost !py-1.5 !text-[11px]" @click="clearFinishedQueue">
            <Icon name="trash" :size="12" />
            清除已完成
          </button>
          <button class="btn-ghost !py-1.5 !text-[11px]" @click="clearLogs">清空日志</button>
        </div>

        <div class="min-h-0 flex-1 overflow-y-auto">
          <div v-if="state.queue.length === 0" class="px-4 py-10 text-center text-xs text-ink-400">
            队列是空的
          </div>
          <section
            v-for="group in visibleGroups"
            :key="group.id"
            class="border-b border-line"
          >
            <div class="flex items-center gap-2 px-4 py-3">
              <button
                class="grid h-6 w-6 shrink-0 place-items-center rounded-lg text-ink-400 hover:bg-veil/[0.06] hover:text-ink-700"
                :aria-label="expandedGroups.has(group.id) ? '收起明细' : '展开明细'"
                @click="toggleGroup(group.id)"
              >
                <Icon name="chevron" :size="13" :class="expandedGroups.has(group.id) ? 'rotate-180' : '-rotate-90'" />
              </button>
              <div class="min-w-0 flex-1">
                <div class="flex items-center gap-2">
                  <span class="truncate text-[11px] font-bold text-ink-800">{{ group.label }}</span>
                  <span class="chip shrink-0 !px-1.5 !py-0.5 bg-surface-sunken text-ink-500">{{ group.items.length }} 个文件</span>
                </div>
                <div class="mt-1 flex flex-wrap gap-x-2 text-[10px] text-ink-400">
                  <span v-if="group.liveCount">{{ group.liveCount }} 进行中</span>
                  <span>{{ group.doneCount }} 完成</span>
                  <span v-if="group.failedCount" class="text-rose-300">{{ group.failedCount }} 失败</span>
                  <span v-if="group.cancelledCount">{{ group.cancelledCount }} 已取消</span>
                  <span>{{ humanSize(group.doneBytes) }} / {{ humanSize(group.totalBytes) }}</span>
                </div>
              </div>
              <span class="shrink-0 text-[10px] font-semibold text-ink-400">{{ (groupPercent(group) * 100).toFixed(0) }}%</span>
              <button
                v-if="group.hasActive"
                class="shrink-0 rounded-lg px-2 py-1 text-[10px] font-bold text-rose-300 hover:bg-rose-500/[0.12]"
                title="取消这一整组下载"
                @click="cancelGroup(group.id, group.liveCount)"
              >
                取消整组
              </button>
            </div>
            <div class="px-4 pb-2">
              <ProgressBar
                :value="groupPercent(group)"
                :height="4"
                :tone="group.failedCount ? 'danger' : groupPercent(group) >= 1 ? 'success' : 'brand'"
              />
            </div>
            <div v-if="expandedGroups.has(group.id)" class="border-t border-line bg-veil/[0.03]">
              <div
                v-for="item in group.detailItems"
                :key="item.id"
                class="border-b border-line px-5 py-2"
              >
                <div class="flex items-center gap-2">
                  <span class="chip shrink-0 !px-1.5 !py-0.5 bg-surface-sunken text-ink-500">{{ kindLabel[item.kind] ?? item.kind }}</span>
                  <span class="min-w-0 flex-1 truncate text-[10px] font-medium text-ink-600">{{ item.label }}</span>
                  <span class="shrink-0 text-[10px] text-ink-400">{{ (percent(item) * 100).toFixed(0) }}%</span>
                  <button
                    v-if="item.state === 'failed'"
                    class="shrink-0 rounded-lg px-2 py-1 text-[10px] font-bold text-brand-500 hover:bg-brand-500/[0.12]"
                    title="重试这个文件"
                    @click="retryFailed(item.id)"
                  >重试</button>
                  <button
                    v-if="item.state === 'running' || item.state === 'pending'"
                    class="shrink-0 rounded-lg px-2 py-1 text-[10px] font-bold text-ink-400 hover:bg-rose-500/[0.14] hover:text-rose-300"
                    title="取消这个文件"
                    @click="cancel(item.id)"
                  >取消文件</button>
                </div>
                <div class="mt-1.5">
                  <ProgressBar
                    :value="percent(item)"
                    :height="2"
                    :tone="item.state === 'failed' ? 'danger' : item.state === 'done' ? 'success' : 'brand'"
                  />
                </div>
                <p v-if="item.error" class="mt-1 break-words text-[10px] text-rose-300">{{ item.error }}</p>
              </div>
              <p v-if="group.hiddenDetails" class="px-5 py-2 text-[10px] text-ink-400">
                为保持界面流畅，当前只显示部分明细，另有 {{ group.hiddenDetails }} 条文件记录。
              </p>
            </div>
          </section>

          <div
            v-if="hiddenGroups > 0"
            class="px-4 py-3 text-center text-[10px] text-ink-500"
          >
            还有 {{ hiddenGroups }} 组较早的已结束记录未显示，点「清除已完成」可以清掉
          </div>
        </div>

        <div
          class="max-h-[120px] shrink-0 overflow-y-auto bg-surface-sunken px-3 py-2 font-mono text-[10px] leading-relaxed"
        >
          <div v-if="state.logs.length === 0" class="text-ink-500">
            // 启动游戏后的日志会出现在这里
          </div>
          <div
            v-for="(log, i) in state.logs.slice(-40)"
            :key="i"
            class="whitespace-pre-wrap break-all"
            :class="
              log.stream === 'err'
                ? 'text-rose-300'
                : log.stream === 'sys'
                  ? 'text-brand-300'
                  : 'text-ink-300'
            "
          >
            {{ log.line }}
          </div>
        </div>
      </div>
    </Transition>
  </div>
</template>
