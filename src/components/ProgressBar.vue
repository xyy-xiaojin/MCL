<script setup lang="ts">
/** 细进度条：0..1。indeterminate 时走流光动画。 */
withDefaults(
  defineProps<{
    value: number;
    height?: number;
    indeterminate?: boolean;
    tone?: "brand" | "success" | "danger";
  }>(),
  { height: 8, indeterminate: false, tone: "brand" },
);

const tones: Record<string, string> = {
  brand: "from-brand-500 to-brand-400",
  success: "from-emerald-500 to-emerald-400",
  danger: "from-rose-500 to-rose-400",
};
</script>

<template>
  <div
    class="w-full overflow-hidden rounded-pill bg-brand-500/[0.14]"
    :style="{ height: `${height}px` }"
  >
    <div
      v-if="indeterminate"
      class="h-full w-1/3 rounded-pill bg-gradient-to-r opacity-80 animate-shimmer"
      :class="tones[tone]"
      style="
        background-size: 200% 100%;
        background-image: linear-gradient(
          90deg,
          rgba(139, 92, 246, 0.25),
          rgba(139, 92, 246, 0.85),
          rgba(139, 92, 246, 0.25)
        );
      "
    />
    <div
      v-else
      class="h-full rounded-pill bg-gradient-to-r transition-[width] duration-300 ease-smooth"
      :class="tones[tone]"
      :style="{ width: `${Math.min(100, Math.max(0, value * 100))}%` }"
    />
  </div>
</template>
