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

const recommendLabel = computed(
  () => `${((state.memory?.plan.recommendMb ?? 0) / 1024).toFixed(1)} GB`,
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
    四张设置卡铺满剩余高度（2×2），页面本身不滚动：每张卡的标题固定，
    正文区在内容超出时自己滚。底部「保存」与「关于」常驻。
  -->
  <div class="flex h-full min-h-0 flex-col gap-3">
    <div
      class="grid min-h-0 flex-1 auto-rows-fr gap-3 overflow-hidden lg:grid-cols-2 lg:grid-rows-[1fr_1.45fr]"
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

          <p class="text-[11px] leading-relaxed text-ink-400">
            选择镜像时，元数据、依赖库、资源地址会被重写到 bmclapi2.bangbang93.com；
            官方源直连 Mojang。修改后对新入队的下载生效。
          </p>
        </div>
      </section>

      <!-- 主题 -->
      <section class="card flex min-h-0 flex-col p-4">
        <div class="mb-3 flex shrink-0 items-center gap-2 text-sm font-bold text-ink-900">
          <Icon name="bolt" :size="18" class="text-brand-300" />
          主题
        </div>

        <div class="min-h-0 flex-1 overflow-y-auto pr-1">
          <div class="flex gap-2">
            <button
              class="flex-1 rounded-xl border-2 px-3 py-3 text-left transition-all duration-200"
              :class="
                state.settings.theme === 'dark'
                  ? 'border-brand-400/70 bg-brand-500/[0.14]'
                  : 'border-line hover:border-brand-300/60'
              "
              @click="state.settings.theme = 'dark'"
            >
              <div class="flex items-center gap-2 text-xs font-bold text-ink-900">
                <span
                  class="h-3 w-3 rounded-full bg-gradient-to-br from-brand-400 to-brand-600 shadow-[0_0_10px_rgba(139,92,246,0.8)]"
                />
                深色（默认）
              </div>
              <div class="mt-1 text-[10px] text-ink-400">冷黑底 + 电光紫强调</div>
            </button>
            <div
              class="flex-1 cursor-not-allowed rounded-xl border-2 border-dashed border-line px-3 py-3 opacity-60"
            >
              <div class="text-xs font-bold text-ink-400">浅色</div>
              <div class="mt-1 text-[10px] text-ink-300">暂未实现</div>
            </div>
          </div>
        </div>
      </section>

      <!-- 内存与账户 -->
      <section class="card flex min-h-0 flex-col p-4">
        <div class="mb-3 flex shrink-0 items-center gap-2 text-sm font-bold text-ink-900">
          <Icon name="cpu" :size="18" class="text-brand-300" />
          内存与账户
        </div>

        <div class="min-h-0 flex-1 space-y-3.5 overflow-y-auto pr-1">
          <div>
            <div class="mb-1 flex items-center justify-between">
              <span class="label !mb-0">内存分配</span>
              <span class="chip bg-brand-500/[0.16] text-brand-200">{{ memoryGb }} GB</span>
            </div>
            <div class="flex gap-2">
              <button
                class="flex-1 rounded-xl px-3 py-2 text-xs font-bold transition-colors duration-200"
                :class="
                  autoRam
                    ? 'bg-brand-600 text-white'
                    : 'bg-surface-sunken text-ink-500 hover:text-brand-200'
                "
                @click="state.settings.ramType = 'auto'"
              >
                自动推荐
              </button>
              <button
                class="flex-1 rounded-xl px-3 py-2 text-xs font-bold transition-colors duration-200"
                :class="
                  !autoRam
                    ? 'bg-brand-600 text-white'
                    : 'bg-surface-sunken text-ink-500 hover:text-brand-200'
                "
                @click="state.settings.ramType = 'manual'"
              >
                手动指定
              </button>
            </div>

            <input
              v-if="!autoRam"
              v-model.number="state.settings.maxMemoryMb"
              type="range"
              min="1024"
              max="16384"
              step="512"
              class="mt-3 w-full accent-brand-600"
            />
            <p class="mt-2 text-[11px] leading-relaxed text-ink-400">
              <template v-if="autoRam">
                按 mod 数量与当前可用内存计算，当前建议
                <span class="font-bold text-brand-200">{{ recommendLabel }}</span>
                （{{ state.memory?.plan.reason || "原版" }}）。
              </template>
              <template v-else>
                始终使用上面的值。大整合包（300+ mod）通常需要 6–8 GB。
              </template>
            </p>
          </div>

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

      <!-- Java -->
      <section class="card flex min-h-0 flex-col p-4">
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

        <div class="min-h-0 flex-1 overflow-y-auto pr-1">
          <input
            v-model="state.settings.javaPath"
            class="field font-mono text-xs"
            placeholder="留空自动探测"
          />
          <div class="mt-2.5 text-[11px] text-ink-400">
            <template v-if="state.java">
              当前：
              <span class="font-semibold text-emerald-400">Java {{ state.java.major }}</span>
              <span class="text-ink-300">（{{ state.java.source }}）</span>
              <div class="mt-0.5 truncate font-mono text-[10px] text-ink-300">
                {{ state.java.path }}
              </div>
            </template>
            <span v-else class="font-semibold text-amber-400">未检测到 Java</span>
          </div>
        </div>
      </section>
    </div>

    <!-- 保存 -->
    <div
      class="flex shrink-0 items-center justify-between rounded-card border border-line bg-surface/60 px-5 py-3"
    >
      <p class="text-[11px] text-ink-400">
        设置保存在 <span class="font-mono text-ink-500">{{ state.paths.root }}\settings.json</span>
      </p>
      <div class="flex items-center gap-2">
        <button class="btn-outline" :disabled="diagnosing" @click="runDiagnostics">
          <Icon name="shield" :size="16" :class="diagnosing ? 'animate-pulse' : ''" />
          {{ diagnosing ? "检测中…" : "一键环境检测" }}
        </button>
        <button class="btn-primary" @click="save">
          <Icon name="check" :size="16" />
          保存设置
        </button>
      </div>
    </div>

    <!-- 关于：版本号 + 数据目录，反馈问题时方便报出来 -->
    <section class="card flex shrink-0 flex-wrap items-center justify-between gap-3 px-5 py-2.5">
      <div class="min-w-0 text-[11px] leading-relaxed text-ink-400">
        <span class="font-bold text-ink-900">MCL {{ version }}</span>
        <span class="mx-1.5 text-ink-300">·</span>
        Tauri 2 + Vue 3
        <span class="mx-1.5 text-ink-300">·</span>
        <span class="font-mono text-[10px] text-ink-300" :title="state.paths.root">
          {{ state.paths.root }}
        </span>
      </div>
      <div class="flex shrink-0 items-center gap-2">
        <button
          class="text-[10px] font-semibold text-brand-300 hover:text-brand-200"
          @click="copyPath(state.paths.root)"
        >
          复制数据目录
        </button>
        <button
          class="text-[10px] font-semibold text-brand-300 hover:text-brand-200"
          @click="revealPath(state.paths.root)"
        >
          打开
        </button>
      </div>
    </section>
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
