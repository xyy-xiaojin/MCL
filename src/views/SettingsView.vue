<script setup lang="ts">
import { computed, ref } from "vue";

import Icon from "../components/Icon.vue";
import { api, errorText } from "../api";
import { notify, refreshJava, saveSettings, state } from "../store";
import type { SourceProbe } from "../types";

const version = __APP_VERSION__;

// 一键诊断：只读取 Java/网络状态，并对 MCL 数据目录做一次可删除的写入测试。
const diagnosticOpen = ref(false);
const diagnosing = ref(false);
const diagnostic = ref<{
  java: { ok: boolean; message: string } | null;
  directory: { ok: boolean; path: string; message: string } | null;
  network: { ok: boolean; message: string } | null;
}>({ java: null, directory: null, network: null });

async function runDiagnostics() {
  if (diagnosing.value) return;
  diagnosticOpen.value = true;
  diagnosing.value = true;
  diagnostic.value = { java: null, directory: null, network: null };
  const javaTask = api.detectJava(true).then((java) => {
    diagnostic.value.java = java
      ? { ok: true, message: `已找到 Java ${java.major}（${java.source}）` }
      : { ok: false, message: "未检测到 Java；可在 Java 路径设置中重新检测或安装运行时" };
  }).catch((e) => {
    diagnostic.value.java = { ok: false, message: errorText(e) };
  });
  const directoryTask = api.checkDataDirectory().then((result) => {
    diagnostic.value.directory = result;
  }).catch((e) => {
    diagnostic.value.directory = { ok: false, path: state.paths.root, message: errorText(e) };
  });
  const networkTask = api.testSources().then((probes) => {
    const reachable = probes.filter((probe) => probe.ms !== null && probe.bytes > 0);
    diagnostic.value.network = reachable.length
      ? { ok: true, message: `至少有 ${reachable.length} 个下载源可连接` }
      : { ok: false, message: "没有可用下载源；请检查网络、代理或稍后重试" };
  }).catch((e) => {
    diagnostic.value.network = { ok: false, message: errorText(e) };
  });
  try {
    await Promise.all([javaTask, directoryTask, networkTask]);
  } finally {
    diagnosing.value = false;
  }
}

// ---- 下载源测速 ----

const probes = ref<SourceProbe[]>([]);
const probing = ref(false);

// "最快的"要同时看延迟和实际下到的字节数：只判 ms 会选中
// "200ms 但一行数据都没下回来"的源。
const bestSource = computed(
  () => probes.value.find((p) => p.ms !== null && p.bytes > 0)?.source ?? "",
);

async function runProbe() {
  if (probing.value) return;
  probing.value = true;
  try {
    probes.value = await api.testSources();
    if (probes.value.length === 0) notify("测速没有返回结果", "error");
  } catch (e) {
    notify(errorText(e), "error");
  } finally {
    probing.value = false;
  }
}

function useFastest() {
  if (!bestSource.value) return;
  state.settings.source = bestSource.value;
  notify(`已切到${bestSource.value === "official" ? "官方源" : "BMCLAPI"}，保存后生效`, "info");
}

const autoRam = computed(() => state.settings.ramType !== "manual");

/** 徽标显示实际会用的值：自动模式下是后端推荐值 */
const memoryGb = computed(() =>
  ((state.memory?.effectiveMaxMb ?? state.settings.maxMemoryMb) / 1024).toFixed(1),
);

async function save() {
  await saveSettings();
  // 用户可能刚装了 Java：这里无视缓存重新探测一次
  await refreshJava(true);
}

function copyPath(value: string) {
  navigator.clipboard
    ?.writeText(value)
    .then(() => notify("路径已复制", "success"))
    .catch(() => notify("复制失败", "error"));
}

async function revealPath(value: string) {
  try {
    await api.revealInExplorer(value);
  } catch (e) {
    notify(errorText(e), "error");
  }
}
</script>

