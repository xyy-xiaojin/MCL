<script setup lang="ts">
import { computed, onMounted, ref } from "vue";

import Icon from "../components/Icon.vue";
import SkinAvatar from "../components/SkinAvatar.vue";
import { api, errorText } from "../api";
import {
  downloadJavaRuntime,
  importSkinFile,
  killGame,
  launchSelected,
  notify,
  refreshAvatar,
  refreshJava,
  resetSkin,
  saveSettings,
  state,
} from "../store";
import type {
  AuthAccountView,
  AuthProfileChoice,
  MsDeviceCode,
} from "../types";

const selected = computed(
  () => state.local.find((v) => v.id === state.selectedVersion) ?? null,
);

// ---- 离线皮肤 ----

/** 隐藏的 file input：点「导入皮肤」时代它弹系统选择框 */
const skinInput = ref<HTMLInputElement | null>(null);

/** 头像是不是玩家自己导的 —— 决定要不要显示「恢复默认」和手臂切换 */
const hasLocalSkin = computed(() => state.avatar.source === "local");

const skinBadge = computed(() => {
  if (state.avatar.source === "local") return "本地皮肤";
  if (state.avatar.source === "authlib") return "皮肤站皮肤";
  if (state.avatar.source === "mojang") return "正版皮肤";
  return "默认皮肤";
});

function pickSkin() {
  skinInput.value?.click();
}

async function onSkinPicked(event: Event) {
  const input = event.target as HTMLInputElement;
  const file = input.files?.[0];
  // 立刻清空：否则连续选同一个文件不会再触发 change
  input.value = "";
  if (file) await importSkinFile(file);
}

/** 自动模式下显示实际会用的值，手动模式显示滑块值 */
const effectiveMb = computed(
  () => state.memory?.effectiveMaxMb ?? state.settings.maxMemoryMb,
);

const memoryLabel = computed(() => `${(effectiveMb.value / 1024).toFixed(1)} GB`);

const autoRam = computed(() => state.settings.ramType !== "manual");

const recommendLabel = computed(
  () => `${((state.memory?.plan.recommendMb ?? 0) / 1024).toFixed(1)} GB`,
);

const availableLabel = computed(() => {
  const mb = state.memory?.plan.availableMb ?? 0;
  return mb > 0 ? `${(mb / 1024).toFixed(1)} GB` : "未知";
});

/** 滑块上限跟着物理内存走，别让用户设出跑不起来的数字 */
const sliderMax = computed(() => {
  const total = state.memory?.plan.totalMb ?? 0;
  if (total > 0) return Math.min(32768, Math.max(2048, Math.round(total / 1024) * 1024));
  return 16384;
});

const overLimit = computed(() => {
  const safe = state.memory?.safeLimitMb ?? 0;
  return safe > 0 && effectiveMb.value > safe;
});

const safeLimitLabel = computed(
  () => `${(((state.memory?.safeLimitMb ?? 0) / 1024) | 0).toFixed(0)} GB`,
);

const gcModeLabel = computed(() => {
  const map: Record<string, string> = {
    auto: "自动（按 Java 版本选 ZGC / G1）",
    g1: "G1GC",
    "g1-optimized": "优化 G1GC",
    custom: "自定义（不加参数）",
  };
  return map[state.settings.gcMode] ?? "自动";
});

/** 切到手动时用建议值当起点，省得玩家对着滑块猜 */
async function switchToManual() {
  state.settings.maxMemoryMb = state.memory?.plan.recommendMb ?? 4096;
  state.settings.ramType = "manual";
  await saveSettings();
}

async function switchToAuto() {
  state.settings.ramType = "auto";
  await saveSettings();
}

const javaTone = computed(() => {
  if (!state.java) return "text-amber-400";
  return "text-emerald-400";
});

/** 当前版本需要的 Java 主版本（版本列表里带 required_java_major 的结果） */
const neededJavaMajor = computed(() => selected.value?.javaMajor ?? 0);

/** Java 缺失或版本不够时显示一键下载按钮 */
const javaNeedsInstall = computed(() => {
  if (neededJavaMajor.value <= 0) return !state.java;
  return !state.java || state.java.major < neededJavaMajor.value;
});

