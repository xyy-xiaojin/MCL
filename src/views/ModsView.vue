<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";

import Icon from "../components/Icon.vue";
import { api, errorText, humanSize } from "../api";
import { importDropped, notify, state } from "../store";
import type { ModEntry, ModIssue } from "../types";

const mods = ref<ModEntry[]>([]);
const loading = ref(false);
const busy = ref("");
// 请求代际号：onMounted / 两个 watch 都会调 load()，快速切版本时两个请求
// 并发，后到的旧响应不能覆盖新列表（DownloadsView 的 requestSeq 同款防护）
let loadSeq = 0;

// ---- 诊断横幅：列表加载后顺手扫一遍，缺前置/冲突在启动前就亮出来 ----
const issues = ref<ModIssue[]>([]);
const issuesOpen = ref(false);
const errorCount = computed(() => issues.value.filter((i) => i.level === "error").length);
const warningCount = computed(() => issues.value.filter((i) => i.level === "warning").length);
let diagSeq = 0;

async function runDiagnostics() {
  if (!state.selectedVersion) {
    diagSeq++;
    issues.value = [];
    return;
  }
  const seq = ++diagSeq;
  try {
    const found = await api.modDiagnostics(state.selectedVersion);
    if (seq === diagSeq) issues.value = found;
  } catch {
    // 诊断失败不影响列表使用，静默
    if (seq === diagSeq) issues.value = [];
  }
}

async function load() {
  // 版本隔离后 mod 是按版本分的：没选版本就没有"这个版本的 mod 列表"可看
  if (!state.selectedVersion) {
    loadSeq++;
    mods.value = [];
    issues.value = [];
    return;
  }
  const seq = ++loadSeq;
  loading.value = true;
  try {
    const list = await api.listMods(state.selectedVersion);
    if (seq === loadSeq) {
      mods.value = list;
      void runDiagnostics();
    }
  } catch (e) {
    if (seq === loadSeq) {
      notify(errorText(e), "error");
    }
  } finally {
    if (seq === loadSeq) {
      loading.value = false;
    }
  }
}

/**
 * 「添加 mod」按钮：弹系统文件选择框，多选 jar/zip。
 *
 * 选中后走与拖拽完全相同的 `importFiles` 后端分流，行为一致：
 * mod 进当前版本的 mods 目录，误选了整合包 zip 也会新建版本。
 */
async function addMods() {
  if (!state.selectedVersion) {
    notify("请先在首页或「版本管理」选择一个版本", "error");
    return;
  }
  const picked = await api.pickFiles("选择要添加的 mod 文件（可多选）", [
    "jar",
    "zip",
    "litemod",
  ]);
  if (!picked || picked.length === 0) return;
  await importDropped(picked);
}

async function toggle(entry: ModEntry) {
  if (!state.selectedVersion) return;
  busy.value = entry.fileName;
  try {
    const enabled = await api.toggleMod(state.selectedVersion, entry.fileName);
    // 后端会改名，重新拉一次拿到新的文件名
    await load();
    notify(`${entry.name} 已${enabled ? "启用" : "停用"}`, "success");
  } catch (e) {
    notify(errorText(e), "error");
  } finally {
    busy.value = "";
  }
}

onMounted(load);

// 拖拽导入会往 mods/ 里塞文件，列表要跟着刷新
watch(
  () => state.modsVersion,
  () => void load(),
);

// 切换版本时列表必须换掉，否则会显示上一个版本的 mod
watch(
  () => state.selectedVersion,
  () => void load(),
);
</script>