<template>
  <!--
    三张设置卡铺满剩余高度：上面一行「下载源 / 内存与账户」，Java 路径横跨
    两列压在下面。页面本身不滚动，每张卡的正文区在内容超出时自己滚；
    底部「保存」与「关于」常驻。
  -->
  <div class="flex h-full min-h-0 flex-col gap-3">
    <div
      class="grid min-h-0 flex-1 auto-rows-min content-start gap-3 overflow-y-auto lg:grid-cols-2"
    >
      <!-- 下载源 -->
      <section class="card flex min-h-0 flex-col p-4">
        <div class="mb-3 flex shrink-0 items-center gap-2 text-sm font-bold text-ink-900">
          <Icon name="download" :size="18" class="text-brand-300" />
          下载源
        </div>

        <div class="min-h-0 flex-1 space-y-2.5 overflow-y-auto pr-1">
          <select v-model="state.settings.source" class="field cursor-pointer">
            <option value="bmclapi">BMCLAPI 镜像（国内推荐）</option>
            <option value="official">Mojang 官方源</option>
          </select>

          <div class="flex items-center gap-2">
            <button class="btn-outline !py-1.5 !text-[11px]" :disabled="probing" @click="runProbe">
              <Icon name="refresh" :size="12" :class="probing ? 'animate-spin' : ''" />
              {{ probing ? "测速中…" : "测速" }}
            </button>
            <button v-if="bestSource" class="btn-ghost !py-1.5 !text-[11px]" @click="useFastest">
              用最快的（{{ bestSource === "official" ? "官方" : "BMCLAPI" }}）
            </button>
          </div>

          <div v-if="probes.length > 0" class="space-y-1">
            <div
              v-for="probe in probes"
              :key="probe.source"
              class="flex items-center gap-2 text-[11px]"
            >
              <span
                class="h-1.5 w-1.5 shrink-0 rounded-full"
                :class="probe.ms === null ? 'bg-rose-400' : 'bg-emerald-400'"
              />
              <span class="w-[86px] shrink-0 font-semibold text-ink-500">
                {{ probe.source === "official" ? "Mojang 官方" : "BMCLAPI" }}
              </span>
              <span v-if="probe.ms !== null" class="font-mono text-ink-900">
                {{ probe.ms }} ms
              </span>
              <span v-else class="truncate text-rose-300">{{ probe.error }}</span>
            </div>
          </div>

          <!-- 下载并发：一个滑块 + 一句话，改完从下一批任务起生效 -->
          <div class="pt-1">
            <div class="flex items-baseline justify-between">
              <span class="text-xs font-semibold text-ink-500">同时下载文件数</span>
              <span class="font-mono text-xs font-bold text-brand-200">
                {{ state.settings.downloadConcurrency ?? 10 }}
              </span>
            </div>
            <input
              v-model.number="state.settings.downloadConcurrency"
              type="range"
              min="1"
              max="32"
              step="1"
              class="mt-2 w-full accent-brand-600"
            />
            <div class="mt-1 flex justify-between text-[10px] font-medium text-ink-300">
              <span>1 慢而稳</span>
              <span>默认 10</span>
              <span>32 激进</span>
            </div>
            <p class="mt-1.5 text-[10px] leading-relaxed text-ink-400">
              调高更快；被镜像限流（频繁失败）就调小。大文件另有 16 线程分片。
            </p>
          </div>

          <p class="text-[10px] leading-relaxed text-ink-400">
            镜像会重写元数据/依赖/资源地址到 bmclapi2.bangbang93.com；改动对新入队的下载生效。
          </p>
        </div>
      </section>

      <!-- 性能与账户：内存分配只在首页（启动前最常调），这里去重 -->
      <section class="card flex min-h-0 flex-col p-4">
        <div class="mb-3 flex shrink-0 items-center gap-2 text-sm font-bold text-ink-900">
          <Icon name="cpu" :size="18" class="text-brand-300" />
          性能与账户
        </div>

        <div class="min-h-0 flex-1 space-y-3.5 overflow-y-auto pr-1">
          <p class="rounded-xl bg-surface-sunken px-3 py-2 text-[11px] leading-relaxed text-ink-400">
            内存分配在首页「内存分配」卡里调整（当前
            <span class="font-bold text-brand-200">{{ memoryGb }} GB</span>，{{
              autoRam ? "自动推荐" : "手动指定"
            }}）。
          </p>

          <div>
            <span class="label">GC 优化</span>
            <select v-model="state.settings.gcMode" class="field">
              <option value="auto">自动（Java 21+ ZGC，更低版本 G1）</option>
              <option value="g1">G1GC（Mojang 默认参数）</option>
              <option value="g1-optimized">优化 G1GC（更少卡顿）</option>
              <option value="custom">不指定（自己写 JVM 参数）</option>
            </select>
            <p class="mt-1.5 font-mono text-[10px] leading-relaxed text-ink-400">
              {{ (state.memory?.gcArgs.length ? state.memory?.gcArgs.join(" ") : "不追加任何 GC 参数") }}
            </p>
          </div>

          <div>
            <span class="label">离线昵称</span>
            <input v-model="state.settings.username" class="field" maxlength="16" />
            <p class="mt-1.5 text-[10px] text-ink-400">
              仅离线模式生效；用皮肤站登录时以皮肤站的角色名为准。
            </p>
          </div>
        </div>
      </section>

      <!-- Java：横跨两列，输入框和检测结果并排，不留空 -->
      <section class="card flex min-h-0 flex-col p-4 lg:col-span-2">
        <div class="mb-3 flex shrink-0 items-center justify-between">
          <div class="flex items-center gap-2 text-sm font-bold text-ink-900">
            <Icon name="cpu" :size="18" class="text-brand-300" />
            Java 路径
          </div>
          <button
            class="text-[11px] font-semibold text-brand-300 hover:text-brand-200"
            @click="refreshJava(true)"
          >
            重新检测
          </button>
        </div>

        <div class="flex min-w-0 flex-col gap-2 lg:flex-row lg:items-center lg:gap-4">
          <input
            v-model="state.settings.javaPath"
            class="field min-w-0 font-mono text-xs lg:flex-1"
            placeholder="留空自动探测"
          />
          <div class="min-w-0 text-[11px] text-ink-400 lg:max-w-[45%] lg:shrink-0">
            <template v-if="state.java">
              当前：
              <span class="font-semibold text-emerald-400">Java {{ state.java.major }}</span>
              <span class="text-ink-400">（{{ state.java.source }}）</span>
              <div class="mt-0.5 truncate font-mono text-[10px] text-ink-300">
                {{ state.java.path }}
              </div>
            </template>
            <span v-else class="font-semibold text-amber-400">未检测到 Java</span>
          </div>
        </div>
      </section>

      <!-- 关于与数据：版本号、数据目录、诊断入口都收进卡片，不再单独占一条底栏 -->
      <section class="card flex flex-wrap items-center justify-between gap-3 p-4 lg:col-span-2">
        <div class="min-w-0">
          <div class="flex items-center gap-2 text-sm font-bold text-ink-900">
            <Icon name="shield" :size="18" class="text-brand-300" />
            关于与数据
          </div>
          <p class="mt-1 text-[11px] leading-relaxed text-ink-400">
            MCL <span class="font-bold text-ink-900">{{ version }}</span>
            <span class="mx-1.5 text-ink-300">·</span>Tauri 2 + Vue 3
          </p>
          <p class="mt-0.5 truncate font-mono text-[10px] text-ink-400" :title="state.paths.root">
            {{ state.paths.root }}
          </p>
        </div>
        <div class="flex shrink-0 flex-wrap items-center gap-2">
          <button class="btn-ghost btn-sm" @click="copyPath(state.paths.root)">
            <Icon name="copy" :size="13" />
            复制数据目录
          </button>
          <button class="btn-ghost btn-sm" @click="revealPath(state.paths.root)">
            <Icon name="folder" :size="13" />
            打开
          </button>
          <button class="btn-outline btn-sm" :disabled="diagnosing" @click="runDiagnostics">
            <Icon name="shield" :size="13" :class="diagnosing ? 'animate-pulse' : ''" />
            {{ diagnosing ? "检测中…" : "一键环境检测" }}
          </button>
        </div>
      </section>
    </div>

    <!-- 保存：只剩"保存"一件事，检测和目录都收进上面的「关于与数据」卡 -->
    <div
      class="flex shrink-0 items-center justify-between rounded-card border border-line bg-surface px-5 py-2.5"
    >
      <p class="truncate text-[11px] text-ink-400">
        设置保存在 <span class="font-mono text-ink-500">settings.json</span>，改完点保存生效
      </p>
      <button class="btn-primary btn-sm shrink-0" @click="save">
        <Icon name="check" :size="15" />
        保存设置
      </button>
    </div>

  </div>

  <div v-if="diagnosticOpen" class="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4" @click.self="diagnosticOpen = false">
    <section class="card w-full max-w-lg p-5 shadow-2xl">
      <div class="mb-4 flex items-center justify-between">
        <div>
          <h2 class="text-base font-bold text-ink-900">MCL 环境检测</h2>
          <p class="mt-1 text-xs text-ink-400">检测 Java、数据目录写入权限和下载源连接。</p>
        </div>
        <button class="btn-ghost !px-2" aria-label="关闭" @click="diagnosticOpen = false">
          <Icon name="close" :size="18" />
        </button>
      </div>
      <div class="space-y-2">
        <div v-for="item in [
          { key: 'java', label: 'Java 运行环境', value: diagnostic.java },
          { key: 'directory', label: '数据目录权限', value: diagnostic.directory },
          { key: 'network', label: '下载源连接', value: diagnostic.network },
        ]" :key="item.key" class="rounded-xl border border-line bg-surface-sunken/50 p-3">
          <div class="flex items-start gap-2.5">
            <span class="mt-0.5 flex h-5 w-5 shrink-0 items-center justify-center rounded-full" :class="!item.value ? 'bg-surface text-ink-400' : item.value.ok ? 'bg-emerald-500/15 text-emerald-400' : 'bg-rose-500/15 text-rose-400'">
              <Icon v-if="!item.value && diagnosing" name="refresh" :size="12" class="animate-spin" />
              <Icon v-else-if="!item.value" name="refresh" :size="12" />
              <Icon v-else-if="item.value.ok" name="check" :size="12" />
              <Icon v-else name="close" :size="12" />
            </span>
            <div class="min-w-0 flex-1">
              <div class="text-xs font-bold text-ink-900">{{ item.label }}</div>
              <div class="mt-1 break-words text-[11px] leading-relaxed text-ink-400">
                {{ item.value?.message || (diagnosing ? '正在检测…' : '尚未检测') }}
              </div>
              <div v-if="item.key === 'directory' && diagnostic.directory?.path" class="mt-1 break-all font-mono text-[10px] text-ink-300">{{ diagnostic.directory.path }}</div>
            </div>
          </div>
        </div>
      </div>
      <div class="mt-4 flex justify-end gap-2">
        <button class="btn-outline" :disabled="diagnosing" @click="runDiagnostics">重新检测</button>
        <button class="btn-primary" @click="diagnosticOpen = false">完成</button>
      </div>
    </section>
  </div>
</template>
