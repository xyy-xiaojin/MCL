<script setup lang="ts">
import { computed, onActivated, onMounted, onUnmounted, ref } from "vue";

import Icon from "../components/Icon.vue";
import SkinAvatar from "../components/SkinAvatar.vue";
import { api, errorText } from "../api";
import {
  downloadJavaRuntime,
  importSkinFile,
  killGame,
  lastPlayed,
  launchSelected,
  notify,
  refreshAvatar,
  refreshJava,
  resetSkin,
  saveSettings,
  state,
} from "../store";
import type {
  AuthProfileChoice,
  AuthStatusView,
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

/** 账户卡是否展开：默认收起成一行，一屏才能同时看到运行环境和版本列表 */
const accountExpanded = ref(false);

/** 快捷操作里"打开目录"：路径来自后端 paths，打开失败只提示不拦 */
async function openPath(key: "game" | "versions" | "mods") {
  const target = state.paths[key];
  if (!target) return;
  try {
    await api.revealInExplorer(target);
  } catch (e) {
    notify(errorText(e), "error");
  }
}

/** 版本轨道上的"最近玩过"：没玩过返回空串，不给界面添噪音 */
function playedAgo(id: string): string {
  const at = lastPlayed[id];
  if (!at) return "";
  const mins = Math.floor((Date.now() - at) / 60000);
  if (mins < 1) return "刚刚玩过";
  if (mins < 60) return `${mins} 分钟前`;
  const hours = Math.floor(mins / 60);
  if (hours < 24) return `${hours} 小时前`;
  const days = Math.floor(hours / 24);
  if (days < 30) return `${days} 天前`;
  return `${Math.floor(days / 30)} 个月前`;
}

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

// ---- 账户：离线 / 皮肤站（authlib-injector）/ 微软正版，多账户切换 ----

/** 预置皮肤站。自定义地址填 URL，后端会先验元数据再登录。 */
const PRESET_SERVERS = [
  { label: "LittleSkin", url: "https://littleskin.cn/api/yggdrasil" },
];

/** 多账户状态（列表 + 活跃下标；离线不占列表位） */
const auth = ref<AuthStatusView | null>(null);
const busyAuth = ref(false);
const serverChoice = ref<string>(PRESET_SERVERS[0].url);
const customServer = ref("");
const email = ref("");
const password = ref("");
/** 多角色账号登录后待选的角色列表 */
const pendingProfiles = ref<AuthProfileChoice[]>([]);

// ---- 微软设备码登录 ----
const msDevice = ref<MsDeviceCode | null>(null);
/**
 * 轮询代次号：取消、重开登录、组件卸载都递增它，旧循环醒来发现
 * 代次变了就自行退出。共享布尔做不到这一点——重开时会把旧循环
 * 「复活」成双循环，两个 device_code 并发轮询还会互相误杀。
 */
let msLoopId = 0;

onUnmounted(() => {
  msLoopId++;
});

async function refreshAuthStatus() {
  try {
    auth.value = await api.authStatus();
  } catch {
    // 浏览器预览（无 Tauri 运行时）时静默
  }
}

/** 账户 tab：默认停在活跃账户对应的形态，没有就离线 */
const accountTab = ref<"offline" | "authlib" | "ms">("offline");

onMounted(async () => {
  await refreshAuthStatus();
  const kind = activeAccount.value?.kind;
  if (kind === "authlib") accountTab.value = "authlib";
  if (kind === "microsoft") accountTab.value = "ms";
});

// KeepAlive 回来后账户可能被「设置」页或后端续期改过，刷新一次 chips
let firstActivate = true;
onActivated(() => {
  if (firstActivate) {
    firstActivate = false;
    return;
  }
  void refreshAuthStatus();
});

/** 当前活跃的登录账户（null = 离线模式） */
const activeAccount = computed(() => {
  const status = auth.value;
  if (!status) return null;
  return status.accounts[status.activeIndex ?? -1] ?? null;
});

const authLoggedIn = computed(() => Boolean(activeAccount.value?.loggedIn));

/** 启动卡的身份角标 */
const accountLabel = computed(() => {
  if (!authLoggedIn.value) return "离线模式";
  if (activeAccount.value?.kind === "microsoft") return "微软正版";
  return `皮肤站 · ${activeAccount.value?.serverName || "已登录"}`;
});

/** 切换活跃账户（chips 点击）。`null` = 切回离线。 */
async function switchAccount(index: number | null) {
  if (busyAuth.value) return;
  busyAuth.value = true;
  // 上一个账户的多角色待选列表不能再留着：那是对着"表单区"的，
  // 切换后点它会去改别的账户的会话（后端会拦，但 UI 先不该给）
  pendingProfiles.value = [];
  try {
    auth.value = await api.authSetActive(index);
    const active = activeAccount.value;
    const name =
      index === null
        ? "离线模式"
        : active?.profileName || active?.loginEmail || "未选角色的账户";
    notify(`已切换到 ${name}`, "info");
    await refreshAvatar();
    const kind = activeAccount.value?.kind;
    if (kind === "authlib") accountTab.value = "authlib";
    else if (kind === "microsoft") accountTab.value = "ms";
  } catch (e) {
    notify(errorText(e), "error");
  } finally {
    busyAuth.value = false;
  }
}

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
  // 取消还在等待确认的设备码流程：否则玩家稍后在浏览器里点了确认，
  // 微软账户会静默覆盖刚刚登录的皮肤站账户
  msLoopId++;
  msDevice.value = null;
  busyAuth.value = true;
  try {
    const result = await api.authLogin(
      activeServerUrl.value,
      email.value.trim(),
      password.value,
    );
    await refreshAuthStatus();
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
    // 后端"无角色"分支是先落盘、再报 Err——账户其实已经进了列表，
    // 不刷新的话 chips 和身份角标还停留在旧状态
    await refreshAuthStatus();
  } finally {
    busyAuth.value = false;
  }
}

