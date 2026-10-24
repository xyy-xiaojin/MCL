<script setup lang="ts">
import { ref } from "vue";

import Icon from "../components/Icon.vue";
import { api, errorText } from "../api";
import { notify } from "../store";
import type { ModpackHit } from "../types";

const query = ref("optimized");
const hits = ref<ModpackHit[]>([]);
const searching = ref(false);
const searched = ref(false);

async function search() {
  if (searching.value) return;
  searching.value = true;
  try {
    hits.value = await api.searchModpacks(query.value);
    searched.value = true;
  } catch (e) {
    notify(errorText(e), "error");
  } finally {
    searching.value = false;
  }
}

function num(value: number): string {
  return value >= 10000 ? `${(value / 1000).toFixed(0)}k` : String(value);
}
</script>

<template>
  <div class="flex h-full min-h-0 flex-col">
    <div class="min-h-0 flex-1 space-y-4 overflow-y-auto pr-1">
    <!-- Modrinth 搜索 -->
    <section class="card p-5">
      <div class="mb-3 flex items-center gap-2 text-sm font-bold text-ink-900">
        <Icon name="search" :size="18" class="text-brand-300" />
        搜索整合包
        <span class="chip bg-brand-500/[0.16] text-brand-200">Modrinth</span>
      </div>

      <div class="flex gap-2">
        <input
          v-model="query"
          class="field flex-1"
          placeholder="例如 optimized / fabric / skyblock"
          @keyup.enter="search"
        />
        <button class="btn-primary !px-5" :disabled="searching" @click="search">
          <Icon name="search" :size="16" :class="searching ? 'animate-spin' : ''" />
          {{ searching ? "搜索中" : "搜索" }}
        </button>
      </div>

      <div v-if="hits.length > 0" class="mt-4 grid gap-2.5 md:grid-cols-2">
        <article
          v-for="hit in hits"
          :key="hit.id"
          class="flex gap-3 rounded-xl border border-line p-3 transition-all duration-200 hover:border-brand-200 hover:bg-white/[0.04]"
        >
          <img
            v-if="hit.iconUrl"
            :src="hit.iconUrl"
            class="h-12 w-12 shrink-0 rounded-xl bg-surface-sunken object-cover"
            alt=""
            @error="($event.target as HTMLImageElement).style.visibility = 'hidden'"
          />
          <div v-else class="grid h-12 w-12 shrink-0 place-items-center rounded-xl bg-brand-500/[0.14] text-brand-300">
            <Icon name="modpack" :size="20" />
          </div>

          <div class="min-w-0 flex-1">
            <div class="truncate text-xs font-bold text-ink-900" :title="hit.title">
              {{ hit.title }}
            </div>
            <p class="mt-0.5 line-clamp-2 text-[11px] leading-snug text-ink-400">
              {{ hit.description }}
            </p>
            <div class="mt-1.5 flex items-center gap-2 text-[10px] font-semibold text-ink-300">
              <span>{{ num(hit.downloads) }} 次下载</span>
              <span v-if="hit.versions.length">· MC {{ hit.versions[0] }}</span>
            </div>
          </div>
        </article>
      </div>

      <div
        v-else-if="searched && !searching"
        class="mt-4 rounded-xl bg-surface-sunken px-3.5 py-6 text-center text-xs text-ink-400"
      >
        没有找到匹配的整合包
      </div>
    </section>

    <!-- 本地包：导入入口统一是拖拽，这里只说明一句 -->
    <section class="card flex items-start gap-3 p-4">
      <div class="grid h-9 w-9 shrink-0 place-items-center rounded-xl bg-brand-500/[0.14] text-brand-300">
        <Icon name="folder" :size="18" />
      </div>
      <div class="min-w-0">
        <div class="text-xs font-bold text-ink-900">导入本地整合包</div>
        <p class="mt-1 text-[11px] leading-relaxed text-ink-400">
          把
          <code class="rounded bg-surface-sunken px-1">.mrpack</code>
          或打包好的 <code class="rounded bg-surface-sunken px-1">.zip</code>
          直接拖进窗口任意位置就会导入——松手前会盖一层提示，确认无误再放。
          导入会新建一个独立版本，并自动切换过去。
        </p>
      </div>
    </section>
    </div>
  </div>
</template>