/** 启动按钮上要显示的 Java 版本（缺 Java 时提示要装哪个） */
const javaBadge = computed(() => {
  if (state.java?.major) return `Java ${state.java.major}`;
  if (neededJavaMajor.value > 0) return `需要 Java ${neededJavaMajor.value}`;
  return "Java ?";
});

/** 当前版本是不是正在运行（重复启动防护的界面提示） */
const runningGame = computed(
  () =>
    state.runningGames.find((g) => g.version === state.selectedVersion) ??
    null,
);

const canLaunch = computed(
  () =>
    Boolean(state.selectedVersion) && !state.launching && !runningGame.value,
);

/** 游戏已运行多少分钟（卡片上的小字） */
function runningMinutes(startedAtMs: number): string {
  const mins = Math.max(0, Math.round((Date.now() - startedAtMs) / 60000));
  if (mins < 1) return "刚刚";
  if (mins < 60) return `${mins} 分钟`;
  return `${Math.floor(mins / 60)} 小时 ${mins % 60} 分`;
}

// ---- 账户：离线 / 皮肤站（authlib-injector）/ 微软正版 ----

/** 预置皮肤站。自定义地址填 URL，后端会先验元数据再登录。 */
const PRESET_SERVERS = [
  { label: "LittleSkin", url: "https://littleskin.cn/api/yggdrasil" },
];

const auth = ref<AuthAccountView | null>(null);
const accountTab = ref<"offline" | "authlib" | "ms">("offline");
const busyAuth = ref(false);
const serverChoice = ref<string>(PRESET_SERVERS[0].url);
const customServer = ref("");
const email = ref("");
const password = ref("");
/** 多角色账号登录后待选的角色列表 */
const pendingProfiles = ref<AuthProfileChoice[]>([]);

// ---- 微软设备码登录 ----
const msDevice = ref<MsDeviceCode | null>(null);
let msPolling = false;

onMounted(async () => {
  try {
    auth.value = await api.authStatus();
    if (auth.value.kind === "authlib") accountTab.value = "authlib";
    if (auth.value.kind === "microsoft") accountTab.value = "ms";
  } catch {
    // 浏览器预览（无 Tauri 运行时）时静默
  }
});

const authLoggedIn = computed(
  () =>
    (auth.value?.kind === "authlib" || auth.value?.kind === "microsoft") &&
    auth.value.loggedIn,
);

/** 启动卡的身份角标 */
const accountLabel = computed(() => {
  if (!authLoggedIn.value) return "离线模式";
  if (auth.value?.kind === "microsoft") return "微软正版";
  return `皮肤站 · ${auth.value?.serverName || "已登录"}`;
});

const activeServerUrl = computed(() =>
  serverChoice.value === "__custom__" ? customServer.value : serverChoice.value,
);

async function testServer() {
  busyAuth.value = true;
  try {
    const info = await api.authServerInfo(activeServerUrl.value);
    notify(`「${info.name}」连接正常`, "success");
  } catch (e) {
    notify(errorText(e), "error");
  } finally {
    busyAuth.value = false;
  }
}

async function authLogin() {
  if (!email.value.trim() || !password.value) {
    notify("请填写皮肤站的邮箱和密码", "error");
    return;
  }
  busyAuth.value = true;
  try {
    const result = await api.authLogin(
      activeServerUrl.value,
      email.value.trim(),
      password.value,
    );
    auth.value = result.account;
    if (result.profiles.length) {
      pendingProfiles.value = result.profiles;
      notify("登录成功，账号下有多个角色，请选一个", "info");
    } else {
      password.value = "";
      notify(
        `已登录「${result.account.serverName}」· ${result.account.profileName}`,
        "success",
      );
      await refreshAvatar();
    }
  } catch (e) {
    notify(errorText(e), "error");
  } finally {
    busyAuth.value = false;
  }
}

async function pickProfile(profile: AuthProfileChoice) {
  busyAuth.value = true;
  try {
    auth.value = await api.authSelectProfile(profile.id, profile.name);
    pendingProfiles.value = [];
    password.value = "";
    notify(`角色「${profile.name}」已选中`, "success");
    await refreshAvatar();
  } catch (e) {
    notify(errorText(e), "error");
  } finally {
    busyAuth.value = false;
  }
}