<template>
  <div class="flex h-full min-h-0 flex-col gap-4">
    <div class="flex shrink-0 flex-wrap items-center justify-between gap-3">
      <div class="flex items-center gap-2">
        <span class="chip bg-brand-500/[0.16] text-brand-200">{{ mods.length }} 个模组</span>
        <span class="chip bg-surface-sunken text-ink-500">
          {{ mods.filter((m) => m.enabled).length }} 个已启用
        </span>
      </div>
      <div class="flex items-center gap-2">
        <button class="btn-primary !py-2 !text-xs" @click="addMods">
          <Icon name="upload" :size="15" />
          添加 mod
        </button>
        <button class="btn-ghost !py-2 !text-xs" :disabled="loading" @click="load">
          <Icon name="refresh" :size="15" :class="loading ? 'animate-spin' : ''" />
          刷新
        </button>
      </div>
    </div>

    <div
      class="shrink-0 rounded-xl bg-surface-sunken px-3.5 py-2 font-mono text-[10px] text-ink-400"
    >
      mods 目录：{{ state.paths.mods || "先在上方选择一个版本" }}
    </div>

    <!-- 诊断横幅：有问题才出现，可折叠 -->
    <div
      v-if="issues.length"
      class="shrink-0 rounded-xl border px-4 py-3"
      :class="
        errorCount
          ? 'border-rose-500/25 bg-rose-500/[0.08]'
          : 'border-amber-500/25 bg-amber-500/[0.08]'
      "
    >
      <button
        class="flex w-full items-center gap-2 text-left text-[13px] font-bold"
        :class="errorCount ? 'text-rose-200' : 'text-amber-100'"
        @click="issuesOpen = !issuesOpen"
      >
        <Icon :name="errorCount ? 'close' : 'shield'" :size="15" />
        {{ errorCount ? `${errorCount} 个问题会导致进游戏失败` : `${warningCount} 个潜在冲突` }}
        <span class="ml-auto text-[10px] font-semibold opacity-70">
          {{ issuesOpen ? "收起" : "展开" }}
        </span>
      </button>
      <div v-if="issuesOpen" class="mt-2 space-y-1.5">
        <div
          v-for="(issue, index) in issues"
          :key="index"
          class="flex items-start gap-2 text-[11px] leading-relaxed"
          :class="issue.level === 'error' ? 'text-rose-100/90' : 'text-amber-100/90'"
        >
          <span
            class="mt-1 h-1.5 w-1.5 shrink-0 rounded-full"
            :class="issue.level === 'error' ? 'bg-rose-400' : 'bg-amber-400'"
          />
          {{ issue.message }}
        </div>
      </div>
    </div>

    <div class="min-h-0 flex-1 overflow-y-auto pr-1">
    <div v-if="mods.length > 0" class="grid gap-2.5 md:grid-cols-2">
      <article
        v-for="entry in mods"
        :key="entry.fileName"
        class="card card-hover flex items-center gap-3 p-4"
      >
        <div
          class="grid h-9 w-9 shrink-0 place-items-center rounded-xl transition-colors duration-200"
          :class="
            entry.enabled
              ? 'bg-brand-500/[0.16] text-brand-200'
              : 'bg-surface-sunken text-ink-300'
          "
        >
          <Icon name="mods" :size="17" />
        </div>

        <div class="min-w-0 flex-1">
          <div
            class="truncate text-xs font-bold"
            :class="entry.enabled ? 'text-ink-900' : 'text-ink-400'"
            :title="entry.fileName"
          >
            {{ entry.name }}
          </div>
          <div class="mt-0.5 text-[10px] text-ink-300">{{ humanSize(entry.size) }}</div>
        </div>

        <!-- 开关 -->
        <button
          class="relative h-6 w-11 shrink-0 rounded-pill transition-colors duration-200"
          :class="entry.enabled ? 'bg-brand-600' : 'bg-ink-300/40'"
          :disabled="busy === entry.fileName"
          :title="entry.enabled ? '点击停用' : '点击启用'"
          @click="toggle(entry)"
        >
          <span
            class="absolute top-0.5 h-5 w-5 rounded-full bg-surface shadow-sm transition-all duration-200 ease-smooth"
            :class="entry.enabled ? 'left-[22px]' : 'left-0.5'"
          />
        </button>
      </article>
    </div>

    <div v-else class="card flex flex-col items-center gap-3 p-12 text-center">
      <div class="grid h-14 w-14 place-items-center rounded-2xl bg-brand-500/[0.14] text-brand-300">
        <Icon name="mods" :size="26" />
      </div>
      <div class="text-sm font-bold text-ink-900">mods 目录是空的</div>
      <p class="max-w-md text-xs leading-relaxed text-ink-400">
        导入整合包会自动把模组下载到对应实例；也可以手动把 .jar 放进上面的目录，
        点「刷新」就能看到。启用/停用只是改扩展名，不会删除文件。
      </p>
      <button class="btn-outline" @click="state.view = 'modpacks'">
        去整合包页
        <Icon name="modpack" :size="16" />
      </button>
    </div>
    </div>
  </div>
</template>
