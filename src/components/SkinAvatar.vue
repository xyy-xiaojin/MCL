<script setup lang="ts">
/**
 * Minecraft 皮肤头像。
 *
 * 和 PCL 的 MySkin 控件一样从皮肤贴图里裁两块叠起来：
 * - 脸：(8,8) 起的 8×8
 * - 帽子/外层：(40,8) 起的 8×8，盖在脸上（默认皮肤这里是全透明）
 *
 * 坐标统一按 64×64 的网格算，所以 128×128 / 256×256 的 HD 皮肤同样正确——
 * 它们只是同一张图的整数倍放大，脸和外层的位置比例不变。
 *
 * 视觉上做了三件事，都不影响裁切坐标：
 * 1. 圆角按尺寸等比算（`size * 0.26`），小头像和大卡片用同一套比例；
 * 2. 外面套一层模糊的紫色辉光，鼠标悬停时变亮、头像轻微放大；
 * 3. 蒙一层"上亮下暗"的渐变，让方块头有点体积感，但透明度压得很低，
 *    不糊掉像素细节（这是皮肤，不是插画）。
 */
import { computed } from "vue";

import alexUrl from "../assets/alex.png";
import steveUrl from "../assets/steve.png";

const props = withDefaults(
  defineProps<{
    /** 皮肤贴图地址（data URL 或本地文件）；不传就用内置 Steve / Alex */
    src?: string;
    /** 没有真实皮肤时用哪个默认模型 */
    model?: "steve" | "alex";
    /** 头像边长（px） */
    size?: number;
  }>(),
  { src: "", model: "steve", size: 40 },
);

const image = computed(() => props.src || (props.model === "alex" ? alexUrl : steveUrl));

/** 圆角跟着尺寸走，1:1 放大时观感才一致 */
const radius = computed(() => `${Math.round(props.size * 0.26)}px`);

const avatarStyle = computed(() => {
  // 皮肤里 1 个像素放大成 s 个屏幕像素，整张 64 格宽的图就是 64s
  const s = props.size / 8;
  const box = `${64 * s}px ${64 * s}px`;
  const url = `url("${image.value}")`;
  return {
    width: `${props.size}px`,
    height: `${props.size}px`,
    // 第一层帽子（外层）在最上面，第二层脸垫底
    backgroundImage: `${url}, ${url}`,
    backgroundSize: `${box}, ${box}`,
    backgroundPosition: `${-40 * s}px ${-8 * s}px, ${-8 * s}px ${-8 * s}px`,
    imageRendering: "pixelated",
  } as const;
});

const label = computed(() =>
  props.model === "alex" ? "玩家头像（Alex 细手臂模型）" : "玩家头像（Steve 宽手臂模型）",
);
</script>

<template>
  <div
    class="group/avatar relative shrink-0"
    :style="{ width: `${props.size}px`, height: `${props.size}px`, borderRadius: radius }"
    :title="label"
  >
    <!-- 辉光垫在头像下面：只有溢出圆角的那圈会透出来，像个低调的背光 -->
    <span
      class="pointer-events-none absolute -inset-1.5 opacity-60 blur-lg transition-opacity duration-300 group-hover/avatar:opacity-100"
      :style="{
        borderRadius: 'inherit',
        background:
          'radial-gradient(circle at 50% 30%, rgba(139,92,246,0.75), rgba(34,211,238,0.25) 55%, transparent 72%)',
      }"
    />

    <!-- 头像本体：脸垫底、帽子层在上；ring-inset 保证不改变可裁切区域 -->
    <div
      class="relative h-full w-full ring-1 ring-inset ring-white/20 transition-transform duration-300 ease-out group-hover/avatar:scale-[1.05]"
      :style="{ ...avatarStyle, borderRadius: radius }"
      role="img"
      :aria-label="label"
    />

    <!-- 上亮下暗的体积光；透明度压得很低，像素细节仍然清楚 -->
    <span
      class="pointer-events-none absolute inset-0 bg-gradient-to-b from-white/[0.16] via-transparent to-black/[0.28]"
      :style="{ borderRadius: radius }"
    />
  </div>
</template>
