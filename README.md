# MCL · Minecraft 启动器 0.03

一个 **PCL2 布局 + 现代卡片质感** 的 Minecraft 启动器，当前版本 **0.03**（语义化版本 `0.0.3`），
相比 0.02 主要补齐了**小白可独立使用**的短板：Java 自动下载、游戏日志落盘 + 崩溃初判、
重复启动防护 + 结束游戏、磁盘空间预检（发布说明见 [RELEASE-0.03.md](./RELEASE-0.03.md)）。
前端只做渲染与交互，**下载、文件 IO、哈希校验、启动游戏全部在 Rust 侧**。

> 关于「Theseus 库」这件事有一节必须说明的结论，见 [THESEUS.md](./THESEUS.md)。

> **想直接跑**：双击项目根目录的 `MCL.exe`（或 `release/MCL-0.02-netplay-win64/MCL.exe`，两者同源）。
> 前端已嵌进二进制，不需要 Node 和 dev server。
> **注意要从整个文件夹运行**——联机组件在同目录的 `resources/netplay/` 下，单独拷 exe 会没有联机。
> 发布说明 / 已知限制见 [RELEASE-0.02.md](./RELEASE-0.02.md)。
>
> 根目录的 `MCL.exe` 与 `resources/` 都是从 `src-tauri/target/release/` 和 `src-tauri/resources/`
> **复制**过来的运行副本（不进 git），重新构建后要再同步一次：
> `cp src-tauri/target/release/mcl-tauri.exe MCL.exe && cp -r src-tauri/resources/netplay resources/`

## 技术栈

| 层 | 选型 |
|---|---|
| 外壳 | Tauri 2（Rust + 系统 WebView） |
| 前端 | Vue 3 `<script setup>` + Vite 6 + TypeScript + TailwindCSS 3 |
| 后端 | Rust：`reqwest`（rustls）、`tokio`、`serde`、`sha1`、`zip` |
| 通信 | IPC `invoke` 调命令 + Tauri Event 推进度/日志 |

## 运行

前置：Node ≥ 18、Rust ≥ 1.77、Windows 需 WebView2（Win11 自带）。

```bash
npm install
npm run dev            # 只跑前端（调样式用，IPC 会失败）
cargo build            # 只编译后端，窗口里的前端走 devUrl，需要配合 vite
```

### 做出一个能双击运行的 exe

`cargo build` 默认不把前端打进二进制——`tauri` 的 `dev` 判定是 `dev = !feature("custom-protocol")`，
不带这个 feature 时窗口会去连 `http://localhost:1420`，没有 dev server 就是一片白。
所以打包必须显式带上：

```bash
npm run build                                           # 产出 dist/
cargo build --features custom-protocol -j 1             # 产物：src-tauri/target/debug/mcl-tauri.exe
```

验证是否打进去了：`grep -a -c "index-" src-tauri/target/debug/mcl-tauri.exe` 应能命中
`dist/assets/` 里的文件名；命中为空说明还在走 devUrl。

> **窗口能开但内容纯黑**：先看 `%LOCALAPPDATA%\com.mcl.launcher\EBWebView\chrome_debug.log`
> （启动时带 `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--enable-logging --log-level=0` 才会写）。
> 如果里面反复刷 `GPU process exited unexpectedly: exit_code=7`，最后一行是
> `FATAL:content/browser/gpu/gpu_data_manager_impl_private.cc GPU process isn't usable. Goodbye.`，
> 那就跟前端、跟前端资源打包都没关系——WebView2 的 GPU 子进程根本起不来
> （本机是被应用控制策略掐掉的），Chromium 重试 9 次后直接结束浏览器进程，窗口只剩底色。
>
> 现在 `tauri.conf.json` 的窗口配置里钉了：
> `"additionalBrowserArgs": "--disable-gpu-sandbox --disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection"`。
> 保留硬件加速，只去掉 GPU 子进程的沙箱。注意这个字段会**替换** wry 的默认参数
> （默认值就是那段 `--disable-features=...`），所以必须一起写，否则 PDF / SmartScreen
> 这些内置 UI 会冒出来。
> 备选参数（都在本机实测能救回来）：`--in-process-gpu`、`--use-angle=swiftshader`（纯软渲染，最省心但慢）、
> `--no-sandbox`（仅诊断用，别发出去）。

正式安装包用 `npm run tauri build`（release + NSIS/MSI）。本机跑不通它：
Tauri CLI 的原生模块 `@tauri-apps/cli-win32-x64-msvc` 被应用控制策略拦住
（`ERR_DLOPEN_FAILED: An Application Control policy has blocked this file`），
要走安装包就得先解决这个拦截。

> **本机编译注意**：这台机器上的 Smart App Control 会随机拦截新编译的 build script
> （`os error 4551`，多试几轮能过）。用 `bash tools/build_retry.sh --features custom-protocol`
> 会自动删掉被拦的 `target/debug/build/<包>-<哈希>` 再重试；编译请加 `-j 1`，
> 并行编 `hyper` 会让 rustc 崩。**不要删 `target/`**——里面已放行的 build script 是宝贵资源。

### 在 Linux 上构建

Windows 上 release 出不来（SAC 稳定拦 `darling_macro`），Linux 没有这层机制，
`cargo build --release` 直接可用：

```bash
bash tools/build_linux.sh            # 出 ./MCL
bash tools/build_linux.sh --bundle   # 额外出 .deb
```

