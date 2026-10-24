<script setup lang="ts">
import { computed, onMounted } from "vue";

import { LOADER_LABEL } from "./labels";

import Icon from "./components/Icon.vue";
import Sidebar from "./components/Sidebar.vue";
import { bootstrap, dismissCrash, openQueue, revealCrashLog, state } from "./store";
import type { ViewKey } from "./types";
import DownloadsView from "./views/DownloadsView.vue";
import HomeView from "./views/HomeView.vue";
import ModpacksView from "./views/ModpacksView.vue";
import NetplayView from "./views/NetplayView.vue";
import ModsView from "./views/ModsView.vue";
import SettingsView from "./views/SettingsView.vue";
import VersionsView from "./views/VersionsView.vue";

const meta: Record<ViewKey, { title: string; sub: string }> = {
  home: { title: "首页", sub: "选择版本并启动游戏" },
  versions: { title: "版本管理", sub: "本地实例与在线安装" },
  netplay: { title: "联机", sub: "选一条能走通的通路，把世界分享给朋友" },
  downloads: { title: "下载", sub: "原版游戏与社区资源，按分类浏览" },
  modpacks: { title: "整合包", sub: "在线安装 Modrinth 整合包，本地包直接拖进窗口" },
  mods: { title: "模组管理", sub: "启用 / 停用已安装的模组" },
  settings: { title: "设置", sub: "下载源、Java、内存与账户" },
};

const views = {
  home: HomeView,
  versions: VersionsView,
  netplay: NetplayView,
  downloads: DownloadsView,
  modpacks: ModpacksView,
  mods: ModsView,
  settings: SettingsView,
};

const current = computed(() => views[state.view]);

const activeCount = computed(
  () =>
    state.queue.filter((q) => q.state === "running" || q.state === "pending")
      .length,
);

onMounted(bootstrap);
</script>

