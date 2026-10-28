# MCL · Minecraft 启动器

用 **Tauri 2 + Vue 3 + Rust** 写的 Minecraft 启动器。前端只负责渲染，
下载、文件 IO、哈希校验、启动游戏全部在 Rust 侧完成。

## 功能

- **一键安装游戏**：原版 / Fabric / Forge / NeoForge / Quilt，Java 缺失时自动下载 Temurin JRE
- **版本隔离**：每个版本独立的 mods / 存档 / 配置目录
- **账户**：离线模式、皮肤站登录（authlib-injector / LittleSkin 等）、微软正版（设备码流程，需配置 `MS_CLIENT_ID`）
- **联机**：EasyTier 组网 / 端口映射 / 樱花 FRP 三条通路（组件在 `resources/netplay/`，不能只拷 exe）
- **整合包**：`.mrpack` / zip 导入导出，mod / 材质 / 光影拖拽安装
- **下载队列**：并发分片、断点续传、哈希校验、BMCLAPI 镜像
- 游戏日志落盘 + 崩溃初判、重复启动防护、磁盘预检、内存自动分配

## 开发

前置：

- Node ≥ 18、Rust ≥ 1.77
- Windows：WebView2 运行时（Win11 自带）
- Linux：`libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev`
  （`sudo apt install` 对应包名以发行版为准）
- macOS：Xcode Command Line Tools

```bash
npm ci            # 前端依赖
npm run dev       # 只跑前端（浏览器可预览，数据来自 src/mock.ts）
npm run tauri dev # 连 Rust 后端一起跑
```

## 构建

```bash
npm run tauri build
```

产物：

- Windows：`src-tauri/target/release/mcl-tauri.exe` + `bundle/msi`、`bundle/nsis`
- Linux：`src-tauri/target/release/mcl-tauri` + `bundle/deb`
- macOS：`bundle/dmg`、`bundle/macos`

只想先看界面，也可以 `npm run build` 出静态前端到 `dist/`。

Windows 开发机若开启 Smart App Control 会拦截 release 产物，可用自带重试脚本：

```bash
bash tools/build_retry.sh --features custom-protocol
```

也可以直接用 **GitHub Actions**（`.github/workflows/windows.yml`，手动触发）：
云端出 release 成品 exe + 免安装 zip，在 Actions 运行页的 Artifacts 下载。

## 源仓库不含的东西

仓库**只保留源码**，以下内容需要自己准备：

| 内容 | 说明 |
| --- | --- |
| `src-tauri/resources/netplay/*.exe/.dll/.sys` | 联机组件（EasyTier / frpc / WinDivert / wintun），约 48MB，第三方成品。获取方式见 [该目录的 README](src-tauri/resources/netplay/README.md)；Linux 构建可以为空 |
| `node_modules/`、`dist/`、`src-tauri/target/` | 依赖与构建产物，`npm ci` / `npm run tauri build` 自动生成 |

## 结构

```
src/               前端（Vue 3）
src-tauri/src/     后端（Rust）
  engine/          启动核心：下载、安装、启动、账户、联机…
src-tauri/resources/netplay/   联机组件（第三方二进制，需自行获取）
.github/workflows/ CI 云端构建
```

> 皮肤站与微软正版登录的令牌只存在本地 `account.json`，密码从不落盘。