系统依赖（`libwebkit2gtk-4.1-dev` 等）、JDK、游戏运行时库、以及代码里已经
做好的平台分支，都写在 **[docs/BUILD-LINUX.md](./docs/BUILD-LINUX.md)**。
不能在 Windows 上交叉编译 Linux 版：Tauri 要链 GTK/WebKitGTK 的系统库。


## 目录

```
src/                      前端：只渲染、只调 IPC
├── App.vue               布局（左固定侧边栏 + 主区域 + 轻提示）
├── api.ts                IPC 封装（唯一的跨进程出口）
├── store.ts              轻量全局状态 + 事件订阅
├── types.ts              与 Rust serde 结构一一对应
├── components/           Icon（内联 SVG）、Sidebar、ProgressBar
└── views/                Home / Versions / Downloads / Modpacks / Mods / Settings

src-tauri/src/
├── lib.rs                Builder、事件、命令注册
├── commands.rs           IPC 命令层（只做参数转发）
├── state.rs              目录布局 + Settings + 全局状态
├── error.rs              统一错误（手动 Serialize 给前端）
└── engine/               启动核心（对标 Theseus 的职责）
    ├── net.rs            HTTP 客户端、镜像重写、失败归类与重试策略
    ├── meta.rs           版本清单 / version.json / rules / inheritsFrom 合并 / 资源索引
    ├── download.rs       分片并行（段直写）+ 断点续传 + 换源 + SHA-1 校验
    ├── queue.rs          全局下载队列（并发 10、暂停/继续/取消、进度事件）
    ├── install.rs        安装计划 → 入队 → 解压 natives
    ├── launch.rs         参数拼装（含占位符）+ 子进程 + 输出转发
    ├── memory.rs         可用内存探测 + PCL 式四段阶梯分配 + GC 参数
    ├── java.rs           Java 探测（JAVA_HOME / PATH / 常见目录）
    ├── browse.rs         Modrinth 分类搜索（热门/搜索）+ 一键入队
    ├── mods.rs           mod 列表与启停（改扩展名）
    ├── export.rs         实例导出（overrides + manifest → zip）
    ├── skin.rs           玩家头像（皮肤地址 / 缓存 / 离线皮肤导入 / 默认模型兜底）
    └── modpack.rs        Modrinth 搜索 + .mrpack / .zip 导入
```

应用图标：源图是根目录的 `app-icon.png`（1024×1024，白底黑月），
各平台尺寸由它生成，改图后重跑一次即可：

```bash
npx tauri icon app-icon.png
```

会写 `src-tauri/icons/` 下的 `32/64/128/128@2x/256/512` 与 `icon.ico`
（含 16/24/32/48/64/256 六档）、`icon.icns`、Windows 商店的 `Square*Logo`。
`tauri.conf.json` 的 `bundle.icon` 列的就是这几个，安装包和 exe 资源都取它。

## 界面

左侧侧边栏：**首页 / 版本管理 / 模组管理 / 联机 / 设置**，下面是下载分类
（**原版游戏 / Mod / 整合包 / 数据包 / 材质包 / 光影包**）——分类直接挂在一级导航，
点一下就进对应分类，没有独立的「下载」页。

视觉是深色科技感：冷黑紫底（`#0a0910`）+ 左上紫色光晕，面板用 `#131120` 配极细描边
和顶部高光，强调色是电光紫（`#8b5cf6`）。阴影靠"顶部高光 + 向外扩散的黑影"做层次，
`hover` 抬升 0.3s 缓动，选中项是左侧 3px 亮条 + 半透明紫色底。

| 页面 | 内容 |
|---|---|
| 首页 | 紫色渐变主卡（版本 + 内存 + Java）、启动按钮、离线账号 + 皮肤导入、内存滑块 |
| 版本管理 | 本地版本卡片（每张可启动 / 导出 zip / 删除）+ 「新增版本」在线安装面板（可搜索） |
| 联机 | 三条通路并排：EasyTier 组网 / 端口映射直连 / 樱花 FRP 隧道，各自一键启停 + 实时日志 |
| 下载 | 分类浏览：社区资源默认给热门，可按名称、游戏版本、加载器搜索；右上角队列胶囊 + 抽屉 |
| 整合包 | 已并入下载分类，支持在线搜索；本地包拖进窗口即导入（不设填路径的输入框） |
| 模组管理 | mod 列表 + 开关（`x.jar` ↔ `x.jar.disabled`）+「添加 mod」文件选择框 |
| 设置 | 下载源、主题、内存、Java 路径、目录结构 |

> 用 `?view=downloads&section=mod` 可以直接打开某个分类（截图和调试用）。

## 联机

三条通路，对应三种网络处境。页面上并排放，不用自己判断该用哪个：

| 通路 | 原理 | 谁需要装东西 | 适合 |
|---|---|---|---|
| **EasyTier 组网** | 双方各有客户端，建一层虚拟局域网（`10.144.1.0/24`） | **两边都要 MCL** | 长期联机，最稳，NAT 类型基本不影响 |
| **端口映射** | 房主这边用 UPnP 请路由器开洞，公网直连 | 只有房主 | 家里路由器支持 UPnP、不是对称 NAT |
| **樱花 FRP** | 流量过服务商节点中继 | 只有房主（要樱花账号） | 前两条都不通时兜底；免费节点有带宽上限 |

三条**互斥**：启动一条会自动停掉另外两条。同时开会抢网络资源，而且出问题时分不清是谁在捣乱。

### 端口怎么填

游戏里「对局域网开放」后，聊天栏会显示 `本地游戏端口 54321`——**填那个数**，
不是固定的 25565（25565 是专用服务端的端口）。填错了页面会提示"这个端口现在没人监听"。

