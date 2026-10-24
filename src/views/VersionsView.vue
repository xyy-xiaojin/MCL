<script setup lang="ts">
import { computed, reactive, ref } from "vue";

import Icon from "../components/Icon.vue";
import { LOADER_LABEL } from "../labels";
import { api, errorText, humanSize } from "../api";
import { notify, refreshLocal, refreshMemory, refreshRemote, state } from "../store";
import type { LocalVersion } from "../types";

const showOnline = ref(false);
const showInstaller = ref(false);
const installerVersion = ref("1.21.1");
const installerLoader = ref("fabric");
const installerJava = ref("auto");
const installerLoaderVersion = ref("latest");
const installerBusy = ref(false);
const loaderChoices = [
  { id: "vanilla", name: "原版 Minecraft", desc: "纯净游戏，不安装模组加载器", icon: "package" },
  { id: "fabric", name: "Fabric", desc: "轻量快速，适合大量现代模组", icon: "bolt" },
  { id: "forge", name: "Forge", desc: "适用于支持 Forge 的模组", icon: "settings" },
  { id: "neoforge", name: "NeoForge", desc: "适用于支持 NeoForge 的模组", icon: "settings" },
  { id: "quilt", name: "Quilt", desc: "另一种轻量模组加载器", icon: "datapack" },
] as const;
const loaderVersions = ref<string[]>([]);
const loadingLoaderVersions = ref(false);
const filter = ref("");
const loadingRemote = ref(false);
const installing = ref("");
const deleting = ref("");

/**
 * 删除版本。
 *
 * 版本目录里的 `game/` 是这个版本私有的（隔离），所以 mods 和存档都会跟着走，
 * 有存档时必须让玩家二次确认——这个操作不可撤销。
 */
async function remove(version: LocalVersion) {
  const warn = version.hasSaves
    ? "\n\n这个版本里有存档，会一起删除且无法恢复。"
    : "";
  const ok = window.confirm(
    `删除版本「${version.id}」？\n将释放 ${humanSize(version.size)}。${warn}`,
  );
  if (!ok) return;

  deleting.value = version.id;
  try {
    const report = await api.deleteVersion(version.id);
    notify(`已删除 ${report.version}，释放 ${humanSize(report.freed)}`, "success");
    const wasSelected = state.selectedVersion === version.id;
    await refreshLocal();
    // 远程清单的 installed 标志是按本地目录算的，删完要重拉一次，否则还显示"已安装"
    await refreshRemote();
    if (wasSelected) {
      state.selectedVersion = state.local[0]?.id ?? "";
      await refreshMemory();
    }
  } catch (e) {
    notify(errorText(e), "error");
  } finally {
    deleting.value = "";
  }
}

const filtered = computed(() => {
  const query = filter.value.trim().toLowerCase();
  const list = query
    ? state.remote.filter((v) => v.id.toLowerCase().includes(query))
    : state.remote;
  // 版本清单有上千条，只渲染前 60 条避免不必要的 DOM
  return list.slice(0, 60);
});

const inQueue = computed(
  () => state.queue.filter((q) => q.state === "running" || q.state === "pending").length,
);

async function openOnline() {
  showOnline.value = true;
  if (state.remote.length === 0) {
    await loadRemote();
  }
}

async function loadRemote() {
  loadingRemote.value = true;
  try {
    await refreshRemote();
  } finally {
    loadingRemote.value = false;
  }
}

function openInstaller() {
  showInstaller.value = true;
  if (state.remote.length === 0) void loadRemote();
  void loadInstallerLoaderVersions();
}

const requiredJavaMajor = (version: string) => {
  const match = version.match(/^(\d+)\.(\d+)(?:\.(\d+))?/);
  if (!match) return 21;
  const major = Number(match[1]);
  const minor = Number(match[2]);
  const patch = Number(match[3] ?? 0);
  if (major >= 2 || (major === 1 && (minor >= 21 || (minor === 20 && patch >= 5)))) return 21;
  if (major === 1 && minor >= 18) return 17;
  if (major === 1 && minor === 17) return 16;
  return 8;
};

