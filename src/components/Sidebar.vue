<script setup lang="ts">
/**
 * 左侧主导航。
 *
 * 只放"入口"，不再放二级分类：下载的 6 个分类已经收进下载页的页内 tab，
 * 侧栏保持 5 项，右侧内容区拿到的宽度也更多。
 */
import Icon from "./Icon.vue";
import SkinAvatar from "./SkinAvatar.vue";
import { state } from "../store";
import type { ViewKey } from "../types";

interface NavItem {
  key: ViewKey;
  label: string;
  icon: string;
}

const top: NavItem[] = [
  { key: "home", label: "首页", icon: "home" },
  { key: "downloads", label: "下载", icon: "download" },
  { key: "versions", label: "版本管理", icon: "versions" },
  { key: "netplay", label: "联机", icon: "netplay" },
  { key: "settings", label: "设置", icon: "settings" },
];

/** 版本号来自 package.json（vite define 注入），只维护一处 */
const version = __APP_VERSION__;

/** 下载任务的角标：挂在「下载」这一项上 */
const activeCount = () =>
  state.queue.filter((q) => q.state === "running" || q.state === "pending").length;
</script>

<template>
  <aside class="flex w-56 shrink-0 flex-col border-r border-line bg-surface-muted">
    <!-- 品牌区：玩家皮肤头像 + 昵称（PCL 风格） -->
    <div class="flex items-center gap-3 px-5 pb-4 pt-6">
      <SkinAvatar :size="40" :model="state.avatar.model" :src="state.avatar.image ?? ''" />
      <div class="min-w-0 flex-1">
        <div class="truncate text-[14px] font-extrabold tracking-tight text-ink-900" :title="state.settings.username">
          {{ state.settings.username || "Player" }}
        </div>
        <div class="mt-0.5 text-[10px] font-semibold tracking-wide text-ink-400">
          离线模式 · MCL
        </div>
      </div>
    </div>

    <nav class="scroll-area min-h-0 flex-1 overflow-y-auto px-3 pb-3">
      <button
        v-for="item in top"
        :key="item.key"
        class="group relative mb-1 flex w-full items-center gap-3 rounded-xl px-3 py-2.5 text-left text-sm font-semibold transition-[background-color,color,box-shadow,transform] duration-200 ease-spring hover:translate-x-0.5 active:scale-[0.985]"
        :class="
          state.view === item.key
            ? 'bg-brand-500/[0.16] text-brand-200'
            : 'text-ink-500 hover:bg-veil/[0.05] hover:text-ink-900'
        "
        @click="state.view = item.key"
      >
        <span
          class="absolute left-0 top-1/2 h-5 w-[3px] -translate-y-1/2 rounded-pill bg-brand-600 transition-[transform,opacity] duration-200"
          :class="state.view === item.key ? 'opacity-100' : 'opacity-0'"
        />
        <Icon
          :name="item.icon"
          :size="19"
          :class="state.view === item.key ? 'text-brand-300' : 'text-ink-400 group-hover:text-ink-700'"
        />
        <span class="flex-1">{{ item.label }}</span>
        <span
          v-if="item.key === 'downloads' && activeCount() > 0"
          class="chip !px-1.5 !py-0 bg-brand-600 text-white"
          :title="`${activeCount()} 个任务进行中`"
        >
          {{ activeCount() }}
        </span>
      </button>
    </nav>

    <!-- 底部状态：Java + 版本号，路径太长的不占地方，鼠标悬停能看 -->
    <div class="border-t border-line px-3 py-3" :title="state.paths.root">
      <div class="flex items-center justify-between gap-2 text-[11px] font-medium rounded-xl bg-surface-sunken px-3 py-2 text-ink-500">
        <div class="flex items-center gap-2">
          <span
            class="h-1.5 w-1.5 rounded-full"
            :class="state.java ? 'bg-emerald-500' : 'bg-amber-400'"
          />
          <span v-if="state.java">Java {{ state.java.major }}</span>
          <span v-else>未检测到 Java</span>
        </div>
        <span class="font-mono text-[10px] text-ink-300" title="MCL 版本号">v{{ version }}</span>
      </div>
    </div>
  </aside>
</template>