async function pickProfile(profile: AuthProfileChoice) {
  busyAuth.value = true;
  try {
    await api.authSelectProfile(profile.id, profile.name);
    await refreshAuthStatus();
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
    await refreshAuthStatus();
    pendingProfiles.value = [];
    msLoopId++;
    msDevice.value = null;
    notify(
      auth.value?.accounts.length
        ? "已退出这个账户，已切到列表里的下一个"
        : "已退出登录，回到离线模式",
      "info",
    );
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
  const myLoop = ++msLoopId;
  try {
    const device = await api.authMsBegin();
    // 等待设备码期间用户可能已经取消或重开
    if (myLoop !== msLoopId) return;
    msDevice.value = device;
    void api.openInBrowser(device.verificationUri).catch(() => {
      // 打不开浏览器就让玩家手动复制链接，不拦流程
    });
    notify(`请在弹出的页面输入代码 ${device.userCode}`, "info");
    void msPollLoop(device, myLoop);
  } catch (e) {
    if (myLoop === msLoopId) notify(errorText(e), "error");
  } finally {
    busyAuth.value = false;
  }
}

/** 按后端给的间隔轮询，直到授权完成、过期或被取代 */
async function msPollLoop(device: MsDeviceCode, myLoop: number) {
  const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
  let interval = device.interval;
  // 设备码寿命用后端返回的 expires_in，不自己拍 15 分钟
  const deadline = Date.now() + (device.expiresIn || 900) * 1000;
  let netFails = 0;
  while (myLoop === msLoopId && Date.now() < deadline) {
    await sleep(interval * 1000);
    if (myLoop !== msLoopId) return;
    try {
      const result = await api.authMsPoll(device.deviceCode);
      netFails = 0;
      if (result.state === "slowDown") {
        interval += 5;
        continue;
      }
      if (result.state === "pending") continue;
      // done
      if (result.account) {
        await refreshAuthStatus();
        msDevice.value = null;
        notify(`微软正版登录成功：${result.account.profileName}`, "success");
        await refreshAvatar();
      }
      return;
    } catch (e) {
      const text = errorText(e);
      // 终态错误（过期/拒绝）立刻终止，不再走瞬断重试白等三轮
      if (text.includes("过期") || text.includes("拒绝")) {
        if (myLoop === msLoopId) {
          msDevice.value = null;
          notify(text, "error");
        }
        return;
      }
      // 网络瞬断不当场作废整个流程：连续 3 次失败才放弃（设备码通常还有效）
      netFails += 1;
      if (netFails >= 3) {
        if (myLoop === msLoopId) {
          msDevice.value = null;
          notify(text, "error");
        }
        return;
      }
    }
  }
  if (myLoop === msLoopId) {
    msDevice.value = null;
    notify("设备码过期了，请重新开始登录", "error");
  }
}

function msCancel() {
  msLoopId++;
  msDevice.value = null;
}

</script>

<template>
  <div class="flex h-full min-h-0 gap-4">
    <!--
      左：版本轨道。原来"选版本"要跳去版本管理页，现在选版本和启动在同一屏，
      管理/安装这类低频操作才跳页。
    -->
    <aside class="flex w-[248px] shrink-0 flex-col">
      <div class="card flex min-h-0 flex-1 flex-col p-3">
        <div class="mb-2 flex shrink-0 items-center justify-between px-1">
          <span class="text-xs font-bold text-ink-900">
            游戏版本
            <span class="font-normal text-ink-400">({{ state.local.length }})</span>
          </span>
          <button
            class="text-[11px] font-semibold text-brand-300 hover:text-brand-200"
            @click="state.view = 'versions'"
          >
            管理
          </button>
        </div>

        <div class="scroll-area min-h-0 flex-1 space-y-1 overflow-y-auto pr-1">
          <button
            v-for="version in state.local"
            :key="version.id"
            class="relative w-full rounded-xl px-3 py-2 text-left transition-[background-color,transform] duration-200 ease-spring active:scale-[0.98]"
            :class="
              version.id === state.selectedVersion
                ? 'bg-brand-500/[0.16]'
                : 'hover:bg-veil/[0.05]'
            "
            @click="state.selectedVersion = version.id"
          >
            <span
              class="absolute left-0 top-1/2 h-6 w-[3px] -translate-y-1/2 rounded-pill bg-brand-600 transition-opacity duration-200"
              :class="version.id === state.selectedVersion ? 'opacity-100' : 'opacity-0'"
            />
            <div class="flex items-center gap-1.5">
              <span
                class="min-w-0 flex-1 truncate text-xs font-bold"
                :class="version.id === state.selectedVersion ? 'text-brand-200' : 'text-ink-900'"
                :title="version.id"
              >
                {{ version.id }}
              </span>
              <span
                v-if="state.runningGames.some((g) => g.version === version.id)"
                class="h-1.5 w-1.5 shrink-0 rounded-full bg-emerald-500 animate-pulse-soft"
                title="这个版本正在运行"
              />
            </div>
            <div class="mt-0.5 flex items-center gap-1 truncate text-[10px] text-ink-400">
              <span>{{ version.complete ? version.kind : "未完成" }}</span>
              <span class="text-ink-300">·</span>
              <span>Java {{ version.javaMajor }}</span>
              <template v-if="version.modCount > 0">
                <span class="text-ink-300">·</span>
                <span>{{ version.modCount }} mod</span>
              </template>
            </div>
            <div v-if="playedAgo(version.id)" class="mt-0.5 truncate text-[10px] text-brand-300">
              {{ playedAgo(version.id) }}
            </div>
          </button>

          <div v-if="state.local.length === 0" class="px-2 py-8 text-center text-[11px] leading-relaxed text-ink-400">
            还没有版本<br />点下面的「安装」开始
          </div>
        </div>

        <div class="mt-2 flex shrink-0 gap-1.5">
          <button class="btn-outline flex-1 btn-sm" @click="state.view = 'versions'">
            <Icon name="download" :size="13" />
            安装
          </button>
          <button
            class="btn-ghost flex-1 btn-sm"
            :disabled="!state.selectedVersion"
            title="管理这个版本已安装的模组"
            @click="state.view = 'mods'"
          >
            <Icon name="mods" :size="13" />
            模组
          </button>
        </div>
      </div>
    </aside>

    <div class="flex min-w-0 flex-1 flex-col gap-4">
    <!-- 主启动卡：PCL2 的"启动"是页面上唯一的视觉焦点 -->
    <section
      class="relative shrink-0 overflow-hidden rounded-card bg-gradient-to-br from-brand-600 via-brand-600 to-brand-800 p-5 text-white shadow-[inset_0_1px_0_rgba(255,255,255,0.18),0_24px_60px_-30px_rgba(76,29,149,0.9)] ring-1 ring-white/10"
    >
      <div
        class="pointer-events-none absolute -right-16 -top-24 h-64 w-64 rounded-full bg-white/[0.12] blur-2xl"
      />
      <div
        class="pointer-events-none absolute -bottom-24 right-24 h-56 w-56 rounded-full bg-brand-300/25 blur-3xl"
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
            <span class="rounded-pill bg-white/15 px-3 py-1 ring-1 ring-white/10">
              内存 {{ memoryLabel }}
            </span>
            <span class="rounded-pill bg-white/15 px-3 py-1 ring-1 ring-white/10">
              {{ javaBadge }}
            </span>
            <span class="rounded-pill bg-white/15 px-3 py-1 ring-1 ring-white/10">
              {{ state.local.length }} 个本地版本
            </span>
          </div>
        </div>

        <div class="flex flex-col items-stretch gap-2">
          <!-- 游戏正在运行：启动位换成"结束游戏"，一眼能看清状态 -->
          <template v-if="runningGame">
            <div class="rounded-xl bg-white/[0.14] px-4 py-2.5 text-center text-xs font-bold ring-1 ring-white/10">
              游戏运行中 · 已 {{ runningMinutes(runningGame.startedAtMs) }}
            </div>
            <button
              class="btn min-w-[196px] gap-2.5 rounded-2xl bg-rose-500/90 px-6 py-3.5 text-base font-extrabold !text-white shadow-[0_18px_40px_-14px_rgba(120,10,40,0.8)] hover:-translate-y-0.5 hover:bg-rose-500 active:translate-y-0"
              @click="killGame(state.selectedVersion)"
            >
              <span class="grid h-8 w-8 place-items-center rounded-full bg-white/20">
                <Icon name="close" :size="15" />
              </span>
              结束游戏
            </button>
          </template>
          <button
            v-else
            class="group/launch btn min-w-[196px] gap-2.5 rounded-2xl bg-white px-6 py-3.5 text-base font-extrabold text-brand-700 shadow-[0_18px_40px_-14px_rgba(23,10,60,0.75)] hover:-translate-y-0.5 hover:shadow-[0_22px_46px_-14px_rgba(23,10,60,0.8)] active:translate-y-0 disabled:!translate-y-0 disabled:opacity-60"
            :disabled="!canLaunch"
            @click="launchSelected"
          >
            <span
              class="grid h-8 w-8 place-items-center rounded-full bg-brand-600 text-white transition-transform duration-300 ease-spring group-hover/launch:scale-110"
            >
              <Icon
                :name="state.launching ? 'refresh' : 'play'"
                :size="15"
                :class="state.launching ? 'animate-spin' : 'ml-0.5'"
              />
            </span>
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
    <div class="scroll-area flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto pr-1">
      <!-- 账户（离线 / 皮肤站）+ 皮肤 -->
      <!-- 注意：这卡绝不能带 overflow-hidden / min-h-0 —— 那会让网格把它的
           最小高度按 0 参与，行高被旁边较矮的「运行环境」卡拉平，内容被裁。
           行高必须由本卡内容撑起来；窗口太矮时由外层网格整体滚动。 -->
      <section class="card card-hover flex flex-col p-4">
        <!--
          摘要行：默认只占一行（头像 + 谁 + 身份 + 皮肤来源）。
          离线玩家 90% 的时间只需要看到这一行，表单点「切换 / 换肤」才展开，
          省下来的高度留给下面的运行环境和版本列表。
        -->
        <div class="flex items-center gap-3">
          <SkinAvatar :size="34" :model="state.avatar.model" :src="state.avatar.image ?? ''" />
          <div class="min-w-0 flex-1">
            <div class="truncate text-sm font-extrabold text-ink-900">
              {{ activeAccount?.profileName || state.settings.username || "Player" }}
            </div>
            <div class="mt-0.5 truncate text-[10px] text-ink-400">
              {{ accountLabel }} · {{ skinBadge }}
            </div>
          </div>
          <button class="btn-ghost shrink-0 btn-sm" @click="accountExpanded = !accountExpanded">
            {{ accountExpanded ? "收起" : "切换 / 换肤" }}
          </button>
        </div>

        <div v-if="accountExpanded" class="mt-3 flex min-h-0 flex-1 flex-col">
        <!-- 多账户切换：有登录账户时才出现；离线也是一格 -->
        <div
          v-if="auth && auth.accounts.length > 0"
          class="mb-3 flex flex-wrap items-center gap-1.5"
        >
          <button
            class="flex items-center gap-1.5 rounded-pill px-3 py-1.5 text-[11px] font-bold transition-colors"
            :class="
              auth.activeIndex === null || auth.activeIndex === undefined
                ? 'bg-brand-600 text-white'
                : 'bg-surface-sunken text-ink-500 hover:text-ink-900'
            "
            :disabled="busyAuth"
            @click="switchAccount(null)"
          >
            <Icon name="home" :size="12" />
            离线 · {{ state.settings.username }}
          </button>
          <button
            v-for="(account, index) in auth.accounts"
            :key="`${account.kind}-${account.profileId}`"
            class="flex items-center gap-1.5 rounded-pill px-3 py-1.5 text-[11px] font-bold transition-colors"
            :class="
              auth.activeIndex === index
                ? 'bg-brand-600 text-white'
                : 'bg-surface-sunken text-ink-500 hover:text-ink-900'
            "
            :title="
              account.kind === 'microsoft'
                ? '微软正版'
                : `${account.serverName}（${account.loginEmail}）`
            "
            :disabled="busyAuth"
            @click="switchAccount(index)"
          >
            <span
              class="h-1.5 w-1.5 rounded-full"
              :class="account.loggedIn ? 'bg-emerald-400' : 'bg-amber-400'"
            />
            {{ account.profileName }}
          </button>
        </div>

        <!-- 模式切换 -->
        <div class="mb-4 flex gap-2 rounded-xl bg-veil/[0.04] p-1">
          <button
            class="flex-1 rounded-lg py-1.5 text-xs font-bold transition-[background-color,color,transform] duration-200 ease-spring active:scale-[0.97]"
            :class="
              accountTab === 'offline'
                ? 'bg-brand-600 text-white shadow-[0_6px_18px_-8px_rgba(124,58,237,0.9)]'
                : 'text-ink-500 hover:text-ink-900'
            "
            @click="accountTab = 'offline'"
          >
            离线模式
          </button>
          <button
            class="flex-1 rounded-lg py-1.5 text-xs font-bold transition-[background-color,color,transform] duration-200 ease-spring active:scale-[0.97]"
            :class="
              accountTab === 'authlib'
                ? 'bg-brand-600 text-white shadow-[0_6px_18px_-8px_rgba(124,58,237,0.9)]'
                : 'text-ink-500 hover:text-ink-900'
            "
            @click="accountTab = 'authlib'"
          >
            皮肤站登录
          </button>
          <button
            class="flex-1 rounded-lg py-1.5 text-xs font-bold transition-[background-color,color,transform] duration-200 ease-spring active:scale-[0.97]"
            :class="
              accountTab === 'ms'
                ? 'bg-brand-600 text-white shadow-[0_6px_18px_-8px_rgba(124,58,237,0.9)]'
                : 'text-ink-500 hover:text-ink-900'
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
          <div v-if="authLoggedIn && activeAccount?.kind === 'authlib'" class="flex items-start gap-4">
            <SkinAvatar
              :size="64"
              :model="state.avatar.model"
              :src="state.avatar.image ?? ''"
            />
            <div class="min-w-0 flex-1">
              <div class="truncate text-lg font-extrabold text-ink-900">
                {{ activeAccount?.profileName }}
              </div>
              <div class="mt-0.5 text-[11px] text-ink-400">
                {{ activeAccount?.serverName }}（{{ activeAccount?.loginEmail }}）
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
                class="flex w-full items-center gap-2 rounded-xl border border-line bg-veil/[0.02] px-3 py-2 text-left text-[13px] font-bold text-ink-900 transition hover:border-brand-500/50 hover:bg-brand-500/[0.12]"
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
          <div v-if="authLoggedIn && activeAccount?.kind === 'microsoft'" class="flex items-start gap-4">
            <SkinAvatar
              :size="64"
              :model="state.avatar.model"
              :src="state.avatar.image ?? ''"
            />
            <div class="min-w-0 flex-1">
              <div class="truncate text-lg font-extrabold text-ink-900">
                {{ activeAccount?.profileName }}
              </div>
              <div class="mt-0.5 text-[11px] text-ink-400">
                微软正版 · {{ activeAccount?.loginEmail || "Minecraft 账号" }}
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
                <div class="mt-1 font-mono text-2xl font-black tracking-[0.3em] text-ink-900">
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
        </div>
      </section>

      <!--
        运行环境：Java + 内存合成一张卡、左右两栏。
        这两块内容都短，各占一张卡会留出大片空白；并排后一屏能同时看到
        "缺不缺 Java / 给多少内存 / 用哪个版本"。
      -->
      <section class="card card-hover p-4">
        <div class="mb-3 flex flex-wrap items-center justify-between gap-2">
          <div class="flex items-center gap-2 text-sm font-bold text-ink-900">
            <Icon name="cpu" :size="18" class="text-brand-300" />
            运行环境
          </div>
          <div class="flex items-center gap-2">
            <span v-if="!autoRam" class="chip bg-brand-500/[0.16] text-brand-200">
              内存 {{ memoryLabel }}
            </span>
            <button
              class="text-[11px] font-semibold text-brand-300 hover:text-brand-200"
              @click="refreshJava(true)"
            >
              重新检测 Java
            </button>
          </div>
        </div>

        <div class="grid gap-y-3 lg:grid-cols-2 lg:gap-x-5">
          <!-- Java -->
          <div class="min-w-0">
            <div
              v-if="state.java && !javaNeedsInstall"
              class="mb-2 flex min-w-0 items-center gap-2"
              :class="javaTone"
            >
              <span class="h-1.5 w-1.5 shrink-0 rounded-full bg-current" />
              <span class="shrink-0 text-xs font-bold">Java {{ state.java.major }}</span>
              <span class="min-w-0 truncate text-[10px] font-medium text-ink-400" :title="state.java.path">
                {{ state.java.source }} · {{ state.java.path }}
              </span>
            </div>
            <div
              v-else-if="javaNeedsInstall && state.java"
              class="mb-2 rounded-lg bg-amber-500/[0.14] px-3 py-2 text-[11px] font-semibold text-amber-300"
            >
              当前 Java {{ state.java.major }}，这个版本需要 Java {{ neededJavaMajor }}
            </div>
            <div
              v-else
              class="mb-2 rounded-lg bg-amber-500/[0.14] px-3 py-2 text-[11px] font-semibold text-amber-300"
            >
              未检测到 Java{{ neededJavaMajor > 0 ? `，这个版本需要 Java ${neededJavaMajor}` : "" }}
            </div>

            <div class="flex gap-2">
              <input
                v-model="state.settings.javaPath"
                class="field min-w-0 flex-1 font-mono !text-[11px]"
                placeholder="留空自动探测，可填 java.exe 路径"
              />
              <button
                v-if="javaNeedsInstall"
                class="btn-primary shrink-0 btn-sm"
                :disabled="state.downloadingJava"
                @click="downloadJavaRuntime(neededJavaMajor > 0 ? neededJavaMajor : 21)"
              >
                <Icon
                  :name="state.downloadingJava ? 'refresh' : 'download'"
                  :size="13"
                  :class="state.downloadingJava ? 'animate-spin' : ''"
                />
                {{ state.downloadingJava ? "下载中…" : `装 Java ${neededJavaMajor > 0 ? neededJavaMajor : 21}` }}
              </button>
            </div>
          </div>

          <!-- 内存 -->
          <div class="min-w-0 border-t border-line pt-3.5 lg:border-l lg:border-t-0 lg:pl-5">
            <template v-if="autoRam">
              <div class="flex flex-wrap items-baseline gap-x-2">
                <span class="text-lg font-extrabold text-ink-900">{{ recommendLabel }}</span>
                <span class="text-[11px] font-semibold text-ink-400">建议值</span>
                <span class="ml-auto text-[10px] text-ink-400">可用 {{ availableLabel }}</span>
              </div>
              <p class="mt-1 text-[11px] leading-relaxed text-ink-400">
                按 {{ state.memory?.plan.reason || "原版" }} 与当前可用内存推算。
              </p>
              <button
                class="mt-1.5 text-[11px] font-semibold text-brand-300 hover:text-brand-200"
                title="改成手动指定内存"
                @click="switchToManual"
              >
                手动指定内存 →
              </button>
            </template>

            <template v-else>
              <input
                v-model.number="state.settings.maxMemoryMb"
                type="range"
                min="1024"
                :max="sliderMax"
                step="512"
                class="w-full accent-brand-600"
              />
              <div class="mt-1 flex justify-between text-[10px] font-medium text-ink-400">
                <span>1 GB</span>
                <span>{{ Math.round(sliderMax / 2048) }} GB</span>
                <span>{{ Math.round(sliderMax / 1024) }} GB</span>
              </div>
              <div class="mt-2 flex gap-2">
                <button
                  v-for="preset in [2048, 4096, 8192]"
                  :key="preset"
                  class="flex-1 rounded-lg py-1.5 text-[11px] font-bold transition-colors duration-200"
                  :class="
                    state.settings.maxMemoryMb === preset
                      ? 'bg-brand-600 text-white'
                      : 'bg-surface-sunken text-ink-500 hover:bg-veil/[0.05] hover:text-brand-200'
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
              class="mt-2 rounded-lg bg-rose-500/[0.14] px-3 py-2 text-[11px] font-semibold text-rose-300"
            >
              已超过物理内存的 70%（{{ safeLimitLabel }}），游戏容易被系统换页拖到卡死
            </p>
            <p v-else-if="state.memory?.gcArgs.length" class="mt-2 text-[10px] text-ink-400">
              GC：{{ gcModeLabel }}
            </p>
          </div>
        </div>
      </section>

      <!-- 快捷操作：这些入口原来都埋在别的页面的二级菜单里，摆到首页一行 -->
      <section class="card flex flex-wrap items-center gap-1.5 px-3 py-2">
        <button class="btn-ghost btn-sm" @click="openPath('game')">
          <Icon name="folder" :size="13" />
          实例目录
        </button>
        <button class="btn-ghost btn-sm" @click="openPath('versions')">
          <Icon name="versions" :size="13" />
          版本目录
        </button>
        <button class="btn-ghost btn-sm" :disabled="!state.selectedVersion" @click="state.view = 'mods'">
          <Icon name="mods" :size="13" />
          模组
        </button>
        <button class="btn-ghost btn-sm" @click="state.view = 'netplay'">
          <Icon name="netplay" :size="13" />
          联机
        </button>
        <button class="btn-ghost btn-sm" @click="state.view = 'versions'">
          <Icon name="download" :size="13" />
          安装 / 备份
        </button>
        <span class="ml-auto truncate text-[10px] text-ink-400">
          上次启动：{{ playedAgo(state.selectedVersion) || "还没启动过" }}
        </span>
      </section>
    </div>

    </div>
  </div>
</template>
