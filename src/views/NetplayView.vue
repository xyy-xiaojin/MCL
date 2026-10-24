<script setup lang="ts">
/**
 * 联机页。
 *
 * 三条通路对应三种网络处境，页面上并排放，玩家不用自己判断该用哪个：
 *
 * - **EasyTier 组网**：双方都装客户端，进同一个虚拟局域网。最稳，但要管理员权限。
 * - **端口映射**：只在房主这边开洞，朋友拿公网地址直连。不用装东西、不用管理员。
 * - **樱花 FRP**：流量过服务商中继。前两条都不通时的兜底。
 *
 * 三条互斥：启动一条会自动停掉其它所有通道（见后端 `Netplay::stop_all`），
 * 同时开会抢网络资源，而且出问题时没法判断是谁在捣乱。
 */
import { listen } from "@tauri-apps/api/event";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { computed, onMounted, onUnmounted, reactive, ref } from "vue";

import Icon from "../components/Icon.vue";
import { api, errorText } from "../api";
import { notify, state as app } from "../store";
import type {
  FrpTunnel,
  NetplayChannel,
  NetplaySlot,
  NetplayView,
} from "../types";

/** 每个通道的日志上限：frpc 连不上时会疯狂重试，不限会吃满内存 */
const MAX_LOGS = 260;

const tabs: { key: NetplayChannel; label: string; icon: string; hint: string }[] = [
  {
    key: "easytier",
    label: "EasyTier 组网",
    icon: "netplay",
    hint: "两边都装 MCL，进同一个房间，最稳",
  },
  {
    key: "portmap",
    label: "端口映射",
    icon: "server",
    hint: "只有房主开洞，朋友拿公网地址直连",
  },
  {
    key: "frp",
    label: "樱花 FRP",
    icon: "key",
    hint: "走中继，前面两条都不通时用",
  },
];

const tab = ref<NetplayChannel>("easytier");
const view = ref<NetplayView | null>(null);
const busy = reactive<Record<NetplayChannel, boolean>>({
  easytier: false,
  portmap: false,
  frp: false,
});
const logs = reactive<Record<NetplayChannel, string[]>>({
  easytier: [],
  portmap: [],
  frp: [],
});

/** 本地端口：MC「对局域网开放」时聊天栏给的那个数，不是固定的 25565 */
const localPort = ref(25565);
/** 输入框被清空时 v-model.number 会变成空串，发给后端前先收敛成合法端口或 0 */
const port = computed<number>(() => {
  const n = Math.floor(Number(localPort.value));
  return Number.isFinite(n) && n >= 1 && n <= 65535 ? n : 0;
});
const lanIp = ref("");
const portOpen = ref(false);

const room = reactive({
  name: "",
  password: "",
  host: true,
});
const useUpnp = ref(true);

const token = ref("");
const tunnels = ref<FrpTunnel[]>([]);
const pickedTunnel = ref<number | null>(null);

const slot = computed<NetplaySlot | null>(() => {
  const v = view.value;
  if (!v) return null;
  return v[tab.value];
});

const running = computed(() => slot.value?.state === "running");
const starting = computed(() => slot.value?.state === "starting");

const canStartEasytier = computed(
  () =>
    !!room.name.trim() &&
    room.password.trim().length >= 4 &&
    !busy.easytier &&
    !starting.value,
);

/** 朋友在同 WiFi 下能直接用这个地址连进来 */
const lanAddress = computed(() => {
  if (!lanIp.value) return "";
  return port.value > 0 ? `${lanIp.value}:${port.value}` : lanIp.value;
});

const activeTunnel = computed(
  () => tunnels.value.find((t) => t.id === pickedTunnel.value) ?? null,
);

let unlisten: UnlistenFn[] = [];
// listen 的 Promise 可能在组件卸载之后才 resolve：到时候直接解绑，
// 不然每快速切一次页就漏两个监听器，回调还在往已卸载组件的数据里写
let disposed = false;