async function loadInstallerLoaderVersions() {
  loaderVersions.value = [];
  if (installerLoader.value === "vanilla") return;
  loadingLoaderVersions.value = true;
  try {
    loaderVersions.value = await api.listLoaderVersions(installerLoader.value, installerVersion.value);
  } catch (e) {
    // 版本清单暂时不可用时仍允许使用后端的稳定版自动选择。
    notify(`暂时无法获取加载器版本列表：${errorText(e)}`, "error");
  } finally {
    loadingLoaderVersions.value = false;
  }
}

async function installConfiguredGame() {
  if (installerBusy.value) return;
  installerBusy.value = true;
  const gameVersion = installerVersion.value.trim();
  try {
    if (!gameVersion) throw new Error("请选择 Minecraft 版本");

    // Java 自动策略：按 Minecraft 元数据规则选择所需主版本；缺少匹配版本时下载 Temurin JRE。
    const javaMajor = requiredJavaMajor(gameVersion);
    const currentJava = await api.detectJava(true);
    if (installerJava.value === "auto") {
      if (!currentJava || currentJava.major !== javaMajor) {
        notify(`正在准备 Java ${javaMajor} 运行环境…`, "info");
        await api.downloadJava(javaMajor);
      }
    } else if (!currentJava || currentJava.major !== javaMajor) {
      throw new Error(`当前检测到的 Java${currentJava ? ` ${currentJava.major}` : " 不可用"}，但 Minecraft ${gameVersion} 需要 Java ${javaMajor}。请改用“自动检测并准备所需 Java”，或先配置匹配的 Java。`);
    }

    const isInstalled = state.local.some((v) => v.id === gameVersion && v.complete);
    if (!isInstalled) {
      notify(`正在安装 Minecraft ${gameVersion} 原版文件…`, "info");
      const report = await api.installVersion(gameVersion);
      notify(`Minecraft ${report.version} 原版文件已就绪`, "success");
    }

    if (installerLoader.value !== "vanilla") {
      await refreshLocal();
      const existingLoaderVersion = state.local.find((v) =>
        v.complete && v.id.startsWith(`${installerLoader.value}-loader-`) && v.id.endsWith(`-${gameVersion}`),
      );
      if (existingLoaderVersion) {
        await refreshRemote();
        state.selectedVersion = existingLoaderVersion.id;
        notify(`已存在可用版本：${existingLoaderVersion.id}`, "success");
        showInstaller.value = false;
        return;
      }
      const selectedLoaderVersion = installerLoaderVersion.value === "latest"
        ? undefined
        : installerLoaderVersion.value;
      const report = await api.installLoader(installerLoader.value, gameVersion, selectedLoaderVersion);
      await refreshLocal();
      await refreshRemote();
      state.selectedVersion = report.versionId;
      if (report.warning) notify(report.warning, "error");
      else notify(`安装完成：${report.versionId}`, "success");
    } else {
      await refreshLocal();
      await refreshRemote();
      state.selectedVersion = gameVersion;
      notify(`安装完成：Minecraft ${gameVersion}（原版）`, "success");
    }
    showInstaller.value = false;
  } catch (e) {
    notify(errorText(e), "error");
  } finally {
    installerBusy.value = false;
  }
}

async function install(id: string) {
  if (installing.value) return;
  installing.value = id;
  try {
    const report = await api.installVersion(id);
    const extra = report.legacy > 0 ? `，老版本资源 ${report.legacy} 个` : "";
    notify(
      `${report.version} 安装完成：${report.files} 个文件 / ${humanSize(report.bytes)}${extra}`,
      "success",
    );
    await refreshLocal();
    // 装完要重拉远程清单，否则面板里还显示"安装"按钮而不是"已安装"
    await refreshRemote();
    state.selectedVersion = report.version;
  } catch (e) {
    notify(errorText(e), "error");
  } finally {
    installing.value = "";
  }
}

