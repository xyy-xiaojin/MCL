# MCL · Minecraft 启动器

Tauri 2 + Vue 3 + Rust 写的 Minecraft 启动器。前端只做渲染，下载、文件 IO、哈希校验、启动游戏都在 Rust 侧。

## 功能

- **装游戏**：原版 / Fabric / Forge / NeoForge / Quilt，缺 Java 自动下 Temurin
- **版本隔离**：每个版本独立的 mods / 存档 / 配置
- **账户**：离线、皮肤站（authlib-injector）、微软正版（设备码，需 `MS_CLIENT_ID`）
- **联机**：EasyTier 组网 / 端口映射 / 樱花 FRP
- **资源**：整合包 `.mrpack` / zip 导入导出，mod / 材质 / 光影支持拖拽
- **下载队列**：并发分片、断点续传、哈希校验、BMCLAPI 镜像
- 崩溃初判、重复启动防护、内存自动分配

## 编译

前置：Node ≥ 18、Rust ≥ 1.77；Linux 另需 `libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev`。

```bash
npm ci
npm run tauri dev     # 开发
npm run tauri build   # 打包（产物在 src-tauri/target/release/）
```

## 仓库说明

**只保留源码**，以下内容需自备：

- `src-tauri/resources/netplay/` 的联机组件（EasyTier / frpc / WinDivert / wintun，约 48MB）
  获取方式见 [该目录 README](src-tauri/resources/netplay/README.md)；只做 Linux 构建可以留空
- 依赖与产物：`node_modules/`、`dist/`、`src-tauri/target/`（命令自动生成）

## 结构

```
src/                         前端（Vue 3）
src-tauri/src/engine/        后端核心：下载、安装、启动、账户、联机
src-tauri/resources/netplay/ 联机组件（二进制，需自行获取）
```

> 登录令牌只存本地 `account.json`，密码从不落盘。