onMounted(async () => {
  await refresh();
  await refreshLan();
  if (view.value?.hasToken) {
    void loadTunnels();
  }
  // 默认房间名带上玩家昵称，省得每次想
  room.name = `${app.settings.username || "player"}-room`;

  const listeners = await Promise.all([
    listen<{ channel: string; stream: string; line: string }>(
      "netplay://log",
      (event) => {
        const channel = event.payload.channel as NetplayChannel;
        if (!logs[channel]) return;
        logs[channel].push(event.payload.line);
        if (logs[channel].length > MAX_LOGS) {
          logs[channel].splice(0, logs[channel].length - MAX_LOGS);
        }
      },
    ),
    listen<{ channel: NetplayChannel; slot: NetplaySlot }>(
      "netplay://status",
      (event) => {
        if (!view.value) return;
        view.value[event.payload.channel] = event.payload.slot;
      },
    ),
  ]);
  if (disposed) {
    listeners.forEach((fn) => fn());
    return;
  }
  unlisten = listeners;
});

onUnmounted(() => {
  disposed = true;
  unlisten.forEach((fn) => fn());
});

async function refresh() {
  try {
    view.value = await api.netplayStatus();
  } catch (e) {
    notify(errorText(e), "error");
  }
}

async function refreshLan() {
  try {
    const info = await api.netplayLanInfo(port.value);
    lanIp.value = info.lanIp;
    portOpen.value = info.portOpen;
  } catch {
    // 拿不到局域网地址不影响联机，静默
  }
}

/** 端口没填好就不发请求：后端要的是 u16，空串只会换来一句不可读的反序列化错误 */
function ensurePort(): boolean {
  if (port.value > 0) return true;
  notify("先填游戏端口（1–65535）", "error");
  return false;
}

async function loadTunnels() {
  try {
    tunnels.value = await api.netplayFrpLogin();
    if (pickedTunnel.value == null && tunnels.value.length) {
      pickedTunnel.value =
        view.value?.lastTunnel ?? tunnels.value[0].id;
    }
  } catch (e) {
    notify(errorText(e), "error");
  }
}

async function saveToken() {
  if (!token.value.trim()) return;
  busy.frp = true;
  try {
    tunnels.value = await api.netplayFrpSaveToken(token.value.trim());
    pickedTunnel.value =
      view.value?.lastTunnel ?? tunnels.value[0]?.id ?? null;
    token.value = "";
    await refresh();
    notify(
      tunnels.value.length
        ? `密钥有效，账号下 ${tunnels.value.length} 条隧道`
        : "密钥有效，但账号下还没有隧道",
      "success",
    );
  } catch (e) {
    notify(errorText(e), "error");
  } finally {
    busy.frp = false;
  }
}

async function startEasytier() {
  if (!ensurePort()) return;
  busy.easytier = true;
  try {
    await api.netplayEasytierStart({
      room: room.name.trim(),
      password: room.password.trim(),
      host: room.host,
      localPort: port.value,
    });
    await refresh();
  } catch (e) {
    notify(errorText(e), "error");
  } finally {
    busy.easytier = false;
  }
}

async function startPortmap() {
  if (!ensurePort()) return;
  busy.portmap = true;
  try {
    await api.netplayPortmapStart({
      localPort: port.value,
      useUpnp: useUpnp.value,
    });
    await refresh();
  } catch (e) {
    notify(errorText(e), "error");
  } finally {
    busy.portmap = false;
  }
}

async function startFrp() {
  if (!ensurePort()) return;
  if (pickedTunnel.value == null) {
    notify("先选一条隧道", "error");
    return;
  }
  busy.frp = true;
  try {
    await api.netplayFrpStart(pickedTunnel.value, port.value);
    await refresh();
  } catch (e) {
    notify(errorText(e), "error");
  } finally {
    busy.frp = false;
  }
}

async function stop(channel: NetplayChannel) {
  await api.netplayStop(channel);
  logs[channel].length = 0;
  await refresh();
}

async function relaunchAsAdmin() {
  const ok = await api.netplayRelaunchAdmin();
  if (ok) {
    notify("正在以管理员身份重启，请在弹出的提示里点「是」", "info");
  } else {
    notify("提权被取消，可以右键 MCL.exe 手动以管理员身份运行", "error");
  }
}

