<script setup lang="ts">
/**
 * 左侧主导航。
 *
 * 下载的分类（原版游戏 / Mod / 整合包 / 数据包 / 材质包 / 光影包）直接挂在这里，
 * 点一下就进下载页的对应分类——不再有单独的「下载」入口，
 * 这样右侧内容区可以占满宽度。
 */
import Icon from "./Icon.vue";
import SkinAvatar from "./SkinAvatar.vue";
import { openDownload, state } from "../store";
import type { DownloadSection, ViewKey } from "../types";

interface NavItem {
  key: ViewKey;
  label: string;
  icon: string;
}

/** 上半部分：首页 / 版本管理 / 模组管理 / 联机 / 设置，紧挨着排在一起。 */
const top: NavItem[] = [
  { key: "home", label: "首页", icon: "home" },
  { key: "versions", label: "版本管理", icon: "versions" },
  { key: "mods", label: "模组管理", icon: "mods" },
  { key: "netplay", label: "联机", icon: "netplay" },
  { key: "settings", label: "设置", icon: "settings" },
];

const downloadItems: { key: DownloadSection; label: string; icon: string }[] = [
  { key: "vanilla", label: "原版游戏", icon: "package" },
  { key: "mod", label: "Mod", icon: "mods" },
  { key: "modpack", label: "整合包", icon: "modpack" },
  { key: "datapack", label: "数据包", icon: "datapack" },
  { key: "resourcepack", label: "材质包", icon: "texture" },
  { key: "shader", label: "光影包", icon: "shader" },
];

/** 版本号来自 package.json（vite define 注入），只维护一处 */
const version = __APP_VERSION__;

const activeCount = () =>
  state.queue.filter((q) => q.state === "running" || q.state === "pending").length;

/** 下载分类的选中态：既要在下载页，还要对得上分类。 */
function isDownloadActive(key: DownloadSection) {
  return state.view === "downloads" && state.downloadSection === key;
}
</script>

<template>
  <aside
    class="flex w-56 shrink-0 flex-col border-r border-line bg-surface-muted/70 backdrop-blur-xl"
  >
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

    <nav class="min-h-0 flex-1 overflow-y-auto px-3 pb-3">
      <!-- 一级 -->
      <button
        v-for="item in top"
        :key="item.key"
        class="group relative mb-1 flex w-full items-center gap-3 rounded-xl px-3 py-2.5 text-left text-sm font-semibold transition-all duration-200 ease-smooth"
        :class="
          state.view === item.key
            ? 'bg-brand-500/[0.16] text-brand-200'
            : 'text-ink-500 hover:bg-white/[0.05] hover:text-white'
        "
        @click="state.view = item.key"
      >
        <span
          class="absolute left-0 top-1/2 h-5 w-[3px] -translate-y-1/2 rounded-pill bg-brand-600 transition-all duration-200"
          :class="state.view === item.key ? 'opacity-100' : 'opacity-0'"
        />
        <Icon
          :name="item.icon"
          :size="19"
          :class="state.view === item.key ? 'text-brand-300' : 'text-ink-400 group-hover:text-ink-700'"
        />
        <span class="flex-1">{{ item.label }}</span>
      </button>

      <!-- 下载分类 -->
      <div class="mb-1 mt-3 flex items-center gap-2 px-3">
        <span class="text-[10px] font-bold tracking-wider text-ink-300">下载</span>
        <span
          v-if="activeCount() > 0"
          class="chip !px-1.5 !py-0 bg-brand-600 text-white"
          :title="`${activeCount()} 个任务进行中`"
        >
          {{ activeCount() }}
        </span>
      </div>

      <button
        v-for="item in downloadItems"
        :key="item.key"
        class="group relative mb-0.5 flex w-full items-center gap-2.5 rounded-xl py-2 pl-5 pr-3 text-left text-[13px] font-semibold transition-all duration-200 ease-smooth"
        :class="
          isDownloadActive(item.key)
            ? 'bg-brand-500/[0.16] text-brand-200'
            : 'text-ink-500 hover:bg-white/[0.05] hover:text-white'
        "
        @click="openDownload(item.key)"
      >
        <span
          class="absolute left-0 top-1/2 h-4 w-[3px] -translate-y-1/2 rounded-pill bg-brand-600 transition-all duration-200"
          :class="isDownloadActive(item.key) ? 'opacity-100' : 'opacity-0'"
        />
        <Icon
          :name="item.icon"
          :size="17"
          :class="
            isDownloadActive(item.key)
              ? 'text-brand-300'
              : 'text-ink-400 group-hover:text-ink-700'
          "
        />
        <span class="flex-1">{{ item.label }}</span>
      </button>
    </nav>

    <!-- 底部状态：Java + 版本号，路径太长的不占地方，鼠标悬停能看 -->
    <div class="border-t border-line px-5 py-3.5" :title="state.paths.root">
      <div class="flex items-center justify-between gap-2 text-[11px] font-medium text-ink-400">
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