async function authLogout() {
  busyAuth.value = true;
  try {
    await api.authLogout();
    auth.value = {
      kind: "offline",
      serverUrl: "",
      serverName: "",
      loginEmail: "",
      profileId: "",
      profileName: "",
      loggedIn: false,
    };
    pendingProfiles.value = [];
    msDevice.value = null;
    msPolling = false;
    notify("已退出登录，回到离线模式", "info");
    await refreshAvatar();
  } catch (e) {
    notify(errorText(e), "error");
  } finally {
    busyAuth.value = false;
  }
}

// ---- 微软正版：设备码流程 ----

async function msBegin() {
  busyAuth.value = true;
  try {
    msDevice.value = await api.authMsBegin();
    void api.openInBrowser(msDevice.value.verificationUri).catch(() => {
      // 打不开浏览器就让玩家手动复制链接，不拦流程
    });
    msPolling = true;
    notify(
      `请在弹出的页面输入代码 ${msDevice.value.userCode}`,
      "info",
    );
    void msPollLoop(msDevice.value.deviceCode, msDevice.value.interval);
  } catch (e) {
    notify(errorText(e), "error");
  } finally {
    busyAuth.value = false;
  }
}

/** 按 interval 轮询直到授权完成、过期或被取消 */
async function msPollLoop(deviceCode: string, interval: number) {
  const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
  // 设备码有效期到了就停，避免无限轮询
  const deadline = Date.now() + 15 * 60 * 1000;
  while (msPolling && Date.now() < deadline) {
    await sleep(interval * 1000);
    if (!msPolling) return;
    try {
      const result = await api.authMsPoll(deviceCode);
      if (result.state === "pending") continue;
      msPolling = false;
      if (result.account) {
        auth.value = result.account;
        msDevice.value = null;
        notify(`微软正版登录成功：${result.account.profileName}`, "success");
        await refreshAvatar();
      }
      return;
    } catch (e) {
      msPolling = false;
      msDevice.value = null;
      notify(errorText(e), "error");
      return;
    }
  }
  if (msPolling) {
    msPolling = false;
    msDevice.value = null;
    notify("设备码过期了，请重新开始登录", "error");
  }
}

function msCancel() {
  msPolling = false;
  msDevice.value = null;
}

</script>