async function copy(text: string) {
  if (!text) return;
  try {
    await navigator.clipboard.writeText(text);
    notify("地址已复制，发给朋友就能进", "success");
  } catch {
    notify("复制失败，手动选中吧", "error");
  }
}

const dotClass = (s: NetplaySlot | undefined) => {
  switch (s?.state) {
    case "running":
      return "bg-emerald-400 shadow-[0_0_8px_rgba(52,211,153,0.8)]";
    case "starting":
      return "bg-amber-400 animate-pulse";
    case "error":
      return "bg-rose-400";
    default:
      return "bg-white/20";
  }
};
</script>

<template>
  <div class="flex h-full min-h-0 flex-col">
    <div class="min-h-0 flex-1 space-y-4 overflow-y-auto pr-1">
    <!-- 前置条件：缺管理员 / 缺组件 -->
    <div
      v-if="view && !view.isAdmin"
      class="flex items-start gap-3 rounded-2xl border border-amber-500/25 bg-amber-500/[0.08] px-4 py-3"
    >
      <Icon name="shield" :size="18" class="mt-0.5 text-amber-300" />
      <div class="min-w-0 flex-1 text-[13px] leading-relaxed text-amber-100/90">
        <span class="font-bold">EasyTier 需要管理员权限</span>
        ——它要装一块虚拟网卡。端口映射和樱花 FRP 不受影响，现在就能用。
      </div>
      <button class="btn-outline !py-1.5 !text-xs" @click="relaunchAsAdmin">
        以管理员身份重启
      </button>
    </div>

    <div
      v-if="view && view.missing.length"
      class="flex items-start gap-3 rounded-2xl border border-rose-500/25 bg-rose-500/[0.08] px-4 py-3"
    >
      <Icon name="close" :size="18" class="mt-0.5 text-rose-300" />
      <div class="text-[13px] leading-relaxed text-rose-100/90">
        <span class="font-bold">联机组件缺失：</span>
        {{ view.missing.join("、") }} —— 请用完整的 MCL 压缩包重新解压，别只拷 exe。
      </div>
    </div>

    <!-- 通路切换 -->
    <div class="grid grid-cols-3 gap-2">
      <button
        v-for="item in tabs"
        :key="item.key"
        class="group relative overflow-hidden rounded-2xl border px-4 py-3 text-left transition-all duration-200 ease-smooth"
        :class="
          tab === item.key
            ? 'border-brand-500/45 bg-brand-500/[0.12]'
            : 'border-line bg-surface hover:border-white/15 hover:bg-white/[0.04]'
        "
        @click="tab = item.key"
      >
        <div class="flex items-center gap-2">
          <Icon
            :name="item.icon"
            :size="17"
            :class="tab === item.key ? 'text-brand-300' : 'text-ink-400'"
          />
          <span
            class="text-[13px] font-extrabold"
            :class="tab === item.key ? 'text-white' : 'text-ink-700'"
          >
            {{ item.label }}
          </span>
          <span class="ml-auto flex items-center gap-1.5">
            <span
              class="h-1.5 w-1.5 rounded-full"
              :class="dotClass(view?.[item.key])"
            />
            <span
              class="text-[10px] font-semibold"
              :class="
                view?.[item.key]?.state === 'running'
                  ? 'text-emerald-300'
                  : 'text-ink-300'
              "
            >
              {{ view?.[item.key]?.state === "running" ? "运行中" : "" }}
            </span>
          </span>
        </div>
        <p class="mt-1.5 text-[11px] leading-snug text-ink-400">
          {{ item.hint }}
        </p>
      </button>
    </div>

    <div class="grid grid-cols-[minmax(0,1fr)_360px] items-start gap-4">
      <!-- 左：配置 -->
      <div class="card p-5">
        <!-- 公共：本地端口 -->
        <div class="mb-4 flex items-end gap-3">
          <div class="w-40">
            <label class="label">游戏端口</label>
            <input
              v-model.number="localPort"
              class="field font-mono"
              type="number"
              min="1"
              max="65535"
              @change="refreshLan"
            />
          </div>
          <div class="pb-2.5 text-[11px] leading-relaxed text-ink-400">
            游戏里「对局域网开放」后，聊天栏会显示
            <span class="font-mono text-ink-700">本地游戏端口 54321</span>
            ，填那个数。<br />
            <span
              class="mt-1 inline-flex items-center gap-1.5"
              :class="portOpen ? 'text-emerald-400' : 'text-ink-300'"
            >
              <span
                class="h-1.5 w-1.5 rounded-full"
                :class="portOpen ? 'bg-emerald-400' : 'bg-white/20'"
              />
              {{ portOpen ? `${localPort} 端口有人监听，可以开` : "这个端口现在没人监听" }}
            </span>
            <span v-if="lanAddress" class="ml-2 text-ink-300">
              · 同 WiFi 直连 <span class="font-mono">{{ lanAddress }}</span>
            </span>
          </div>
        </div>

        <!-- EasyTier -->
        <div v-if="tab === 'easytier'" class="space-y-4">
          <div class="flex gap-2 rounded-xl bg-white/[0.04] p-1">
            <button
              class="flex-1 rounded-lg py-2 text-xs font-bold transition-all"
              :class="
                room.host
                  ? 'bg-brand-600 text-white shadow-[0_6px_18px_-8px_rgba(124,58,237,0.9)]'
                  : 'text-ink-500 hover:text-white'
              "
              @click="room.host = true"
            >
              我开房间（房主）
            </button>
            <button
              class="flex-1 rounded-lg py-2 text-xs font-bold transition-all"
              :class="
                !room.host
                  ? 'bg-brand-600 text-white shadow-[0_6px_18px_-8px_rgba(124,58,237,0.9)]'
                  : 'text-ink-500 hover:text-white'
              "
              @click="room.host = false"
            >
              我加入朋友
            </button>
          </div>

          <div class="grid grid-cols-2 gap-3">
            <div>
              <label class="label">房间名</label>
              <input
                v-model="room.name"
                class="field"
                placeholder="例如 xiaoming-room"
                maxlength="32"
                :disabled="running"
              />
            </div>
            <div>
              <label class="label">房间密码</label>
              <input
                v-model="room.password"
                class="field"
                placeholder="至少 4 位"
                :disabled="running"
              />
            </div>
          </div>

          <p class="rounded-xl bg-white/[0.03] px-3.5 py-2.5 text-[11px] leading-relaxed text-ink-400">
            <template v-if="room.host">
              房间建好后，把<span class="font-semibold text-ink-700">房间名 + 密码</span>发给朋友，
              他在游戏里填 <span class="font-mono text-brand-300">10.144.1.1:{{ localPort }}</span> 就能进。
            </template>
            <template v-else>
              填朋友给的房间名和密码。进房后你会拿到一个
              <span class="font-mono">10.144.1.x</span> 的地址，游戏里连房主的
              <span class="font-mono text-brand-300">10.144.1.1:{{ localPort }}</span>。
            </template>
          </p>

          <div class="flex items-center gap-2">
            <button
              v-if="!running"
              class="btn-primary"
              :disabled="!canStartEasytier || (!!view && !view.isAdmin)"
              @click="startEasytier"
            >
              <Icon name="play" :size="15" filled />
              {{ starting ? "正在组网…" : busy.easytier ? "处理中…" : room.host ? "创建房间" : "加入房间" }}
            </button>
            <button v-else class="btn-ghost" @click="stop('easytier')">
              <Icon name="close" :size="15" />
              断开房间
            </button>
            <span
              v-if="view && !view.isAdmin"
              class="text-[11px] text-amber-300/80"
            >
              需要管理员权限
            </span>
          </div>
        </div>

        <!-- 端口映射 -->
        <div v-else-if="tab === 'portmap'" class="space-y-4">
          <label class="flex cursor-pointer items-start gap-3 rounded-xl bg-white/[0.03] px-3.5 py-3">
            <input
              v-model="useUpnp"
              type="checkbox"
              class="mt-0.5 h-4 w-4 accent-[#7c3aed]"
              :disabled="running"
            />
            <span class="text-[12px] leading-relaxed text-ink-500">
              <span class="font-bold text-ink-700">让路由器自动开洞（UPnP）</span><br />
              大部分家用路由器支持。关掉的话只做探测，你需要自己去路由器里把端口转发到本机。
            </span>
          </label>

          <p class="rounded-xl bg-white/[0.03] px-3.5 py-2.5 text-[11px] leading-relaxed text-ink-400">
            开好之后会给你一个 <span class="font-mono text-brand-300">公网IP:端口</span>，
            朋友在游戏里「直接连接」填这个。你的网络如果是运营商级 NAT（对称型），这条路会失败，
            那就换 EasyTier 或樱花 FRP。
          </p>

          <div class="flex items-center gap-2">
            <button
              v-if="!running"
              class="btn-primary"
              :disabled="busy.portmap || starting"
              @click="startPortmap"
            >
              <Icon name="play" :size="15" filled />
              {{ starting ? "正在协商…" : busy.portmap ? "处理中…" : "开洞并取地址" }}
            </button>
            <button v-else class="btn-ghost" @click="stop('portmap')">
              <Icon name="close" :size="15" />
              关闭映射
            </button>
          </div>
        </div>

        <!-- 樱花 FRP -->
        <div v-else class="space-y-4">
          <div>
            <label class="label">访问密钥</label>
            <div class="flex gap-2">
              <input
                v-model="token"
                class="field font-mono"
                type="password"
                :placeholder="view?.hasToken ? '已保存，需要更换时再填' : '樱花面板 → 用户中心 → API 信息'"
                :disabled="running"
                @keyup.enter="saveToken"
              />
              <button
                class="btn-outline shrink-0"
                :disabled="!token.trim() || busy.frp"
                @click="saveToken"
              >
                {{ busy.frp ? "验证中…" : "验证" }}
              </button>
            </div>
          </div>

          <div v-if="view?.hasToken">
            <div class="mb-1.5 flex items-center justify-between">
              <label class="label !mb-0">隧道</label>
              <button
                class="text-[11px] font-semibold text-brand-300 hover:text-brand-200"
                @click="loadTunnels"
              >
                刷新列表
              </button>
            </div>
            <div v-if="tunnels.length" class="max-h-52 space-y-1.5 overflow-y-auto pr-1">
              <button
                v-for="t in tunnels"
                :key="t.id"
                class="flex w-full items-center gap-3 rounded-xl border px-3 py-2 text-left transition-all"
                :class="
                  pickedTunnel === t.id
                    ? 'border-brand-500/50 bg-brand-500/[0.12]'
                    : 'border-line bg-white/[0.02] hover:border-white/15'
                "
                :disabled="t.banned || running"
                @click="pickedTunnel = t.id"
              >
                <div class="min-w-0 flex-1">
                  <div class="flex items-center gap-2">
                    <span class="truncate text-[13px] font-bold text-ink-900">
                      {{ t.name }}
                    </span>
                    <span class="chip-mute !px-1.5 !py-0 !text-[10px]">
                      {{ t.kind.toUpperCase() }}
                    </span>
                    <span v-if="t.banned" class="chip-bad !px-1.5 !py-0 !text-[10px]">
                      封禁
                    </span>
                    <span
                      v-else-if="t.online"
                      class="chip-ok !px-1.5 !py-0 !text-[10px]"
                    >
                      在线
                    </span>
                  </div>
                  <div class="mt-0.5 truncate text-[11px] text-ink-400">
                    {{ t.remote || `#${t.id} · 节点 ${t.node}` }}
                  </div>
                </div>
              </button>
            </div>
            <p v-else class="rounded-xl bg-white/[0.03] px-3.5 py-3 text-[11px] text-ink-400">
              账号下还没有隧道。去樱花面板「服务 → 隧道列表」建一条
              <span class="font-semibold">TCP</span> 隧道再回来。
            </p>
          </div>

          <p class="rounded-xl bg-white/[0.03] px-3.5 py-2.5 text-[11px] leading-relaxed text-ink-400">
            隧道连通后，把右边显示的地址发给朋友。流量走樱花节点，
            免费节点有带宽上限，人多会卡。
          </p>

          <div class="flex items-center gap-2">
            <button
              v-if="!running"
              class="btn-primary"
              :disabled="busy.frp || starting || !view?.hasToken || !activeTunnel"
              @click="startFrp"
            >
              <Icon name="play" :size="15" filled />
              {{ starting ? "正在连节点…" : busy.frp ? "处理中…" : "启动隧道" }}
            </button>
            <button v-else class="btn-ghost" @click="stop('frp')">
              <Icon name="close" :size="15" />
              断开隧道
            </button>
          </div>
        </div>
      </div>

      <!-- 右：状态 -->
      <div class="card flex flex-col gap-3 p-5">
        <div class="flex items-center gap-2">
          <span class="h-2 w-2 rounded-full" :class="dotClass(slot ?? undefined)" />
          <span class="text-[13px] font-extrabold text-ink-900">
            {{ slot?.summary || "未启动" }}
          </span>
        </div>

        <!-- 可分享地址 -->
        <div
          v-if="slot?.address"
          class="rounded-xl border border-brand-500/25 bg-brand-500/[0.08] px-3.5 py-3"
        >
          <div class="text-[10px] font-bold tracking-wider text-brand-300/80">
            发给朋友的地址
          </div>
          <div class="mt-1 flex items-center gap-2">
            <code class="min-w-0 flex-1 truncate font-mono text-[15px] font-bold text-white">
              {{ slot.address }}
            </code>
            <button
              class="rounded-lg bg-white/10 p-1.5 text-ink-700 transition hover:bg-white/20 hover:text-white"
              title="复制"
              @click="copy(slot.address)"
            >
              <Icon name="copy" :size="14" />
            </button>
          </div>
        </div>

        <!-- 详情 -->
        <dl v-if="slot?.facts.length" class="space-y-1.5">
          <div
            v-for="fact in slot.facts"
            :key="fact.label"
            class="flex items-start justify-between gap-3 border-b border-white/[0.04] pb-1.5 last:border-0 last:pb-0"
          >
            <dt class="shrink-0 text-[11px] font-semibold text-ink-400">
              {{ fact.label }}
            </dt>
            <dd
              class="break-all text-right font-mono text-[11px] text-ink-700"
            >
              {{ fact.value }}
            </dd>
          </div>
        </dl>

        <!-- 错误 -->
        <div
          v-if="slot?.error"
          class="rounded-xl border border-rose-500/25 bg-rose-500/[0.08] px-3 py-2.5 text-[11px] leading-relaxed text-rose-200"
        >
          {{ slot.error }}
        </div>

        <p
          v-if="slot?.state === 'idle'"
          class="text-[11px] leading-relaxed text-ink-400"
        >
          还没开始。左边填好参数点启动，状态会实时出现在这里。
        </p>
      </div>
    </div>

    <!-- 日志 -->
    <div class="card p-4">
      <div class="mb-2 flex items-center justify-between">
        <div class="flex items-center gap-2">
          <Icon name="server" :size="14" class="text-ink-400" />
          <span class="text-xs font-bold text-ink-700">运行日志</span>
          <span class="chip-mute !px-1.5 !py-0 !text-[10px]">
            {{ tabs.find((t) => t.key === tab)?.label }}
          </span>
        </div>
        <button
          class="text-[11px] font-semibold text-ink-400 hover:text-ink-700"
          @click="logs[tab].length = 0"
        >
          清空
        </button>
      </div>
      <div
        class="max-h-56 overflow-y-auto rounded-xl bg-black/35 p-3 font-mono text-[11px] leading-[1.65] text-ink-500"
      >
        <div v-if="!logs[tab].length" class="text-ink-300">
          启动后这里会显示输出的每一行，连不上时先看它。
        </div>
        <div
          v-for="(line, i) in logs[tab]"
          :key="i"
          class="whitespace-pre-wrap break-all"
        >
          {{ line }}
        </div>
      </div>
    </div>
    </div>
  </div>
</template>