### EasyTier

房主选「我开房间」，房间名 + 密码（≥4 位）发给朋友；朋友选「我加入朋友」填同样的两项。
房主固定拿 `10.144.1.1`，朋友在游戏里「直接连接」填 `10.144.1.1:<端口>`。

实现上有两个容易踩的点：

- **两端都必须带 `-i`**。EasyTier 的规则是"不给 IP 就只当转发节点、不建 TUN 设备"，
  而 MC 要连虚拟 IP 必须有那块网卡。房主固定 `.1`，加入者随机挑 `10.144.1.2~251`
  （不用 `-d` DHCP：它的默认网段是 `10.0.0.0/24`，和房主的 `10.144.1.x` 根本不通）。
- **必须管理员权限**。装 wintun 虚拟网卡没有管理员会**静默失败**——它连日志都来不及写就退了。
  所以前端在列表里就拦：非管理员时按钮禁用 + 顶栏给「以管理员身份重启」（`ShellExecuteW` + `runas`）。

另外 EasyTier 自带 STUN 探测，`easytier-cli -o json node` 里的 `stun_info`
顺手拿来显示公网出口和 NAT 类型，省一次独立探测。

### 端口映射（等价于 LinkStar 的 MC 联机能力）

没有内置 LinkStar 的 GUI 程序：它要登录自己的管理后台（JWT）才能改配置，
而且是个 27 MB 的 Wails 窗口，塞进启动器里既割裂又难管。这里按同样的原理自己实现：

1. `igd-next` 找 UPnP 网关，请它把公网端口转发到本机游戏端口（同端口优先，被占就 +1 试十个）；
2. 手写 20 字节 STUN Binding Request 问公网回显服务器"你看到的我是谁"，
   解析 `XOR-MAPPED-ADDRESS` 拿 `IP:端口`；
3. STUN 回的端口和本地绑定端口一致 → 锥形 NAT，这条路有戏；不一致 → 对称 NAT，直接劝换路。

UDP socket 特意绑在**游戏端口**上问 STUN：这样 NAT 记下来的映射端口和游戏端口是同一个，
后面的映射才对得上。

### 樱花 FRP

内置的是 SakuraFrp 分发的 frpc（`0.51.0-sakura-15`）——**不是**上游 fatedier/frp 的版本，
它带 `sakura_mode` 和 DKC 加密，用官方 frpc 连不上樱花节点。

配置不自己拼：调 `POST /v4/tunnel/config` 让服务端生成（`server_addr`、`sakura_mode`
这些字段写错一个就连不上），只把 `[[proxies]]` 段里的 `local_port` 改成玩家实际端口——
隧道是建的时候填的端口，而玩家每次开世界的端口可能不同。

访问密钥存在 `<数据目录>/netplay/frp.json`，不进 `settings.json`：
它是凭据，删版本、清缓存都不该把它带走。

### 组件从哪来

随包分发的四个文件（`src-tauri/resources/netplay/`），打包后落在安装目录的 `resources/netplay/`：

