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

前置：Node ≥ 18、Rust ≥ 1.77（Windows 另需 WebView2）。

```bash
npm ci            # 前端依赖
npm run dev       # 开发模式（vite dev server + cargo run）
```

## 构建

```bash
npm run build
cargo build --features custom-protocol   # 在 src-tauri/ 下执行
```

开发机若开启 Smart App Control 会拦截 release 产物，可用自带重试脚本：

```bash
bash tools/build_retry.sh --features custom-protocol
```

推荐直接用 **GitHub Actions**（`.github/workflows/windows.yml`）：push 后云端出
release 成品 exe + 免安装 zip，在 Actions 运行页的 Artifacts 下载。

## 结构

```
src/               前端（Vue 3）
src-tauri/src/     后端（Rust）
  engine/          启动核心：下载、安装、启动、账户、联机…
src-tauri/resources/netplay/   联机组件（第三方二进制）
.github/workflows/ CI 云端构建
```

> 皮肤站与微软正版登录的令牌只存在本地 `account.json`，密码从不落盘。