function startVersion(id: string) {
  state.selectedVersion = id;
  state.view = "home";
}

// ---- 导出实例 ----
//
// 导出按"这个版本"来算，不是"当前选中的版本"：玩家可能想备份一个没在用的包，
// 也可能对比两个包的差异，所以入口挂在每张版本卡片上。

const exportTarget = ref<LocalVersion | null>(null);
const exporting = ref(false);
const exportName = ref("");

const exportPicks = reactive({
  includeMods: true,
  includeConfig: true,
  includeSaves: false,
  includeResourcepacks: false,
  includeShaderpacks: false,
});

const exportChoices = [
  { key: "includeMods", label: "模组" },
  { key: "includeConfig", label: "配置" },
  { key: "includeSaves", label: "存档" },
  { key: "includeResourcepacks", label: "材质" },
  { key: "includeShaderpacks", label: "光影" },
] as const;

function openExport(version: LocalVersion) {
  exportTarget.value = version;
  exportName.value = "";
  // 存档默认不带：分享备份时最容易夹带出去的就是存档
  exportPicks.includeSaves = false;
}

function closeExport() {
  if (exporting.value) return;
  exportTarget.value = null;
}

async function exportNow() {
  const target = exportTarget.value;
  if (!target || exporting.value) return;
  exporting.value = true;
  try {
    const report = await api.exportInstance(
      { name: exportName.value.trim() || target.id, ...exportPicks },
      target.id,
    );
    notify(
      `已导出 ${report.files} 个文件（${humanSize(report.bytes)}）到 ${report.path}`,
      "success",
    );
    exportTarget.value = null;
  } catch (e) {
    notify(errorText(e), "error");
  } finally {
    exporting.value = false;
  }
}

// ---- 加载器 ----

const loaderBusy = ref("");

/**
 * 在原版之上装一个加载器，生成独立版本。
 *
 * Fabric / Quilt 的 meta 服务直接给完整 version.json，几秒钟建好；
 * Forge / NeoForge 走官方安装器（后端自动补 Java、跑 --installClient），
 * 要下载并跑几十个处理器，几分钟属正常，进度在「下载」页看安装器本体。
 */
async function installLoader(gameVersion: string, loader: string) {
  if (loaderBusy.value) return;
  loaderBusy.value = `${loader}:${gameVersion}`;
  try {
    const report = await api.installLoader(loader, gameVersion);
    notify(
      `已新建版本「${report.versionId}」：${report.gameVersion} + ${LOADER_LABEL[report.loader] ?? report.loader} ${report.loaderVersion}`,
      "success",
    );
    await refreshLocal();
    await refreshRemote();
    state.selectedVersion = report.versionId;
    // 类库没下全时后端会带一条 warning，别把版本说成"可以直接玩"
    if (report.warning) notify(report.warning, "error");
  } catch (e) {
    notify(errorText(e), "error");
  } finally {
    loaderBusy.value = "";
  }
}
</script>

