<script setup lang="ts">
import { computed, onMounted, onUnmounted, reactive, ref } from "vue";

import Icon from "../components/Icon.vue";
import { LOADER_LABEL } from "../labels";
import { api, errorText, humanSize } from "../api";
import { notify, refreshLocal, refreshMemory, refreshRemote, state } from "../store";
import type { LocalVersion, SavesBackup } from "../types";

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
    const backupNote = report.savesBackup
      ? `；存档已自动备份（版本卡的「备份」里可还原）`
      : "";
    notify(
      `已删除 ${report.version}，释放 ${humanSize(report.freed)}${backupNote}`,
      "success",
    );
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

const releaseVersions = computed(() => state.remote.filter((v) => v.kind === "release"));
const nonReleaseVersions = computed(() => state.remote.filter((v) => v.kind !== "release"));

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

/**
 * 加载器版本列表的请求代际号：快速切换加载器/游戏版本时，
 * 慢的旧响应不能覆盖新响应（否则下拉里显示的是别的组合的版本）。
 */
let loaderVersionsSeq = 0;

async function loadInstallerLoaderVersions() {
  const seq = ++loaderVersionsSeq;
  loaderVersions.value = [];
  if (installerLoader.value === "vanilla") return;
  loadingLoaderVersions.value = true;
  try {
    const list = await api.listLoaderVersions(installerLoader.value, installerVersion.value);
    if (seq !== loaderVersionsSeq) return;
    loaderVersions.value = list;
  } catch (e) {
    if (seq !== loaderVersionsSeq) return;
    // 版本清单暂时不可用时仍允许使用后端的稳定版自动选择。
    notify(`暂时无法获取加载器版本列表：${errorText(e)}`, "error");
  } finally {
    if (seq === loaderVersionsSeq) loadingLoaderVersions.value = false;
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
      // 「已装过就复用」只对 Fabric / Quilt 生效：它们的版本 ID 是我们自己
      // 拼的 `<loader>-loader-<v>-<game>`，格式确定。Forge / NeoForge 的目录
      // 名是官方安装器产的（`1.20.1-forge-47.2.0`），前缀对不上，强行匹配
      // 只会漏判然后每次重跑安装器——宁可让后端自己判重。
      const existingLoaderVersion =
        installerLoader.value === "fabric" || installerLoader.value === "quilt"
          ? state.local.find((v) =>
              v.complete &&
              v.id.startsWith(`${installerLoader.value}-loader-`) &&
              v.id.endsWith(`-${gameVersion}`),
            )
          : undefined;
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

// ---- 版本卡「更多」菜单：复制 / 重命名（低频操作收进菜单，卡面保持简洁） ----

const menuOpen = ref("");

function toggleMenu(id: string) {
  menuOpen.value = menuOpen.value === id ? "" : id;
}

// 点到菜单以外的地方就收起（点开菜单后点页面任何空白都该关掉）
function onDocumentClick(event: MouseEvent) {
  const target = event.target as HTMLElement | null;
  if (!target?.closest("[data-ver-menu]")) {
    menuOpen.value = "";
  }
}
onMounted(() => document.addEventListener("click", onDocumentClick));
onUnmounted(() => document.removeEventListener("click", onDocumentClick));

/** 复制/重命名共用的小输入弹层 */
const idEditor = ref<{
  version: LocalVersion;
  mode: "copy" | "rename";
  value: string;
} | null>(null);
const idBusy = ref(false);

function startCopy(version: LocalVersion) {
  menuOpen.value = "";
  idEditor.value = { version, mode: "copy", value: `${version.id}-copy` };
}

function startRename(version: LocalVersion) {
  menuOpen.value = "";
  idEditor.value = { version, mode: "rename", value: version.id };
}

const repairing = ref("");

async function repairVersion(version: LocalVersion) {
  menuOpen.value = "";
  if (repairing.value) return;
  const ok = window.confirm(
    `校验并修复「${version.id}」的游戏文件？\n健康的文件会跳过、只重新下载缺失或损坏的；存档、mod、配置一律不动。`,
  );
  if (!ok) return;
  repairing.value = version.id;
  notify(`正在校验「${version.id}」的游戏文件…`, "info");
  try {
    const report = await api.repairVersion(version.id);
    notify(`修复完成：校验 ${report.files} 个文件（完好的已跳过）`, "success");
    await refreshLocal();
  } catch (e) {
    notify(errorText(e), "error");
  } finally {
    repairing.value = "";
  }
}

async function applyIdEdit() {
  const editor = idEditor.value;
  if (!editor || idBusy.value) return;
  idBusy.value = true;
  try {
    const newId = editor.value.trim();
    if (!newId) {
      notify("名字不能为空", "error");
      return;
    }
    if (editor.mode === "copy") {
      await api.copyVersion(editor.version.id, newId);
      notify(`已复制为「${newId}」（原版未动，可以放心折腾副本）`, "success");
    } else {
      await api.renameVersion(editor.version.id, newId);
      notify(`已改名为「${newId}」`, "success");
    }
    // 只关"还是当初那个"的弹层：大版本复制耗时几秒，期间玩家可能
    // 关掉重开过，晚到的完成不能把新弹层误关掉
    if (idEditor.value === editor) {
      idEditor.value = null;
      state.selectedVersion = newId;
    }
    await refreshLocal();
    await refreshRemote();
  } catch (e) {
    notify(errorText(e), "error");
  } finally {
    idBusy.value = false;
  }
}

// ---- 存档备份 ----
//
// 入口只有一个「备份」小按钮，弹层里看历史/立即备份/还原。
// 有存档的版本卡片按钮上带个小红点提醒（存档 = 不可再生数据）。

const backupTarget = ref<LocalVersion | null>(null);
const backups = ref<SavesBackup[]>([]);
const backupBusy = ref(false);

function openBackups(version: LocalVersion) {
  backupTarget.value = version;
  backups.value = [];
  void loadBackups();
}

// 列表请求代际号：快速切换不同版本的备份弹层时，
// 慢的旧响应不能覆盖新目标（否则显示 B 的名字、列着 A 的备份）
let backupsSeq = 0;

async function loadBackups() {
  if (!backupTarget.value) return;
  const seq = ++backupsSeq;
  try {
    const list = await api.backupList(backupTarget.value.id);
    if (seq === backupsSeq) backups.value = list;
  } catch (e) {
    if (seq === backupsSeq) notify(errorText(e), "error");
  }
}

async function createBackup() {
  if (!backupTarget.value || backupBusy.value) return;
  backupBusy.value = true;
  try {
    const info = await api.backupCreate(backupTarget.value.id);
    if (info) {
      notify(`已备份（${humanSize(info.bytes)}）`, "success");
    } else {
      notify("这个版本还没有存档，没有可备份的内容", "info");
    }
    await loadBackups();
  } catch (e) {
    notify(errorText(e), "error");
  } finally {
    backupBusy.value = false;
  }
}

async function restoreBackup(file: string) {
  if (!backupTarget.value || backupBusy.value) return;
  const ok = window.confirm(
    `还原这份备份？\n当前的存档会先自动备份一份，还原错了还能再还原回来。`,
  );
  if (!ok) return;
  backupBusy.value = true;
  try {
    await api.backupRestore(backupTarget.value.id, file);
    await loadBackups();
    notify("存档已还原（还原前的状态也留了一份备份）", "success");
  } catch (e) {
    notify(errorText(e), "error");
  } finally {
    backupBusy.value = false;
  }
}

async function removeBackup(file: string) {
  if (!backupTarget.value || backupBusy.value) return;
  backupBusy.value = true;
  try {
    await api.backupRemove(backupTarget.value.id, file);
    await loadBackups();
  } catch (e) {
    notify(errorText(e), "error");
  } finally {
    backupBusy.value = false;
  }
}

function backupTime(at: number): string {
  return new Date(at).toLocaleString();
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
        <button class="btn-ghost btn-sm" @click="refreshLocal">
          <Icon name="refresh" :size="15" />
          刷新
        </button>
        <button class="btn-outline btn-sm" @click="openInstaller">
          <Icon name="download" :size="15" />
          安装游戏…
        </button>
        <button class="btn-primary btn-sm" @click="openOnline">
          <Icon name="versions" :size="15" />
          版本列表
        </button>
      </div>
    </div>

    <div class="scroll-area min-h-0 flex-1 space-y-4 overflow-y-auto pr-1">
    <!-- 本地版本卡片 -->
    <div v-if="state.local.length > 0" class="grid gap-3 md:grid-cols-2 xl:grid-cols-3">
      <article
        v-for="version in state.local"
        :key="version.id"
        class="card card-hover group flex flex-col gap-2.5 p-3.5 [content-visibility:auto] [contain-intrinsic-size:auto_200px]"
      >
        <div class="flex items-start justify-between gap-2">
          <div class="min-w-0">
            <div class="truncate text-sm font-bold text-ink-900" :title="version.id">
              {{ version.id }}
            </div>
            <div class="mt-1 flex flex-wrap items-center gap-1.5">
              <span
                class="chip whitespace-nowrap"
                :class="
                  version.complete
                    ? 'bg-brand-500/[0.16] text-brand-200'
                    : 'bg-amber-500/[0.14] text-amber-300'
                "
              >
                {{ version.complete ? version.kind : "安装未完成" }}
              </span>
              <span class="chip whitespace-nowrap bg-surface-sunken text-ink-500">
                Java {{ version.javaMajor }}
              </span>
              <span
                v-if="version.modCount > 0"
                class="chip whitespace-nowrap bg-surface-sunken text-ink-500"
              >
                {{ version.modCount }} mod
              </span>
            </div>
          </div>
          <div
            class="grid h-9 w-9 shrink-0 place-items-center rounded-xl bg-brand-500/[0.16] text-brand-200 transition-transform duration-300 ease-spring group-hover:scale-110"
          >
            <Icon name="package" :size="18" />
          </div>
        </div>

        <div class="flex min-w-0 items-center gap-2">
          <span class="min-w-0 flex-1 truncate font-mono text-[10px] text-ink-300" :title="version.dir">
            {{ version.dir }}
          </span>
          <span class="shrink-0 font-mono text-[10px] text-ink-400">
            {{ humanSize(version.size) }}
          </span>
        </div>

        <!-- 卡面只留「启动」一个主按钮，其余低频操作收进 ⋯ 菜单 -->
        <div class="flex gap-2">
          <button
            class="btn-outline flex-1 btn-sm"
            :disabled="!version.complete"
            :title="version.complete ? '' : '安装未完成，请重新安装'"
            @click="startVersion(version.id)"
          >
            <Icon name="play" :size="14" />
            选择并启动
          </button>
          <button
            class="btn-ghost relative !px-2.5 btn-sm"
            :title="version.hasSaves ? '备份 / 还原这个版本的存档' : '这个版本没有存档'"
            @click="openBackups(version)"
          >
            <Icon name="package" :size="14" />
            备份
            <span
              v-if="version.hasSaves"
              class="absolute -right-0.5 -top-0.5 h-2 w-2 rounded-full bg-emerald-400"
              title="有存档"
            />
          </button>
          <div class="relative" data-ver-menu>
            <button
              class="btn-ghost !px-2.5 btn-sm"
              title="更多操作"
              @click="toggleMenu(version.id)"
            >
              <Icon name="settings" :size="15" />
            </button>
            <div
              v-if="menuOpen === version.id"
              class="absolute right-0 top-10 z-30 w-40 origin-top-right animate-pop rounded-xl border border-line bg-surface p-1 shadow-pop"
            >
              <button
                class="w-full rounded-lg px-3 py-2 text-left text-[12px] font-semibold text-ink-700 transition hover:bg-veil/[0.06] hover:text-ink-900"
                title="把这个版本打包成 zip（导出实例）"
                @click="openExport(version)"
              >
                导出实例…
              </button>
              <button
                class="w-full rounded-lg px-3 py-2 text-left text-[12px] font-semibold text-ink-700 transition hover:bg-veil/[0.06] hover:text-ink-900"
                @click="startCopy(version)"
              >
                复制版本…
              </button>
              <button
                class="w-full rounded-lg px-3 py-2 text-left text-[12px] font-semibold text-ink-700 transition hover:bg-veil/[0.06] hover:text-ink-900"
                @click="startRename(version)"
              >
                重命名…
              </button>
              <button
                class="w-full rounded-lg px-3 py-2 text-left text-[12px] font-semibold text-ink-700 transition hover:bg-veil/[0.06] hover:text-ink-900"
                title="校验并补齐缺失/损坏的游戏文件，不碰存档和 mod"
                @click="repairVersion(version)"
              >
                修复文件…
              </button>
              <button
                class="w-full rounded-lg px-3 py-2 text-left text-[12px] font-semibold text-ink-700 transition hover:bg-veil/[0.06] hover:text-ink-900"
                title="管理这个版本已安装的模组"
                @click="menuOpen = ''; state.selectedVersion = version.id; state.view = 'mods'"
              >
                管理模组…
              </button>
              <button
                class="mt-1 w-full rounded-lg border-t border-line px-3 py-2 text-left text-[12px] font-semibold text-rose-400 transition hover:bg-rose-500/[0.12] hover:text-rose-300"
                title="删除这个版本（有存档会先备份）"
                :disabled="deleting === version.id"
                @click="menuOpen = ''; remove(version)"
              >
                删除版本…
              </button>
            </div>
          </div>
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
                <option v-for="version in releaseVersions" :key="version.id" :value="version.id">{{ version.id }}{{ version.id === state.latest.release ? '（最新正式版）' : '' }}</option>
                <option v-for="version in nonReleaseVersions" :key="version.id" :value="version.id">{{ version.id }}（{{ version.kind }}）</option>
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
        <button class="btn-ghost btn-sm" :disabled="loadingRemote" @click="loadRemote">
          <Icon
            name="refresh"
            :size="15"
            :class="loadingRemote ? 'animate-spin' : ''"
          />
          {{ loadingRemote ? "加载中…" : "刷新清单" }}
        </button>
        <button class="btn-ghost btn-sm" @click="showOnline = false">
          <Icon name="close" :size="15" />
          收起
        </button>
      </div>

      <div class="max-h-[420px] divide-y divide-line overflow-y-auto">
        <!-- 清单加载中：骨架行（8 行，模拟真实行高） -->
        <div
          v-if="loadingRemote && filtered.length === 0"
          class="divide-y divide-line"
        >
          <div
            v-for="i in 8"
            :key="`sk-${i}`"
            class="flex items-center gap-3 px-4 py-2.5"
          >
            <div class="skeleton h-5 w-[74px]"></div>
            <div class="skeleton h-4 flex-1"></div>
            <div class="skeleton h-4 w-16"></div>
          </div>
        </div>
        <div
          v-for="version in filtered"
          :key="version.id"
          class="flex items-center gap-3 px-4 py-2.5 transition-colors duration-150 hover:bg-veil/[0.05]"
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

    <!-- 复制/重命名的小输入弹层 -->
    <div
      v-if="idEditor"
      class="fixed inset-0 z-[75] flex items-center justify-center bg-black/65 p-6 backdrop-blur-sm"
      @click.self="!idBusy && (idEditor = null)"
    >
      <div class="w-full max-w-sm rounded-2xl border border-line bg-surface p-5 shadow-pop">
        <div class="text-sm font-extrabold text-ink-900">
          {{ idEditor.mode === "copy" ? "复制版本" : "重命名版本" }}
        </div>
        <p class="mt-1 truncate text-[11px] text-ink-400" :title="idEditor.version.id">
          {{ idEditor.version.id }} · {{ humanSize(idEditor.version.size) }}
        </p>
        <input
          v-model="idEditor.value"
          class="field mt-4 w-full font-mono !text-xs"
          maxlength="60"
          placeholder="新版本名，例如 my-pack-v2"
          @keyup.enter="applyIdEdit"
        />
        <p class="mt-2 text-[11px] leading-relaxed text-ink-400">
          {{
            idEditor.mode === "copy"
              ? "会完整复制 mods、配置和存档；底层游戏文件是共享的，副本实际只多占版本目录本身。"
              : "只改名字，内容原样保留。"
          }}
        </p>
        <div class="mt-4 flex justify-end gap-2">
          <button class="btn-ghost btn-sm" :disabled="idBusy" @click="idEditor = null">
            取消
          </button>
          <button class="btn-primary btn-sm" :disabled="idBusy" @click="applyIdEdit">
            <Icon :name="idBusy ? 'refresh' : 'check'" :size="14" :class="idBusy ? 'animate-spin' : ''" />
            {{ idBusy ? "处理中…" : idEditor.mode === "copy" ? "复制" : "改名" }}
          </button>
        </div>
      </div>
    </div>

    <!-- 存档备份弹层 -->
    <div
      v-if="backupTarget"
      class="fixed inset-0 z-[70] flex items-center justify-center bg-black/65 p-6 backdrop-blur-sm"
      @click.self="!backupBusy && (backupTarget = null)"
    >
      <div class="w-full max-w-md rounded-2xl border border-line bg-surface p-5 shadow-pop">
        <div class="flex items-start justify-between gap-3">
          <div class="min-w-0">
            <div class="text-sm font-extrabold text-ink-900">存档备份</div>
            <p class="mt-1 truncate text-[11px] text-ink-400" :title="backupTarget.id">
              {{ backupTarget.id }}
            </p>
          </div>
          <button
            class="btn-ghost !px-2 !py-1.5"
            title="关闭"
            :disabled="backupBusy"
            @click="backupTarget = null"
          >
            <Icon name="close" :size="15" />
          </button>
        </div>

        <div class="mt-4 max-h-64 space-y-1.5 overflow-y-auto pr-1">
          <div
            v-for="item in backups"
            :key="item.file"
            class="flex items-center gap-3 rounded-xl border border-line bg-veil/[0.02] px-3 py-2"
          >
            <div class="min-w-0 flex-1">
              <div class="truncate text-xs font-semibold text-ink-900">
                {{ backupTime(item.at) }}
              </div>
              <div class="mt-0.5 text-[10px] text-ink-400">{{ humanSize(item.bytes) }}</div>
            </div>
            <button
              class="btn-outline !px-2.5 !py-1.5 !text-[11px]"
              :disabled="backupBusy"
              @click="restoreBackup(item.file)"
            >
              还原
            </button>
            <button
              class="btn-ghost !px-2 !py-1.5 !text-[11px] text-ink-500 hover:bg-rose-500/[0.12] hover:text-rose-300"
              :disabled="backupBusy"
              title="删除这份备份"
              @click="removeBackup(item.file)"
            >
              <Icon name="trash" :size="13" />
            </button>
          </div>
          <div v-if="!backups.length" class="px-1 py-6 text-center text-xs text-ink-400">
            还没有备份。有存档时点下面的按钮留一份。
          </div>
        </div>

        <button class="btn-primary mt-4 w-full justify-center !py-2.5 !text-xs" :disabled="backupBusy" @click="createBackup">
          <Icon :name="backupBusy ? 'refresh' : 'package'" :size="14" :class="backupBusy ? 'animate-spin' : ''" />
          {{ backupBusy ? "处理中…" : "立即备份" }}
        </button>
        <p class="mt-2 text-center text-[10px] text-ink-400">
          自动保留最近 10 份；删除版本前也会自动备一份。
        </p>
      </div>
    </div>

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
          <button class="btn-ghost btn-sm" :disabled="exporting" @click="closeExport">
            取消
          </button>
          <button class="btn-primary btn-sm" :disabled="exporting" @click="exportNow">
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