<template>
  <div class="flex h-full min-h-0 flex-col gap-4">
    <!-- 主启动卡：PCL2 的"启动"是页面上唯一的视觉焦点 -->
    <section
      class="relative shrink-0 overflow-hidden rounded-card bg-gradient-to-br from-brand-600 via-brand-600 to-brand-800 p-6 text-white shadow-[0_24px_60px_-30px_rgba(76,29,149,0.9)]"
    >
      <div
        class="pointer-events-none absolute -right-16 -top-24 h-64 w-64 rounded-full bg-white/10 blur-2xl"
      />
      <div
        class="pointer-events-none absolute -bottom-24 right-24 h-56 w-56 rounded-full bg-brand-300/20 blur-3xl"
      />

      <div class="relative flex flex-wrap items-center justify-between gap-6">
        <div class="min-w-0">
          <div class="flex items-center gap-2 text-xs font-semibold uppercase tracking-widest text-white/70">
            <Icon name="bolt" :size="14" />
            Minecraft · {{ accountLabel }}
          </div>
          <h2 class="mt-2 truncate text-3xl font-black tracking-tight">
            {{ state.selectedVersion || "尚未选择版本" }}
          </h2>
          <div class="mt-3 flex flex-wrap items-center gap-2 text-xs font-semibold">
            <span class="rounded-pill bg-white/15 px-3 py-1">
              内存 {{ memoryLabel }}
            </span>
            <span class="rounded-pill bg-white/15 px-3 py-1">
              {{ javaBadge }}
            </span>
            <span class="rounded-pill bg-white/15 px-3 py-1">
              {{ state.local.length }} 个本地版本
            </span>
          </div>
        </div>

        <div class="flex flex-col items-stretch gap-2">
          <!-- 游戏正在运行：启动位换成"结束游戏"，一眼能看清状态 -->
          <template v-if="runningGame">
            <div class="rounded-xl bg-white/[0.12] px-4 py-2.5 text-center text-xs font-bold backdrop-blur-md">
              游戏运行中 · 已 {{ runningMinutes(runningGame.startedAtMs) }}
            </div>
            <button
              class="btn min-w-[190px] border border-rose-300/40 bg-rose-500/25 px-8 py-4 text-base !text-white shadow-[0_16px_36px_-14px_rgba(0,0,0,0.55)] backdrop-blur-md hover:bg-rose-500/40 active:scale-[0.98]"
              @click="killGame(state.selectedVersion)"
            >
              <Icon name="close" :size="18" />
              结束游戏
            </button>
          </template>
          <button
            v-else
            class="btn min-w-[190px] border border-white/25 bg-white/[0.14] px-8 py-4 text-base !text-white shadow-[0_16px_36px_-14px_rgba(0,0,0,0.55)] backdrop-blur-md hover:bg-white/[0.22] active:scale-[0.98] disabled:opacity-60"
            :disabled="!canLaunch"
            @click="launchSelected"
          >
            <Icon :name="state.launching ? 'refresh' : 'play'" :size="20" />
            {{ state.launching ? "启动中…" : "启动游戏" }}
          </button>
        </div>
      </div>
    </section>

    <!-- 无版本提示 -->
    <section
      v-if="state.local.length === 0"
      class="card flex shrink-0 items-center gap-4 p-4"
    >
      <div class="grid h-11 w-11 place-items-center rounded-2xl bg-amber-500/[0.14] text-amber-400">
        <Icon name="package" :size="22" />
      </div>
      <div class="flex-1">
        <div class="text-sm font-bold text-ink-900">第一次用？三步就能玩</div>
        <p class="text-xs leading-relaxed text-ink-400">
          ① 去右上角「版本管理」装一个正式版 → ② 装好后点版本卡片上的
          Fabric / Forge 按钮加模组支持（不装 mod 可跳过）→ ③ 回这里点「启动游戏」。
          没 Java 的话下方会提示一键下载，不用自己去官网找。
        </p>
      </div>
      <button class="btn-outline" @click="state.view = 'versions'">
        去安装
        <Icon name="versions" :size="16" />
      </button>
    </section>

    <!-- 下半部分：卡片网格自己滚，页面本身不动 -->
    <div class="grid min-h-0 flex-1 content-start gap-4 overflow-y-auto pr-1 lg:grid-cols-2">
      <!-- 版本选择 -->
      <section class="card card-hover p-5">
        <div class="mb-3 flex items-center justify-between">
          <div class="flex items-center gap-2 text-sm font-bold text-ink-900">
            <Icon name="versions" :size="18" class="text-brand-300" />
            游戏版本
          </div>
          <span class="text-[11px] text-ink-400">
            {{ selected ? selected.kind : "—" }}
          </span>
        </div>

        <select v-model="state.selectedVersion" class="field cursor-pointer">
          <option v-if="state.local.length === 0" value="">暂无本地版本</option>
          <option v-for="version in state.local" :key="version.id" :value="version.id">
            {{ version.id }}
          </option>
        </select>

        <p class="mt-2.5 text-[11px] leading-relaxed text-ink-400">
          需要 <span class="font-semibold text-ink-500">Java {{ selected?.javaMajor ?? "?" }}</span>
          · 目录 <span class="font-mono text-[10px] text-ink-300">{{ selected?.dir ?? "—" }}</span>
        </p>
      </section>

      <!-- 账户（离线 / 皮肤站）+ 皮肤 -->
      <!-- 注意：这卡绝不能带 overflow-hidden / min-h-0 —— 那会让网格把它的
           最小高度按 0 参与，行高被旁边较矮的"游戏版本"卡拉平，内容被裁。
           行高必须由本卡内容撑起来；窗口太矮时由外层网格整体滚动。 -->
      <section class="card card-hover flex flex-col p-5">
        <div class="mb-4 flex shrink-0 items-center justify-between">
          <div class="flex items-center gap-2 text-sm font-bold text-ink-900">
            <Icon name="home" :size="18" class="text-brand-300" />
            账户
          </div>
          <span
            class="chip transition-colors"
            :class="
              authLoggedIn
                ? 'bg-emerald-500/15 text-emerald-300'
                : hasLocalSkin
                  ? 'bg-brand-500/15 text-brand-200'
                  : 'bg-surface-sunken text-ink-400'
            "
          >
            {{ skinBadge }}
          </span>
        </div>

        <!-- 模式切换 -->
        <div class="mb-4 flex gap-2 rounded-xl bg-white/[0.04] p-1">
          <button
            class="flex-1 rounded-lg py-1.5 text-xs font-bold transition-all"
            :class="
              accountTab === 'offline'
                ? 'bg-brand-600 text-white shadow-[0_6px_18px_-8px_rgba(124,58,237,0.9)]'
                : 'text-ink-500 hover:text-white'
            "
            @click="accountTab = 'offline'"
          >
            离线模式
          </button>
          <button
            class="flex-1 rounded-lg py-1.5 text-xs font-bold transition-all"
            :class="
              accountTab === 'authlib'
                ? 'bg-brand-600 text-white shadow-[0_6px_18px_-8px_rgba(124,58,237,0.9)]'
                : 'text-ink-500 hover:text-white'
            "
            @click="accountTab = 'authlib'"
          >
            皮肤站登录
          </button>
          <button
            class="flex-1 rounded-lg py-1.5 text-xs font-bold transition-all"
            :class="
              accountTab === 'ms'
                ? 'bg-brand-600 text-white shadow-[0_6px_18px_-8px_rgba(124,58,237,0.9)]'
                : 'text-ink-500 hover:text-white'
            "
            @click="accountTab = 'ms'"
          >
            微软正版
          </button>
        </div>

        <div class="min-h-0 flex-1 overflow-y-auto pr-1">
        <!-- 皮肤站模式 -->
        <template v-if="accountTab === 'authlib'">
          <!-- 已登录：角色卡 -->
          <div v-if="authLoggedIn" class="flex items-start gap-4">
            <SkinAvatar
              :size="64"
              :model="state.avatar.model"
              :src="state.avatar.image ?? ''"
            />
            <div class="min-w-0 flex-1">
              <div class="truncate text-lg font-extrabold text-white">
                {{ auth?.profileName }}
              </div>
              <div class="mt-0.5 text-[11px] text-ink-400">
                {{ auth?.serverName }}（{{ auth?.loginEmail }}）
              </div>
              <button
                class="btn-ghost mt-2 !px-2.5 !py-1.5 !text-[11px] text-ink-500 hover:bg-rose-500/[0.12] hover:text-rose-300"
                :disabled="busyAuth"
                @click="authLogout"
              >
                <Icon name="refresh" :size="13" />
                退出登录
              </button>
            </div>
          </div>

          <!-- 未登录：登录表单 -->
          <template v-else>
            <div class="space-y-3">
              <div>
                <label class="label">皮肤站</label>
                <select v-model="serverChoice" class="field cursor-pointer">
                  <option v-for="server in PRESET_SERVERS" :key="server.url" :value="server.url">
                    {{ server.label }}
                  </option>
                  <option value="__custom__">自定义地址…</option>
                </select>
              </div>
              <div v-if="serverChoice === '__custom__'">
                <label class="label">yggdrasil 地址</label>
                <input
                  v-model="customServer"
                  class="field font-mono text-xs"
                  placeholder="例如 https://mcskin.example.com/api/yggdrasil"
                />
              </div>
              <div>
                <label class="label">邮箱 / 用户名</label>
                <input
                  v-model="email"
                  class="field"
                  maxlength="64"
                  placeholder="皮肤站的登录邮箱"
                  @keyup.enter="authLogin"
                />
              </div>
              <div>
                <label class="label">密码</label>
                <input
                  v-model="password"
                  class="field"
                  type="password"
                  maxlength="64"
                  placeholder="不会保存，只在登录时用一次"
                  @keyup.enter="authLogin"
                />
              </div>
            </div>

            <!-- 多角色：登录后让玩家选 -->
            <div v-if="pendingProfiles.length" class="mt-3 space-y-1.5">
              <div class="text-[11px] font-semibold text-ink-400">
                这个账号有 {{ pendingProfiles.length }} 个角色，选一个进游戏：
              </div>
              <button
                v-for="profile in pendingProfiles"
                :key="profile.id"
                class="flex w-full items-center gap-2 rounded-xl border border-line bg-white/[0.02] px-3 py-2 text-left text-[13px] font-bold text-ink-900 transition hover:border-brand-500/50 hover:bg-brand-500/[0.12]"
                :disabled="busyAuth"
                @click="pickProfile(profile)"
              >
                <SkinAvatar
                  :size="24"
                  :model="state.avatar.model"
                  :src="''"
                />
                {{ profile.name }}
              </button>
            </div>

            <div class="mt-4 flex items-center gap-2">
              <button class="btn-primary flex-1" :disabled="busyAuth" @click="authLogin">
                <Icon name="key" :size="15" />
                {{ busyAuth ? "处理中…" : "登录" }}
              </button>
              <button class="btn-outline shrink-0" :disabled="busyAuth" @click="testServer">
                测试连接
              </button>
            </div>
            <p class="mt-3 text-[11px] leading-relaxed text-ink-400">
              用皮肤站的账号密码登录，游戏里会通过
              <span class="font-mono text-ink-500">authlib-injector</span>
              走皮肤站验证，皮肤、披风、角色名都和皮肤站一致。密码只在登录时用一次，不会被保存。
            </p>
          </template>
        </template>

        <!-- 微软正版模式 -->
        <template v-else-if="accountTab === 'ms'">
          <!-- 已登录：角色卡 -->
          <div v-if="authLoggedIn && auth?.kind === 'microsoft'" class="flex items-start gap-4">
            <SkinAvatar
              :size="64"
              :model="state.avatar.model"
              :src="state.avatar.image ?? ''"
            />
            <div class="min-w-0 flex-1">
              <div class="truncate text-lg font-extrabold text-white">
                {{ auth?.profileName }}
              </div>
              <div class="mt-0.5 text-[11px] text-ink-400">
                微软正版 · {{ auth?.loginEmail || "Minecraft 账号" }}
              </div>
              <button
                class="btn-ghost mt-2 !px-2.5 !py-1.5 !text-[11px] text-ink-500 hover:bg-rose-500/[0.12] hover:text-rose-300"
                :disabled="busyAuth"
                @click="authLogout"
              >
                <Icon name="refresh" :size="13" />
                退出登录
              </button>
            </div>
          </div>

          <!-- 未登录：设备码流程 -->
          <template v-else>
            <div v-if="!msDevice" class="space-y-3">
              <button class="btn-primary w-full" :disabled="busyAuth" @click="msBegin">
                <Icon name="key" :size="15" />
                {{ busyAuth ? "获取设备码中…" : "用微软账号登录" }}
              </button>
              <p class="text-[11px] leading-relaxed text-ink-400">
                点「登录」会给你一个代码并打开浏览器，在微软页面输入代码确认即可。
                正版账号进游戏不需要任何注入，皮肤、披风、角色名都是官方的。
                启动器只会保存一个续期令牌，不会保存你的微软密码。
              </p>
            </div>
            <div v-else class="space-y-3">
              <div class="rounded-xl border border-brand-500/25 bg-brand-500/[0.08] px-4 py-4 text-center">
                <div class="text-[10px] font-bold tracking-wider text-brand-300/80">
                  在浏览器里输入这个代码
                </div>
                <div class="mt-1 font-mono text-2xl font-black tracking-[0.3em] text-white">
                  {{ msDevice.userCode }}
                </div>
                <div class="mt-1 truncate font-mono text-[10px] text-ink-400">
                  {{ msDevice.verificationUri }}
                </div>
              </div>
              <div class="flex items-center gap-2">
                <span class="flex-1 text-[11px] text-ink-400">
                  <Icon name="refresh" :size="12" class="mr-1 inline animate-spin" />
                  等待你在浏览器里确认…
                </span>
                <button class="btn-outline shrink-0 !py-1.5 !text-xs" @click="msCancel">
                  取消
                </button>
              </div>
            </div>
          </template>
        </template>

        <!-- 离线模式 -->
        <template v-else>
          <div class="flex items-start gap-4">
            <!-- 头像跟名字联动：离线账号优先用导入的皮肤，没有就是内置 Steve -->
            <SkinAvatar
              :size="64"
              :model="state.avatar.model"
              :src="state.avatar.image ?? ''"
            />

            <div class="min-w-0 flex-1">
              <input
                v-model="state.settings.username"
                class="field"
                maxlength="16"
                placeholder="输入玩家名，例如 Steve"
              />

              <div class="mt-2 flex flex-wrap items-center gap-1.5">
                <button class="btn-outline !px-2.5 !py-1.5 !text-[11px]" @click="pickSkin">
                  <Icon name="upload" :size="13" />
                  {{ hasLocalSkin ? "换一张" : "导入皮肤" }}
                </button>
                <button
                  v-if="hasLocalSkin"
                  class="btn-ghost !px-2.5 !py-1.5 !text-[11px] text-ink-500 hover:bg-rose-500/[0.12] hover:text-rose-300"
                  @click="resetSkin"
                >
                  <Icon name="refresh" :size="13" />
                  恢复默认
                </button>
              </div>
            </div>
          </div>

          <p class="mt-3 border-t border-line pt-2.5 text-[11px] leading-relaxed text-ink-400">
            皮肤只影响启动器头像；游戏内换肤要用皮肤站登录，或在服务端装皮肤插件。
          </p>

          <!-- 隐藏的文件选择器：WebView2 里 <input type=file> 会直接弹系统对话框 -->
          <input
            ref="skinInput"
            type="file"
            accept="image/png"
            class="hidden"
            @change="onSkinPicked"
          />
        </template>
        </div>
      </section>

      <!-- Java -->
      <section class="card card-hover p-5">
        <div class="mb-3 flex items-center justify-between">
          <div class="flex items-center gap-2 text-sm font-bold text-ink-900">
            <Icon name="cpu" :size="18" class="text-brand-300" />
            Java 运行环境
          </div>
          <button
            class="text-[11px] font-semibold text-brand-300 hover:text-brand-300"
            @click="refreshJava(true)"
          >
            重新检测
          </button>
        </div>

        <div v-if="state.java && !javaNeedsInstall" class="mb-3 rounded-xl bg-surface-sunken px-3.5 py-2.5">
          <div class="flex items-center gap-2" :class="javaTone">
            <span class="h-1.5 w-1.5 rounded-full bg-current" />
            <span class="text-xs font-bold">Java {{ state.java.major }}</span>
            <span class="text-[11px] font-medium text-ink-400">({{ state.java.source }})</span>
          </div>
          <div class="mt-1 truncate font-mono text-[10px] text-ink-400" :title="state.java.path">
            {{ state.java.path }}
          </div>
        </div>
        <div
          v-else-if="javaNeedsInstall && state.java"
          class="mb-3 rounded-xl bg-amber-500/[0.14] px-3.5 py-2.5 text-xs font-semibold text-amber-300"
        >
          当前是 Java {{ state.java.major }}，这个版本需要 Java {{ neededJavaMajor }}
        </div>
        <div
          v-else-if="javaNeedsInstall"
          class="mb-3 rounded-xl bg-amber-500/[0.14] px-3.5 py-2.5 text-xs font-semibold text-amber-300"
        >
          未检测到 Java{{ neededJavaMajor > 0 ? `，这个版本需要 Java ${neededJavaMajor}` : "" }}
        </div>

        <!-- 一键下载：没装 Java 的小白不用再去官网找安装包 -->
        <button
          v-if="javaNeedsInstall"
          class="btn-primary mb-3 w-full"
          :disabled="state.downloadingJava"
          @click="downloadJavaRuntime(neededJavaMajor > 0 ? neededJavaMajor : 21)"
        >
          <Icon :name="state.downloadingJava ? 'refresh' : 'download'" :size="15" />
          {{
            state.downloadingJava
              ? "下载中…（进度看顶栏队列）"
              : `自动下载 Java ${neededJavaMajor > 0 ? neededJavaMajor : 21} 运行时`
          }}
        </button>

        <input
          v-model="state.settings.javaPath"
          class="field font-mono text-xs"
          placeholder="留空自动探测，例如 C:\\Program Files\\Java\\jdk-17\\bin\\java.exe"
        />
        <p class="mt-2 text-[10px] leading-relaxed text-ink-400">
          下载的 Java 装在 MCL 的数据目录里，不写注册表、不污染系统；下载源用清华镜像，官方直连兜底。
        </p>
      </section>

      <!-- 内存 -->
      <section class="card card-hover p-5">
        <div class="mb-3 flex items-center justify-between">
          <div class="flex items-center gap-2 text-sm font-bold text-ink-900">
            <Icon name="bolt" :size="18" class="text-brand-300" />
            内存分配
          </div>
          <span class="chip bg-brand-500/[0.16] text-brand-200">{{ memoryLabel }}</span>
        </div>

        <!-- 自动模式：给一行结论 + 采纳按钮，不用玩家自己算 -->
        <template v-if="autoRam">
          <div class="rounded-xl bg-surface-sunken px-3.5 py-3">
            <div class="flex items-baseline gap-2">
              <span class="text-lg font-extrabold text-white">{{ recommendLabel }}</span>
              <span class="text-[11px] font-semibold text-ink-400">建议值</span>
            </div>
            <p class="mt-1 text-[11px] leading-relaxed text-ink-400">
              检测到 {{ state.memory?.plan.reason || "原版" }}，可用内存
              {{ availableLabel }}
            </p>
          </div>

          <button
            class="btn-ghost mt-2.5 w-full !py-2 !text-xs"
            title="改成手动指定内存"
            @click="switchToManual"
          >
            手动指定内存
          </button>
        </template>

        <!-- 手动模式：原来的滑块 -->
        <template v-else>
          <input
            v-model.number="state.settings.maxMemoryMb"
            type="range"
            min="1024"
            :max="sliderMax"
            step="512"
            class="w-full accent-brand-600"
          />
          <div class="mt-1 flex justify-between text-[10px] font-medium text-ink-300">
            <span>1 GB</span>
            <span>{{ Math.round(sliderMax / 2048) }} GB</span>
            <span>{{ Math.round(sliderMax / 1024) }} GB</span>
          </div>
          <div class="mt-3 flex gap-2">
            <button
              v-for="preset in [2048, 4096, 8192]"
              :key="preset"
              class="flex-1 rounded-lg py-1.5 text-[11px] font-bold transition-colors duration-200"
              :class="
                state.settings.maxMemoryMb === preset
                  ? 'bg-brand-600 text-white'
                  : 'bg-surface-sunken text-ink-500 hover:bg-white/[0.05] hover:text-brand-200'
              "
              @click="state.settings.maxMemoryMb = preset"
            >
              {{ preset / 1024 }} GB
            </button>
          </div>
          <button
            class="mt-2 w-full text-[11px] font-semibold text-brand-300 hover:text-brand-200"
            @click="switchToAuto"
          >
            改回自动推荐
          </button>
        </template>

        <!-- 超限警告：物理内存不够时 VFS 会疯狂换页 -->
        <p
          v-if="overLimit"
          class="mt-2.5 rounded-lg bg-rose-500/[0.14] px-3 py-2 text-[11px] font-semibold text-rose-300"
        >
          已超过物理内存的 70%（{{ safeLimitLabel }}），游戏容易被系统换页拖到卡死
        </p>

        <p v-else-if="state.memory?.gcArgs.length" class="mt-2.5 text-[10px] text-ink-400">
          GC：{{ gcModeLabel }}
        </p>
      </section>
    </div>

    <p class="px-1 text-[11px] text-ink-300">
      提示：启动前会自动保存上面的设置。所有下载、哈希校验与进程创建均由 Rust 后端完成。
    </p>
  </div>
</template>