<template>
  <div class="flex h-full min-h-0 flex-col gap-4">
    <!-- 工具条 -->
    <div class="flex shrink-0 flex-wrap items-center justify-between gap-3">
      <div class="flex items-center gap-2">
        <span class="chip bg-brand-500/[0.16] text-brand-200">
          已安装 {{ state.local.length }} 个
        </span>
        <span v-if="inQueue" class="chip bg-amber-500/[0.14] text-amber-300">
          {{ inQueue }} 个任务进行中
        </span>
      </div>
      <div class="flex items-center gap-2">
        <button class="btn-ghost !py-2 !text-xs" @click="refreshLocal">
          <Icon name="refresh" :size="15" />
          刷新
        </button>
        <button class="btn-outline !py-2 !text-xs" @click="openInstaller">
          <Icon name="download" :size="15" />
          安装游戏…
        </button>
        <button class="btn-primary !py-2 !text-xs" @click="openOnline">
          <Icon name="versions" :size="15" />
          版本列表
        </button>
      </div>
    </div>

    <div class="min-h-0 flex-1 space-y-4 overflow-y-auto pr-1">
    <!-- 本地版本卡片 -->
    <div v-if="state.local.length > 0" class="grid gap-3 md:grid-cols-2 xl:grid-cols-3">
      <article
        v-for="version in state.local"
        :key="version.id"
        class="card card-hover group flex flex-col gap-3 p-4"
      >
        <div class="flex items-start justify-between gap-2">
          <div class="min-w-0">
            <div class="truncate text-sm font-bold text-ink-900" :title="version.id">
              {{ version.id }}
            </div>
            <div class="mt-1 flex items-center gap-2">
              <span
                class="chip"
                :class="
                  version.complete
                    ? 'bg-brand-500/[0.16] text-brand-200'
                    : 'bg-amber-500/[0.14] text-amber-300'
                "
              >
                {{ version.complete ? version.kind : "安装未完成" }}
              </span>
              <span class="chip bg-surface-sunken text-ink-500">
                Java {{ version.javaMajor }}
              </span>
              <span class="chip bg-surface-sunken text-ink-500">
                {{ humanSize(version.size) }}
              </span>
            </div>
          </div>
          <div
            class="grid h-9 w-9 shrink-0 place-items-center rounded-xl bg-brand-500/[0.16] text-brand-200 transition-transform duration-300 group-hover:scale-105"
          >
            <Icon name="package" :size="18" />
          </div>
        </div>

        <div
          class="truncate rounded-lg bg-surface-sunken px-2.5 py-1.5 font-mono text-[10px] text-ink-400"
          :title="version.dir"
        >
          {{ version.dir }}
        </div>

        <div class="flex gap-2">
          <button
            class="btn-outline flex-1 !py-2 !text-xs"
            :disabled="!version.complete"
            :title="version.complete ? '' : '安装未完成，请重新安装'"
            @click="startVersion(version.id)"
          >
            <Icon name="play" :size="14" />
            选择并启动
          </button>
          <button
            class="btn-ghost !px-2.5 !py-2 !text-xs"
            title="把这个版本打包成 zip（导出实例）"
            @click="openExport(version)"
          >
            导出
          </button>
          <button
            class="btn-ghost !px-2.5 !py-2 !text-xs text-rose-400/80 hover:bg-rose-500/[0.14] hover:text-rose-300"
            title="删除这个版本"
            :disabled="deleting === version.id"
            @click="remove(version)"
          >
            <Icon name="trash" :size="15" />
          </button>
        </div>
      </article>
    </div>

    <!-- 空状态 -->
    <div v-else class="card flex flex-col items-center gap-3 p-12 text-center">
      <div class="grid h-14 w-14 place-items-center rounded-2xl bg-brand-500/[0.14] text-brand-300">
        <Icon name="versions" :size="26" />
      </div>
      <div class="text-sm font-bold text-ink-900">还没有安装任何版本</div>
      <p class="max-w-md text-xs leading-relaxed text-ink-400">
        第一次使用，建议先点「安装游戏」，选择 Minecraft 版本和游戏类型（原版或 Fabric / Forge）。
        MCL 会自动下载需要的文件；进度可以在「下载」页查看。已有整合包文件时，也可以把
        <code class="rounded bg-surface-sunken px-1">.zip</code> 或
        <code class="rounded bg-surface-sunken px-1">.mrpack</code> 拖进窗口导入。
      </p>
      <div class="flex flex-wrap justify-center gap-2">
        <button class="btn-primary" @click="openInstaller">
          <Icon name="download" :size="16" />
          安装游戏（推荐）
        </button>
        <button class="btn-ghost" @click="openOnline">
          <Icon name="versions" :size="15" />
          高级：浏览全部版本
        </button>
      </div>
    </div>

    <!-- 安装向导：分区式布局，优先展示常用版本和加载器选择 -->
    <section v-if="showInstaller" class="card overflow-hidden">
      <header class="border-b border-line bg-surface-sunken/60 p-5">
        <div class="flex items-start justify-between gap-3">
          <div class="flex items-start gap-3">
            <div class="grid h-11 w-11 shrink-0 place-items-center rounded-2xl bg-brand-500/[0.16] text-brand-200">
              <Icon name="download" :size="21" />
            </div>
            <div>
              <h2 class="text-lg font-extrabold text-ink-900">安装 Minecraft</h2>
              <p class="mt-1 text-xs leading-relaxed text-ink-400">选择游戏版本和加载器。MCL 会自动准备基础文件与依赖，安装进度可在「下载」页查看。</p>
            </div>
          </div>
          <button class="btn-ghost !px-2 !py-1.5" :disabled="installerBusy" @click="showInstaller = false" aria-label="关闭安装向导">
            <Icon name="close" :size="15" />
          </button>
        </div>
      </header>

      <div class="grid gap-0 lg:grid-cols-[minmax(0,1fr)_280px]">
        <div class="space-y-5 p-5">
          <section>
            <div class="mb-2 flex items-center gap-2">
              <span class="grid h-6 w-6 place-items-center rounded-full bg-brand-500/[0.16] text-[11px] font-bold text-brand-200">1</span>
              <h3 class="text-sm font-bold text-ink-900">选择 Minecraft 版本</h3>
            </div>
            <label class="block text-xs font-semibold text-ink-500">
              游戏版本
              <select v-model="installerVersion" class="field mt-1.5 w-full" :disabled="installerBusy" @change="loadInstallerLoaderVersions">
                <option v-for="version in state.remote.filter(v => v.kind === 'release')" :key="version.id" :value="version.id">{{ version.id }}{{ version.id === state.latest.release ? '（最新正式版）' : '' }}</option>
                <option v-for="version in state.remote.filter(v => v.kind !== 'release')" :key="version.id" :value="version.id">{{ version.id }}（{{ version.kind }}）</option>
                <option v-if="!state.remote.some(v => v.id === '1.21.1')" value="1.21.1">1.21.1</option>
              </select>
            </label>
            <p class="mt-1.5 text-[11px] text-ink-400">默认 Minecraft 1.21.1。新手建议选正式版；快照版可能不兼容部分模组。</p>
          </section>

          <section>
            <div class="mb-2 flex items-center gap-2">
              <span class="grid h-6 w-6 place-items-center rounded-full bg-brand-500/[0.16] text-[11px] font-bold text-brand-200">2</span>
              <h3 class="text-sm font-bold text-ink-900">选择游戏类型</h3>
              <span class="text-[10px] text-ink-400">不确定就选 Fabric；只玩原版则选「原版 Minecraft」</span>
            </div>
            <div class="grid gap-2 sm:grid-cols-2">
              <button v-for="item in loaderChoices"
                :key="item.id"
                type="button"
                class="flex min-h-[76px] items-start gap-3 rounded-xl border p-3 text-left transition-colors"
                :class="installerLoader === item.id ? 'border-brand-400/70 bg-brand-500/[0.10] ring-1 ring-brand-400/30' : 'border-line bg-surface-sunken/40 hover:border-brand-400/40'"
                :disabled="installerBusy"
                @click="installerLoader = item.id; installerLoaderVersion = 'latest'; loadInstallerLoaderVersions()"
              >
                <span class="mt-0.5 grid h-8 w-8 shrink-0 place-items-center rounded-lg" :class="installerLoader === item.id ? 'bg-brand-500/[0.18] text-brand-200' : 'bg-surface-sunken text-ink-400'">
                  <Icon :name="item.icon" :size="16" />
                </span>
                <span class="min-w-0 flex-1">
                  <span class="block text-xs font-bold" :class="installerLoader === item.id ? 'text-brand-200' : 'text-ink-900'">{{ item.name }}</span>
                  <span class="mt-1 block text-[10px] leading-relaxed text-ink-400">{{ item.desc }}</span>
                </span>
                <Icon v-if="installerLoader === item.id" name="check" :size="15" class="shrink-0 text-brand-300" />
              </button>
            </div>
          </section>

          <section class="grid gap-3 sm:grid-cols-2">
            <label v-if="installerLoader !== 'vanilla'" class="block text-xs font-semibold text-ink-500">
              加载器版本
              <select v-model="installerLoaderVersion" class="field mt-1.5 w-full" :disabled="installerBusy || loadingLoaderVersions">
                <option value="latest">自动选择推荐稳定版</option>
                <option v-for="version in loaderVersions" :key="version" :value="version">{{ version }}</option>
              </select>
              <span class="mt-1 block text-[10px] text-ink-400">{{ loadingLoaderVersions ? '正在获取真实版本列表…' : `可用版本：${loaderVersions.length} 个` }}</span>
            </label>
            <label class="block text-xs font-semibold text-ink-500">
              Java 环境
              <select v-model="installerJava" class="field mt-1.5 w-full" :disabled="installerBusy">
                <option value="auto">自动检测并准备所需 Java</option>
                <option value="existing">仅使用已有 Java</option>
              </select>
              <span class="mt-1 block text-[10px] text-ink-400">根据 Minecraft 版本自动匹配 Java 主版本。</span>
            </label>
          </section>
        </div>

        <aside class="flex flex-col border-t border-line bg-surface-sunken/45 p-5 lg:border-l lg:border-t-0">
          <h3 class="text-sm font-bold text-ink-900">安装摘要</h3>
          <div class="mt-3 rounded-xl border border-line bg-surface p-3">
            <div class="text-[10px] font-semibold uppercase tracking-wider text-ink-400">Minecraft 版本</div>
            <div class="mt-1 text-xl font-extrabold text-ink-900">{{ installerVersion }}</div>
            <div class="mt-3 border-t border-line pt-3 text-[10px] font-semibold uppercase tracking-wider text-ink-400">游戏类型</div>
            <div class="mt-1 text-sm font-bold text-ink-900">{{ ({ vanilla: '原版', fabric: 'Fabric', forge: 'Forge', neoforge: 'NeoForge', quilt: 'Quilt' } as Record<string, string>)[installerLoader] }}</div>
            <div v-if="installerLoader !== 'vanilla'" class="mt-1 text-[11px] text-ink-400">{{ installerLoaderVersion === 'latest' ? '推荐稳定版' : installerLoaderVersion }}</div>
          </div>
          <ul class="mt-3 space-y-2 text-[11px] leading-relaxed text-ink-400">
            <li class="flex gap-2"><Icon name="check" :size="14" class="mt-0.5 shrink-0 text-emerald-400" />自动下载缺失的游戏文件与依赖</li>
            <li class="flex gap-2"><Icon name="check" :size="14" class="mt-0.5 shrink-0 text-emerald-400" />加载器安装为独立版本</li>
            <li class="flex gap-2"><Icon name="check" :size="14" class="mt-0.5 shrink-0 text-emerald-400" />尽可能复用已下载的资源</li>
          </ul>
          <div class="mt-auto pt-5">
            <button class="btn-primary w-full justify-center !py-3" :disabled="installerBusy || loadingLoaderVersions" @click="installConfiguredGame">
              <Icon :name="installerBusy ? 'refresh' : 'download'" :size="16" :class="installerBusy ? 'animate-spin' : ''" />
              {{ installerBusy ? '安装中…' : '开始安装' }}
            </button>
            <button class="btn-ghost mt-2 w-full justify-center !py-2 text-xs" :disabled="installerBusy" @click="showInstaller = false">取消</button>
            <p class="mt-2 text-center text-[10px] text-ink-400">安装期间可继续使用启动器；到「下载」页查看整体进度。首次安装文件较多，请耐心等待。</p>
          </div>
        </aside>
      </div>
    </section>

    <!-- 在线安装面板 -->
    <section v-if="showOnline" class="card overflow-hidden">
      <div class="flex flex-wrap items-center gap-3 border-b border-line p-4">
        <div class="relative flex-1 min-w-[200px]">
          <Icon
            name="search"
            :size="16"
            class="pointer-events-none absolute left-3 top-1/2 -translate-y-1/2 text-ink-300"
          />
          <input
            v-model="filter"
            class="field !pl-9"
            placeholder="搜索版本，例如 1.20.1"
          />
        </div>
        <button class="btn-ghost !py-2 !text-xs" :disabled="loadingRemote" @click="loadRemote">
          <Icon
            name="refresh"
            :size="15"
            :class="loadingRemote ? 'animate-spin' : ''"
          />
          {{ loadingRemote ? "加载中…" : "刷新清单" }}
        </button>
        <button class="btn-ghost !py-2 !text-xs" @click="showOnline = false">
          <Icon name="close" :size="15" />
          收起
        </button>
      </div>

      <div class="max-h-[420px] divide-y divide-line/80 overflow-y-auto">
        <div
          v-for="version in filtered"
          :key="version.id"
          class="flex items-center gap-3 px-4 py-2.5 transition-colors duration-150 hover:bg-white/[0.05]"
        >
          <span
            class="chip w-[74px] justify-center"
            :class="
              version.kind === 'release'
                ? 'bg-emerald-500/[0.14] text-emerald-300'
                : 'bg-surface-sunken text-ink-500'
            "
          >
            {{ version.kind === "release" ? "正式版" : version.kind }}
          </span>
          <span class="flex-1 truncate text-sm font-semibold text-ink-900">
            {{ version.id }}
          </span>
          <span
            v-if="version.id === state.latest.release"
            class="chip bg-brand-600 text-white"
          >
            最新正式版
          </span>
          <span v-if="version.installed" class="text-[11px] font-semibold text-emerald-400">
            已安装
          </span>
          <button
            v-if="version.installed"
            class="btn-outline !px-2.5 !py-1.5 !text-[11px]"
            :disabled="Boolean(loaderBusy)"
            title="在原版之上装 Fabric，生成一个可装 mod 的新版本"
            @click="installLoader(version.id, 'fabric')"
          >
            <Icon
              :name="loaderBusy === `fabric:${version.id}` ? 'refresh' : 'bolt'"
              :size="12"
              :class="loaderBusy === `fabric:${version.id}` ? 'animate-spin' : ''"
            />
            Fabric
          </button>
          <button
            v-if="version.installed"
            class="btn-outline !px-2.5 !py-1.5 !text-[11px]"
            :disabled="Boolean(loaderBusy)"
            title="在原版之上装 Quilt"
            @click="installLoader(version.id, 'quilt')"
          >
            Quilt
          </button>
          <button
            v-if="version.installed"
            class="btn-outline !px-2.5 !py-1.5 !text-[11px]"
            :disabled="Boolean(loaderBusy)"
            title="跑官方 Forge 安装器（1.13+），要几分钟，进度看「下载」页"
            @click="installLoader(version.id, 'forge')"
          >
            <Icon
              :name="loaderBusy === `forge:${version.id}` ? 'refresh' : 'bolt'"
              :size="12"
              :class="loaderBusy === `forge:${version.id}` ? 'animate-spin' : ''"
            />
            Forge
          </button>
          <button
            v-if="version.installed"
            class="btn-outline !px-2.5 !py-1.5 !text-[11px]"
            :disabled="Boolean(loaderBusy)"
            title="跑官方 NeoForge 安装器（1.20.2+），要几分钟；国内直连 NeoForge maven 可能较慢"
            @click="installLoader(version.id, 'neoforge')"
          >
            NeoForge
          </button>
          <button
            v-else
            class="btn-primary !px-3 !py-1.5 !text-[11px]"
            :disabled="Boolean(installing)"
            @click="install(version.id)"
          >
            <Icon
              :name="installing === version.id ? 'refresh' : 'download'"
              :size="13"
              :class="installing === version.id ? 'animate-spin' : ''"
            />
            {{ installing === version.id ? "安装中" : "安装" }}
          </button>
        </div>

        <div v-if="filtered.length === 0" class="px-4 py-10 text-center text-xs text-ink-400">
          {{ state.remote.length === 0 ? "还没有加载版本清单" : "没有匹配的版本" }}
        </div>
      </div>

      <p class="flex flex-wrap items-center gap-x-3 border-t border-line px-4 py-2.5 text-[11px] text-ink-300">
        <span>安装会下载客户端 jar、全部依赖库与资源文件，进度实时显示在「下载」页。</span>
        <span v-if="state.latest.release">
          最新正式版 {{ state.latest.release }}
          <template v-if="state.latest.snapshot"> · 最新快照 {{ state.latest.snapshot }}</template>
        </span>
      </p>
    </section>

    <!-- 导出面板：只对当前点开的那个版本生效 -->
    <div
      v-if="exportTarget"
      class="fixed inset-0 z-[70] flex items-center justify-center bg-black/65 p-6 backdrop-blur-sm"
      @click.self="closeExport"
    >
      <div class="w-full max-w-md rounded-2xl border border-line bg-surface p-5 shadow-pop">
        <div class="flex items-start justify-between gap-3">
          <div class="min-w-0">
            <div class="text-sm font-extrabold text-ink-900">导出实例</div>
            <p class="mt-1 truncate text-[11px] text-ink-400" :title="exportTarget.dir">
              {{ exportTarget.id }} · {{ humanSize(exportTarget.size) }}
            </p>
          </div>
          <button class="btn-ghost !px-2 !py-1.5" title="关闭" @click="closeExport">
            <Icon name="close" :size="15" />
          </button>
        </div>

        <label class="mt-4 block text-[11px] font-semibold text-ink-500">包名</label>
        <input
          v-model="exportName"
          class="field mt-1.5 w-full !text-xs"
          :placeholder="`留空则用「${exportTarget.id}」`"
          maxlength="40"
          @keyup.enter="exportNow"
        />

        <div class="mt-3 flex flex-wrap gap-x-4 gap-y-2">
          <label
            v-for="choice in exportChoices"
            :key="choice.key"
            class="flex cursor-pointer items-center gap-1.5 text-[11px] font-semibold text-ink-500"
          >
            <input
              v-model="exportPicks[choice.key]"
              type="checkbox"
              class="h-3.5 w-3.5 accent-brand-600"
            />
            {{ choice.label }}
          </label>
        </div>

        <p class="mt-3 text-[11px] leading-relaxed text-ink-400">
          打包成 zip 放在实例目录的
          <code class="rounded bg-surface-sunken px-1">exports/</code>
          下，只含勾选的内容。不勾存档就不会把存档带走。
        </p>

        <div class="mt-4 flex justify-end gap-2">
          <button class="btn-ghost !py-2 !text-xs" :disabled="exporting" @click="closeExport">
            取消
          </button>
          <button class="btn-primary !py-2 !text-xs" :disabled="exporting" @click="exportNow">
            <Icon
              :name="exporting ? 'refresh' : 'download'"
              :size="15"
              :class="exporting ? 'animate-spin' : ''"
            />
            {{ exporting ? "导出中…" : "导出 zip" }}
          </button>
        </div>
      </div>
    </div>
    </div>
  </div>
</template>