<template>
  <div class="flex h-full w-full overflow-hidden">
    <Sidebar />

    <div class="flex min-w-0 flex-1 flex-col">
      <!-- 顶栏 -->
      <header
        class="flex h-16 shrink-0 items-center justify-between border-b border-line bg-surface-muted/60 px-7 backdrop-blur-xl"
      >
        <div>
          <div class="flex items-center gap-2">
            <h1 class="text-[17px] font-extrabold tracking-tight text-ink-900">
              {{ meta[state.view].title }}
            </h1>
            <span
              v-if="state.view === 'downloads' && activeCount"
              class="chip-brand"
            >
              {{ activeCount }} 个任务
            </span>
          </div>
          <p class="text-xs text-ink-400">{{ meta[state.view].sub }}</p>
        </div>

        <div class="flex items-center gap-3">
          <button
            class="btn-ghost !py-2 !text-xs"
            title="打开下载队列"
            @click="openQueue()"
          >
            <Icon name="download" :size="16" />
            {{ activeCount ? `${activeCount} 个进行中` : "队列空闲" }}
          </button>
          <div
            class="flex items-center gap-2 rounded-pill border border-line bg-white/[0.04] px-3 py-1.5 text-xs font-semibold text-ink-500"
          >
            <span class="h-1.5 w-1.5 rounded-full bg-emerald-400 shadow-[0_0_8px_rgba(52,211,153,0.8)]" />
            {{ state.settings.source === "official" ? "官方源" : "BMCLAPI" }}
          </div>
        </div>
      </header>

      <!--
        主区域：外层不滚动，滚动交给各页自己管。
        页面根节点拿 `h-full min-h-0`，内部再把"固定部分"和"可滚动列表"分开，
        这样切页时外壳不跳、也不出现整页滚动条。
      -->
      <main class="min-h-0 flex-1 overflow-hidden px-6 py-5">
        <component :is="current" :key="state.view" class="view-enter h-full min-h-0" />
      </main>
    </div>

    <!-- 拖拽导入提示层：盖住整个窗口，"松手就会导入"一眼可见 -->
    <Transition
      enter-active-class="transition duration-150"
      enter-from-class="opacity-0"
      leave-active-class="transition duration-150"
      leave-to-class="opacity-0"
    >
      <div
        v-if="state.dragging"
        class="pointer-events-none fixed inset-0 z-[60] flex items-center justify-center bg-black/70 backdrop-blur-sm"
      >
        <div
          class="flex flex-col items-center gap-4 rounded-3xl border-2 border-dashed border-brand-400/70 bg-surface/90 px-16 py-12 shadow-pop"
        >
          <div class="grid h-16 w-16 place-items-center rounded-2xl bg-brand-500/[0.18]">
            <Icon name="package" :size="30" class="text-brand-300" />
          </div>
          <div class="text-center">
            <p class="text-lg font-extrabold text-white">松开即可导入</p>
            <p class="mt-1.5 text-xs leading-relaxed text-ink-400">
              <template v-if="state.dragCount > 1">{{ state.dragCount }} 个文件 · </template>
              模组→mods · 材质包→resourcepacks · 光影包→shaderpacks · 压缩包→整合包
            </p>
          </div>
        </div>
      </div>
    </Transition>

    <!-- 崩溃诊断卡：非零退出时弹出。给小白的是"该做什么"，不是堆栈 -->
    <Transition
      enter-active-class="transition duration-200 ease-smooth"
      enter-from-class="opacity-0"
      leave-active-class="transition duration-150"
      leave-to-class="opacity-0"
    >
      <div
        v-if="state.crash"
        class="fixed inset-0 z-[70] flex items-center justify-center bg-black/70 p-6 backdrop-blur-sm"
        @click.self="dismissCrash"
      >
        <div class="w-full max-w-lg rounded-3xl border border-line bg-surface p-6 shadow-pop">
          <div class="flex items-start gap-4">
            <div class="grid h-12 w-12 shrink-0 place-items-center rounded-2xl bg-rose-500/[0.15] text-rose-400">
              <Icon name="flame" :size="24" />
            </div>
            <div class="min-w-0 flex-1">
              <h3 class="text-base font-extrabold text-ink-900">
                游戏「{{ state.crash.version }}」异常退出（退出码 {{ state.crash.code }}）
              </h3>
              <p class="mt-1 text-xs text-ink-400">
                下面是初步判断，按提示处理通常就能解决。
              </p>
            </div>
          </div>

          <ul class="mt-4 space-y-2">
            <li
              v-for="(tip, i) in state.crash.tips ?? []"
              :key="i"
              class="rounded-xl bg-white/[0.04] px-4 py-3 text-[13px] leading-relaxed text-ink-900"
            >
              {{ tip }}
            </li>
          </ul>

          <p
            v-if="state.crash.logPath"
            class="mt-3 truncate rounded-xl bg-surface-sunken px-4 py-2.5 font-mono text-[10px] text-ink-400"
            :title="state.crash.logPath"
          >
            {{ state.crash.logPath }}
          </p>

          <div class="mt-5 flex items-center justify-end gap-2">
            <button
              v-if="state.crash.logPath"
              class="btn-outline"
              @click="revealCrashLog"
            >
              <Icon name="folder" :size="15" />
              打开日志
            </button>
            <button class="btn-primary" @click="dismissCrash">我知道了</button>
          </div>
        </div>
      </div>
    </Transition>

    <!-- 加载器安装进度：全局面板，切页不丢。日志只留最后几行，别让它变成控制台 -->
    <Transition
      enter-active-class="transition duration-200 ease-smooth"
      enter-from-class="translate-y-3 opacity-0"
      leave-active-class="transition duration-200"
      leave-to-class="translate-y-2 opacity-0"
    >
      <div
        v-if="state.installer.running"
        class="fixed bottom-6 left-6 z-40 w-[26rem] overflow-hidden rounded-2xl border border-line bg-surface/95 shadow-pop backdrop-blur-xl"
      >
        <div class="flex items-center gap-2.5 border-b border-line px-4 py-3">
          <Icon name="refresh" :size="16" class="animate-spin text-brand-300" />
          <span class="text-xs font-extrabold text-ink-900">
            正在安装 {{ LOADER_LABEL[state.installer.loader] ?? state.installer.loader }}
          </span>
          <span class="text-[10px] text-ink-400">要跑几分钟，属正常，别关窗口</span>
        </div>
        <div class="flex h-28 flex-col justify-end overflow-hidden bg-black/30 px-4 py-2">
          <p
            v-for="(line, i) in state.installer.lines.slice(-6)"
            :key="i"
            class="truncate font-mono text-[10px] leading-relaxed text-ink-400"
            :class="i === Math.min(5, state.installer.lines.length - 1) ? 'text-ink-900' : ''"
          >
            {{ line }}
          </p>
        </div>
      </div>
    </Transition>

    <!-- 轻提示 -->
    <Transition
      enter-active-class="transition duration-300 ease-smooth"
      enter-from-class="translate-y-3 opacity-0"
      leave-active-class="transition duration-200"
      leave-to-class="translate-y-2 opacity-0"
    >
      <div
        v-if="state.toast.text"
        class="fixed bottom-6 right-6 z-50 max-w-md rounded-2xl px-4 py-3 text-sm font-medium shadow-pop"
        :class="{
          'bg-black/50 text-white': state.toast.tone === 'info',
          'bg-emerald-600 text-white': state.toast.tone === 'success',
          'bg-rose-600 text-white': state.toast.tone === 'error',
        }"
      >
        {{ state.toast.text }}
      </div>
    </Transition>
  </div>
</template>