| 文件 | 来源 | 体积 |
|---|---|---|
| `easytier-core.exe` / `easytier-cli.exe` | [EasyTier](https://github.com/EasyTier/EasyTier) v2.6.4 Windows x86_64 | 34 MB |
| `wintun.dll` / `WinDivert64.sys` / `Packet.dll` | 同上（虚拟网卡与流量重定向，缺一不可） | 0.7 MB |
| `frpc.exe` | SakuraFrp 官方分发的 frpc `0.51.0-sakura-15` | 14 MB |

EasyTier 是 Apache-2.0，frpc 是 Apache-2.0；两者都是**独立进程**调用，不链接进 MCL。

## 玩家头像

左上角的头像和 PCL2 是同一套做法（`engine/skin.rs` + `components/SkinAvatar.vue`）：

1. **查地址**：`sessionserver.mojang.com/session/minecraft/profile/<uuid>` →
   `properties` 里 `name="textures"` 的 base64 → 解出 `textures.SKIN.url`；
2. **缓存**：索引在 `cache/skins/index.json`，贴图按 URL 的 SHA-1 存成
   `cache/skins/<hash>.png`（写 `.tmp` 再改名，避免半截文件）。
   缓存键是 URL 不是 UUID——换了皮肤 URL 就变，自然重下；
   成功缓存 6 小时，失败缓存 10 分钟（断网时不会每次刷新都等一次超时）；
3. **裁剪**：脸取 `(8,8)` 起 8×8，帽子/外层取 `(40,8)` 起 8×8 叠在上面。
   坐标按 64×64 网格算，所以 128/256 的 HD 皮肤同样正确；
4. **本地皮肤**：首页点「导入皮肤」挑一张 PNG（也可以直接把 PNG 拖进窗口），
   后端校验 PNG 签名和尺寸（64×64 / 64×32 及其整数倍，上限 1 MB）后
   存成 `<root>/skins/offline.png`，路径记在 `settings.skinPath`，离线账号优先用它。
   头像只裁脸和帽子层，宽 / 细手臂在头像上看不出差别，所以本地皮肤一律按
   Steve 标（`metadata.model` 是官方 textures 属性才有的信息，本地文件里没有）；
5. **兜底**：离线账号、没皮肤、请求失败 → 用内置 `assets/steve.png` / `alex.png`。
   这两张**不是手画的**，是从官方客户端 jar 里取的原图
   （`assets/minecraft/textures/entity/player/wide/steve.png` 与 `slim/alex.png`），
   所以放大后就是游戏里那个头。
   **默认模型固定是 Steve**：只有皮肤自己声明了 `metadata.model == "slim"`
   才会用 Alex。早期版本照抄 PCL 的 `McSkinSex`，按派生 UUID 的低半字节异或
   去"猜" Alex，导致一半离线账号顶着一张 Alex——那是个 bug，已去掉。
   > 皮肤贴图版权归 Mojang；这里只作为默认头像的兜底资源。

> 现在只有离线账号，所以实际看到的是导入的皮肤或内置 Steve；等接了微软登录后，
> 把 `uuid` 传给 `skin_avatar` 就会自动换成真实皮肤。
>
> **导入的皮肤只影响启动器头像**，不会写进 `game/`：游戏内换肤要服务端支持
> 或 `CustomSkinLoader` 这类 mod。导入界面上也写了这句话，免得玩家以为没生效。

## Forge / NeoForge

版本管理页每个已安装的版本上都有 **Forge / NeoForge 按钮**，点一下：

1. **原版没装好就先装原版**（installer 要读原版客户端 jar 并校验哈希）；
2. **挑版本**：Forge 从 BMCLAPI 的 `/forge/minecraft/<版本>` 拿构建列表（最新 build 优先），
   NeoForge 解析官方 maven 的 metadata（只支持 1.20.2+）；
3. **Java 不够就自动补**：installer 自己也是 Java 程序，检测到 Java 低于需求时
   直接触发上一节的自动下载——裸机走到这里也能一路装完；
4. **下载 installer**：BMCLAPI 镜像优先（302 到自家 maven），官方 maven 兜底，
   BMCLAPI 列表里的 SHA-1 会校验；走全局队列，进度可见；
5. **无头运行** `java -jar installer --installClient <目录>`，让官方安装器自己完成
   类库下载和客户端补丁——不自己重新实现安装器是刻意的：Forge 的处理器管线
   每个版本都可能变，跟着官方走才不会腐烂；
6. **归化版本目录**：对比 `versions/` 前后差异找出安装器新建的目录；
   整合包导入时会把它改名成我们自己的版本 id（目录 + 内部 jar/json + JSON 的 id 字段）。

启动无需任何特殊处理：installer 生成的版本 JSON 走 `inheritsFrom` 合并链，
客户端 jar、natives 沿链解析——这套逻辑 0.01 就有了。

> **实测记录**（本机 Linux + Java 25，Forge 1.20.1-47.4.28）：裸目录直接跑
> installer 会被拒绝——它要求目标目录里有 `launcher_profiles.json`
> （"There is no Minecraft launcher profile"），所以安装前会补一个桩文件
> （已存在绝不覆盖）。补桩后安装成功，产出 `versions/1.20.1-forge-47.4.28/`，
> 里面**只有版本 JSON、没有客户端 jar**——现代 Forge 靠 `inheritsFrom`
> 用回原版 jar，正好命中我们已有的沿链找 jar 逻辑，不需要任何适配。

**Modrinth 整合包**声明 `forge` / `neoforge` 依赖时也走同一条路（.mrpack 的
依赖版本号支持 `47.2.0` 和 `1.20.1-47.2.0` 两种写法），overrides 照常铺进版本目录。

安装期间左下角有一个**全局面板**实时滚动安装器输出（120ms 批量推送，只留最后几行），
切到别的页也不会丢——处理器管线那几分钟不再是黑盒等待。

## 模组链路（对标 PCL2 的"给版本装 mod"）

- **在线装**：侧边栏「Mod」分类，按游戏版本 + 加载器过滤 Modrinth，一键进当前版本
  的私有 mods 目录；整合包 / 材质 / 光影 / 数据包同理（数据包进最近修改的存档）。
- **自动补前置**：装 mod 时解析 Modrinth 的 `dependencies`，`required` 且尚未安装的
  前置会**直接入队一起下**（队列里带「前置：」标签，完成提示里也列了名字）。
  「已安装」按文件名判断：完全同名，或文件名里包含该项目的 slug
  （停用的 `.disabled` 也算装过）——这样不会给同一个 mod 补出第二份造成重复崩溃。
  没有兼容当前游戏版本的 前置会跳过而不是拦住主 mod，真缺了启动失败时崩溃诊断会指出来。
- **本地文件**：拖进窗口，或「模组管理」页的**「添加 mod」按钮**（系统文件选择框，
  `tauri-plugin-dialog`，可多选）——两条路走同一个 `import_files` 后端分流，
  误选整合包 zip 也会正确新建版本。
- **管理**：mod 列表 + 启停开关（`x.jar` ↔ `x.jar.disabled`）。

## 运行环境与崩溃现场

这三件事是"扔给没用过启动器的朋友也能自己玩"的底线：

### Java 自动下载

新机器上没有 Java 时，首页的 Java 卡片会出现**「自动下载 Java X 运行时」**按钮：

1. 查 Adoptium 的 v3 API（Temurin JRE，按系统 OS / 架构挑最新版）；
2. 走全局下载队列，**清华 TUNA 镜像优先**（`mirrors.tuna.tsinghua.edu.cn/Adoptium/`，
   目录结构与 GitHub release 一致，只换域名），404 自动换官方直连；
   进度就在顶栏队列里，能暂停能取消；
3. 下载完先对 **sha256**（队列只校验 Mojang 的 SHA-1，Adoptium 给的是 sha256，
   装一个坏的 Java 比不装更坑），再解压到 `<数据目录>/jre/<主版本>/`；
4. 把 `bin/java` 写进 `settings.java_path`——之后的探测、启动、内存计算全走正常路径。

要装哪个版本由当前选中版本决定（`required_java_major`：1.17 以前 8、1.18–1.20.4 是 17、
1.20.5+ 是 21）。已装过就直接复用，不会重复下载。

### 游戏日志落盘

每次启动的完整输出写到 `logs/<版本>/<时间戳>.log`（文件头带 Java 路径和整条命令行），
关窗口不再丢日志。之前只有退出码，出了问题玩家只能截图控制台。

### 崩溃初判

进程非零退出时对日志尾部（最后 400 行）做关键词匹配，把**中文建议**直接放进
`game://exit` 事件，前端弹一张卡片：该做什么写在第一条，日志路径在下面，
「打开日志」一键定位文件。认得出的：

| 日志特征 | 建议 |
|---|---|
| `OutOfMemoryError` | 调大内存 / 少装 mod |
| `Could not reserve enough space` | 物理内存不够，调小内存 |
| `UnsupportedClassVersionError` | Java 版本太低，去首页一键下载 |
| `UnsatisfiedLinkError` / `Failed to locate library` | 删版本重装（natives 不完整） |
| `DuplicateModsFoundException` | 模组管理里删重复 |
| `missing mods` / `mod resolution` | 补前置 |
| `mixin apply failed` | 新装 mod 冲突，逐个停用排查 |
| `mods.toml` | mods 里混进了非 jar 文件 |
| `GLFW error` / `OpenGL` | 更新显卡驱动 |
| 退出码 `-1073741819`（0xC0000005） | 原生崩溃：mod 与版本不匹配或驱动过旧 |

认不出的给兜底建议：「把日志文件发人」。定位是挡掉大半求助，不是 PCL 那种 1100 行全诊断。

### 游戏进程管理

- **重复启动防护**：同一版本已在运行时再点启动会被后端拒绝——
  两个游戏抢同一份存档是存档损坏的高发原因。
- **结束游戏**：首页启动卡在游戏运行中会变成「结束游戏」按钮（Windows 走
  `taskkill /F /T` 连子进程一起收，Unix 直接 SIGKILL）。游戏在窗口外被关掉的话，
  每 5 秒的状态轮询会把按钮恢复成「启动游戏」。

## 内存与 GC

内存不走"玩家自己猜"，而是按 PCL2 的算法现算（`engine/memory.rs`）。
`-Xmx` 的推荐值分四段阶梯：前 1.5 GB 全额给，之后每段只肯给 70% / 40% / 15%，
因为堆外（Metaspace、JIT 代码缓存、LWJGL 直接内存）还要占 0.5–1 GB。
**越往后每 GB 可用内存换到的配额越少**，避免把机器吃干。

| 场景 | 下限 | T1 | T2 | T3 |
|---|---|---|---|---|
| 可装 mod | `0.5 + n/150` | `1.5 + n/90` | `2.7 + n/50` | `4.5 + n/25` |
| OptiFine | 0.5 | 1.5 | 3.0 | 5.0 |
| 原版 | 0.5 | 1.5 | 2.5 | 4.0 |

`n` 是 `mods/` 里 `.jar`/`.zip`/`.litemod` 的数量。可用内存 Windows 走
`GlobalMemoryStatusEx`、Linux 读 `/proc/meminfo` 的 `MemAvailable`。

两处与 PCL 的差异，都是为了解决"启动卡很久"：

- **`-Xms` 抬到 `-Xmx` 的一半**。PCL 只设 `-Xmx`，JVM 默认 `-Xms` 只有物理内存的 1/64，
  大整合包启动时堆要一路扩容、每扩一次一次 GC。初始堆同时受"当前可用内存 × 80%"约束，
  避免 `Could not reserve enough space`。
- **清理版本自带的 GC 参数**。版本 json 里若带了 `-XX:+UseG1GC`，会和我们的 `-XX:+UseZGC`
  冲突（JVM 直接拒绝启动），所以统一在拼参数时丢掉。

GC 四档（设置页可选），参数照搬 PCL 的 `LaunchAdvanceGC`：

| 模式 | Java 21+ | Java 15–20 | Java ≤14 |
|---|---|---|---|
| 自动 | ZGC + `ZGenerational` | ZGC | G1 |
| G1 | G1 | G1 | G1 |
| 优化 G1 | G1 + `PerfDisableSharedMem` / `MinHeapFreeRatio` / `MaxHeapFreeRatio` | 同左 | 同左（Java 8 另加 `ParallelRefProcEnabled`） |
| 自定义 | 不追加任何参数 | | |

设置页会把**实际要传给 JVM 的参数原样列出来**，不用猜。

## 导入与导出

**导入只有一个入口：把文件拖进窗口**。任意页面都可以，拖进来会盖一层
「松开即可导入」提示，并按扩展名自动分流（mod / 材质 / 光影进当前版本，
`.mrpack` / `.zip` 新建版本，`.png` 当离线皮肤）。界面上不再有填路径的输入框。

皮肤是唯一例外：首页的「导入皮肤」按钮走 `<input type="file">`（WebView2 里就是
系统文件对话框），因为读文件只需要 `File` 对象、不需要真实路径。

导入 = **新建一个独立版本**（和 PCL 一致），不是往当前目录里堆文件：

```
versions/<包名>/<包名>.json     ← 版本 JSON（加载器给的完整 JSON，或 inheritsFrom 薄壳）
versions/<包名>/game/           ← 这个包自己的 mods / config / saves
```

- **`.mrpack`**：读 `dependencies` 拿到「哪个 Minecraft 版本 + 哪个加载器」，
  自动补装原版和 Fabric/Quilt，再把 overrides 铺到新版本的 `game/`；
  `modrinth.index.json` 里列的文件进下载队列，目标是新版本的 `game/mods/`。
- **普通 `.zip`**：没有依赖信息，用当前选中的版本当基底，写一个
  `inheritsFrom` 薄壳，内容解进新版本的 `game/`。
- 已存在的文件**跳过不覆盖**；重名版本自动加 `-2`、`-3` 后缀，绝不复用旧目录。
- 导入完前端会**自动切到新版本**：加载器类库在返回前已经下完（缺了会启动失败），
  包里的 mod 继续在后台下载，进度看下载队列。

**导出在「版本管理」里**：每张版本卡片右下角都有「导出」，点开勾选
`模组 / 配置 / 存档 / 材质 / 光影`，把**那个版本**的 `game/`
打成 `exports/<包名>-<时间戳>.zip`，内部是 `overrides/` + `mcl-manifest.json`。
日志、崩溃报告、缓存一律排除。入口挂在卡片上而不是"当前选中版本"上，
所以可以备份一个没在用的包。


### 为"快"做的几件事

- **release 构建**：`bash tools/build_retry.sh --release --features custom-protocol`
  产出的二进制比 debug 版小得多，冷启动也快一截。
- **首屏不白闪**：窗口 `backgroundColor` 和 `index.html` 内联样式都是同一个深色，
  JS/CSS 加载完成前不会先闪一下白底。
- **事件订阅并行**：`bootstrap` 里四个 `listen` 用 `Promise.all` 一起注册，不再串行等四个来回。
- **长列表跳过渲染**：原版版本列表每一项都带 `content-visibility: auto`，
  滚动到几百个版本时浏览器只布局可视区。
- **进度事件节流**：`queue://progress` 每 120ms 一次，避免 IPC 打满。
- **慢命令进线程池**：Tauri 的同步命令默认在界面线程上跑，`java -version`
  （实测 130~170 ms）、递归统计 `versions/` 体积这类活会直接卡住点击。
  所以它们都标了 `#[tauri::command(async)]`，并且 Java 探测结果按 `javaPath`
  缓存——切版本、存设置不会每次都重起一个 Java 进程（"重新检测"可以强制刷新）。
- **阻塞调用不占 async worker**：`spawn_blocking` 只留给纯阻塞活，async 任务里
  一律不直接做整盘读写。目前走 blocking 池的有：复用检查 / SHA-1 校验、
  资源索引解析、natives 解压、旧版资源展开、整合包解压、实例导出、
  800 多个版本的"是否已安装"探测。
- **导出不重复压缩**：mod 包本身就是 jar / zip / png / ogg，再 deflate 一遍
  实测 84 MB 要烧 1969 ms、包只小 7%，所以这类扩展名直接 `Stored` 原样拷贝，
  只有 config / json 之类文本才压。240 MB 的实例实测 **463 ms** 导出完。
- **版本目录并行扫描**：`list_local` 要递归统计每个实例的体积，按版本切块丢给
  `std::thread::scope` 并行扫，不再一个接一个串着等。
- **安装收尾不再轮询**：`queue.wait()` 从 150 ms 轮询改成 `Notify` 唤醒，
  3598 个资源下完的那一刻就继续下一步，空闲时也不去抢锁。


## 下载链路

对照 PCL2 的 `NetTask` 做了取舍，只保留真正影响体验的部分：

| 能力 | 做法 |
|---|---|
| 分片并行 | 文件 ≥ 2 MB 且大小已知时拆段，每段 4 MB、最多 **16 段**；先发 `Range: bytes=0-0` 探测（结果按主机缓存），源不支持就整体回退单连接 |
| 段直写 | 目标文件先 `set_len` 预分配，每段 `seek` 到自己区间直接写 —— 不再产生 `.partN` 副本，也没有"合并"这一趟额外的读 + 写。一个 300 MB 的文件从 **≈900 MB 磁盘读写降到 ≈600 MB** |
| 文件级并发 | 队列同时下 **10 个文件**（assets 是几千个小文件，这项比单文件分片更重要） |
| 连接复用 | 连接池 `pool_max_idle_per_host = 64`、空闲保持 300 s；跨运营商一次 TLS 握手要 200–400 ms，几千个文件不复用光握手就吃掉几十秒 |
| 关闭 Nagle | `tcp_nodelay(true)`，小分片请求不必等 40 ms 的合并窗口 |
| 断点续传 | 段进度另存 `<file>.merging.json`（每 5 秒落一次盘），重试/重开从记录的偏移接着下；单连接仍从 `.part` 现有长度续 |
| 失败换源 | 每个任务带主备地址（镜像 ↔ 官方），`net::endpoints` 负责推；文本类请求同样双源 |
| 重试 | 3 次退避（`700ms × 2^n`，封顶 8s），429 额外 +5s；**403/404 不重试**，白等没有意义 |
| 校验 | 单连接边下边算 SHA-1，下完不用再读一遍；分片是并发写的，只能顺序算，所以还剩一趟读。哈希不符直接删临时文件 |
| 落盘 | 先写临时文件，`sync_all` 之后才 rename，断电不会留下"名字对、内容缺"的文件 |
| 元数据缓存 | 版本清单落盘到 `cache/version_manifest.json`，30 分钟内直接用；这份 JSON 有 200–400 KB，之前每次进版本页都重拉 |

### 布局为什么是「固定外壳 + 内部滚动」

`main` 只负责裁切（`overflow-hidden`），不吃滚动条；每个页面根节点拿 `h-full min-h-0`
自己当 flex 容器，把"常驻部分"和"可滚内容"分开：

| 页面 | 常驻 | 自己滚的 |
|---|---|---|
| 首页 | 启动卡、账户卡 | 下方卡片网格 |
| 版本管理 / 模组管理 | 工具条（+ mods 目录提示） | 版本 / 模组列表 |
| 下载 | 页头、筛选条、分类条 | 结果列表（原版游戏则是左右两栏的版本清单） |
| 设置 | 四张设置卡铺满 2×2、保存条、关于卡 | 卡片正文（内容超出时才滚） |
| 联机 / 整合包 | —— | 整页内容一个滚动区 |

好处是切页、翻列表时外壳不跳，也不会出现"整页滚到底才能看到筛选条"。
代价是纵向空间要省着花，所以下载页做了三处压缩：筛选条**单行**、排序挪进列表标题行、
本地包提示并进标题行右侧。1062×617 窗口下列表可视约 **320px（3.4 行）**，改造前只有 120px。

原版游戏页是「正式版（左，占宽）+ 快照（右，占窄）」两栏，各自内部滚动，整页不滚；
「其他下载」入口不放在这一页，分类只留在左侧导航。社区资源列表不再有「加载更多」按钮 ——
滚到离底部 160px 时自动续下一页，返回不足 20 条就标记到底，底部只留一行「已经到底了」。

设置页也不再整页滚：下载源 / 主题 / 内存与账户 / Java 四张卡按 2×2 铺满，
卡片标题固定、正文超出时自己滚，底部「保存」与「关于」常驻。行高比例 `1fr / 1.45fr`，
给内容多的「内存与账户」多分一点。

### 源选错了才是最常见的"慢"

设置页有个 **测速** 按钮：同时请求官方和 BMCLAPI 的版本清单（各读前 32 KB 就断开），
把两边的毫秒数摊开给你看，下面直接给「用最快的」按钮。

选官方源时是真正跨境直连 `piston-meta.mojang.com`，国内常见只有几十 KB/s；
BMCLAPI 在国内通常快一个数量级，但它偶尔也会抽风——所以别凭感觉猜，测一下。

还没做的：磁盘剩余空间预检、整盘校验（PCL 那种"修复文件"）、
多源竞速（hedged request：主源 X 毫秒没响应就同时发备用，谁先给数据用谁）。

## IPC 命令

| 命令 | 作用 |
|---|---|
| `list_remote_versions` | 拉版本清单（含"是否已安装"，带 30 分钟磁盘缓存） |
| `list_local_versions` | 扫 `versions/` 得到已安装版本 |
| `install_version(id)` | 安装：元数据 → 计划 → 入队 → 解压 natives |
| `install_loader(loader, gameVersion, loaderVersion?, targetId?)` | 装加载器并生成可启动的新版本：Fabric / Quilt 走 meta 服务；Forge / NeoForge 下载官方 installer 无头运行（缺 Java 会自动补装） |
| `list_loader_versions(loader, gameVersion)` | 该加载器支持的所有版本 |
| `test_sources` | 两个下载源各测一次，返回毫秒数 |
| `launch_version(id)` | 拼参数并启动 Java，返回 PID |
| `detect_java` | 探测 Java 路径与主版本 |
| `download_java(major)` | 自动下载 Adoptium JRE（TUNA 镜像优先，sha256 校验），装完写进设置 |
| `game_status` | 正在运行的游戏进程列表 |
| `game_kill(version?)` | 强制结束游戏（空 = 全部） |
| `get_settings` / `save_settings` | 设置读写（同时同步队列的下载源） |
| `queue_snapshot` / `queue_pause` / `queue_resume` / `queue_cancel` / `queue_clear` | 队列控制 |
| `search_modpacks(query)` | Modrinth 整合包搜索 |
| `browse_search(kind, query, gameVersion, loader, index, offset)` | 按分类搜 Modrinth；空 query + `index=downloads` 即热门 |
| `browse_install(..., targetVersion)` | 解析兼容版本并入队；**装 mod 会自动补装 required 前置**（跳过已装的）；整合包不用 `targetVersion`（会新建版本） |
| `import_modpack(path, baseVersion)` | 导入整合包，**新建一个独立版本**；`baseVersion` 只给普通 zip 当基底 |
| `import_files(paths, targetVersion)` | 拖拽导入：mod/材质/光影进 `targetVersion` 的私有目录，整合包新建版本 |
| `memory_plan(versionId)` | 内存方案：mod 数（按该版本目录统计）、可用内存、推荐值、实际 `-Xmx/-Xms`、GC 参数 |
| `export_instance(options, versionId)` | 导出**指定版本**的 `game/` 为 zip（版本管理卡片上的「导出」） |
| `list_mods(versionId)` / `toggle_mod(versionId, fileName)` | mod 列表与启停（按版本隔离） |
| `app_paths(versionId)` | 目录结构（设置页展示用，`game` 跟着选中版本变） |
| `skin_avatar(name?, uuid?)` | 玩家头像：有正版 UUID 才查官方皮肤，否则派生离线 UUID；有导入的皮肤就用它；返回 `data:` URL |
| `import_skin(data)` | 导入离线皮肤：`data` 是 data URL，校验 PNG 尺寸后落到 `skins/offline.png` 并记住路径 |
| `reset_skin()` | 删掉导入的皮肤，头像回到内置 Steve |
| `netplay_status()` | 三条通路的当前状态 + 是否已有密钥 + 缺不缺组件 + 是否管理员 |
| `netplay_easytier_start(req)` | 创建/加入 EasyTier 房间，轮询出虚拟 IP 后返回 |
| `netplay_portmap_start(req)` | UPnP 加映射 + STUN 探公网地址，返回 `公网IP:端口` |
| `netplay_frp_login()` / `netplay_frp_save_token(token)` | 用已存 / 新填的访问密钥拉隧道列表 |
| `netplay_frp_start(tunnelId, localPort)` | 拉服务端配置 → 改本地端口 → 起 frpc → 等连通 |
| `netplay_stop(channel)` | 停掉某条通路（杀掉子进程） |
| `netplay_lan_info(localPort?)` | 局域网 IP + 该端口是否有人监听 |
| `netplay_relaunch_admin()` | 用管理员权限重启自己（EasyTier 建虚拟网卡要） |

## 事件

| 事件 | 载荷 |
|---|---|
| `queue://progress` | `{ id, label, kind, done, total }`（120ms 节流） |
| `queue://changed` | 完整队列快照 `QueueItem[]` |
| `log://line` | `{ stream: "out" \| "err", line }` |
| `game://exit` | `{ version, code, logPath, tips }`——崩溃初判的建议与日志路径（正常退出时 tips 为空） |
| `installer://log` | `{ loader, line }`，Forge/NeoForge 官方安装器的实时输出（120ms 批量推送） |
| `installer://done` | `{ loader, ok }`，安装器退出；前端据此收起进度面板 |
| `netplay://log` | `{ channel, stream, line }`，子进程的每一行输出 |
| `netplay://status` | `{ channel, slot }`，某条通路状态变了（含进程意外退出） |

## 几条实现原则

1. **前端零 IO**：没有 `fetch`、没有 `fs`，连"文件大小格式化"都只是个纯展示函数。
   所有网络与磁盘操作都在 `engine/`。
2. **下载只有一个入口**：安装、整合包、单文件全部进 `queue`，
   于是「下载页」天然是全局的，暂停也是全局的。
3. **哈希在流里算**：`download.rs` 边读边喂 SHA-1，不二次读盘；
   校验失败先删 `.part` 再报错，不留半截成品。
4. **继承链自己走**：`meta::load_merged_with_chain` 处理 `inheritsFrom`，
   并且会区分"实例目录"和"客户端 jar 所在目录"——Fabric/Forge 的目录里通常没有 jar。
   **natives 也沿链找**：薄壳版本自己不会产出 `.dll`，`-Djava.library.path` 必须
   落到真正解压过 LWJGL 的那一层（原版目录），否则一启动就 `Failed to locate library`。
5. **错误一律走 `Result`**：后端错误序列化成字符串回传，前端 `notify` 直接展示。

## 已知限制

完整评估与优先级见 **[docs/ROADMAP.md](./docs/ROADMAP.md)**（含与 PCL2 / HMCL / Prism / Modrinth App 的逐项对照）。
最要紧的几条：

- **Forge / NeoForge 的边界**：都能自动装了（跑官方 installer 无头模式），
  但 Forge 只支持 1.13+（更早的 universal.jar 时代装法完全不同）、
  NeoForge 只支持 1.20.2+（1.20.1 的 NeoForge 请用 Forge）。
  NeoForge 的 maven（maven.neoforged.net）国内可达性差，装它可能要等或失败；
  Forge 走 BMCLAPI 镜像 + 官方 maven（Cloudflare）兜底，国内没问题。
- **崩溃分析是初判不是全诊断**：常见崩溃（内存不足 / Java 版本太低 / 缺前置 / mod 冲突 /
  显卡驱动 / 原生崩溃）会给出中文建议，认不出的只提示把日志发人。
  游戏日志已落盘 `logs/<版本>/<时间>.log`，不再关窗口就丢。
- **没有正版登录**：离线账号 + 皮肤站（authlib-injector），不做微软 OAuth。
- **导入没有「浏览」按钮**：mod / 材质 / 光影 / 整合包只能拖进窗口
  （要接 `tauri-plugin-dialog` 才能点选）。皮肤是例外，走 WebView 的 `<input type="file">`。
- **离线皮肤只在启动器里生效**：游戏内换肤需要服务端或 `CustomSkinLoader` 之类的 mod；
  另外皮肤只有一张（不按版本区分），HD 皮肤上限 1 MB。
- **磁盘预检有了，"修复文件"还没有**：空间不够会在下载前拦住；
  但文件坏了仍只能删版本重装（没有按 SHA-1 全量核对补缺）。
- **老版本（1.6 以前）的 `virtual` / `resources` 资源布局**：
  `virtual` 铺到 `assets/virtual/`，`map_to_resources` 铺到版本自己的 `game/resources/`，
  两条都已实现；但只覆盖索引里声明了这两个字段的情况。
- **只有深色主题**：设置里「浅色」是禁用占位。
- **EasyTier 要管理员权限**：虚拟网卡是驱动级操作。非管理员时联机页会拦住并给提权按钮，
  但端口映射和樱花 FRP 不受影响，能照常用。
- **没做 DDNS / SRV 记录**：LinkStar 的"朋友只填域名、端口变了也不用重发"那套要域名 +
  Cloudflare 账号，这里没做——端口映射拿到的是 `IP:端口`，重启后端口可能变。
- **樱花 FRP 要自己有账号**：注册 + 实名（1 元）+ 建一条 TCP 隧道才能在 MCL 里选到它；
  MCL 不会替你建隧道。
- **发布包必须带 `resources/` 目录**：联机组件是独立可执行文件（48 MB），不能像以前那样
  只拷一个 `MCL.exe` 走。少了 `resources/netplay/` 联机页会提示缺哪个文件。
