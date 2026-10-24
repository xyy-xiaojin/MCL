# MCL 测试报告（智谱接手第一轮 · 2026-10-24）

环境：Linux x86_64（内核 7.0.0-38-generic），Rust + Cargo、Node.js/npm 均为本机已装版本。
修改前后各执行了一轮完整基线，结果一致（修改只涉及任务组标注与演示数据）。

## 一、已执行且通过

| 命令 | 结果 |
| --- | --- |
| `npm install` | 成功，依赖按 `package-lock.json` 安装 |
| `npm run build`（= `vue-tsc --noEmit && vite build`） | 通过：类型检查 0 错误，产物 234.59 kB JS / 41.18 kB CSS |
| `cargo check`（在 `src-tauri/`） | 通过，0 警告 0 错误 |
| `cargo test` | **133 通过 / 0 失败 / 3 忽略**（忽略项均为标注 `#[ignore]` 的真实网络/实机探针测试） |
| API 与命令注册对账（脚本比对 `api.ts` 调用名与 `lib.rs` 注册名） | 一致，45 个命令全部注册 |

关键测试覆盖（摘自 133 项，与任务清单重点对应）：
- `engine::download::*`：分片直写内容正确、按段进度续传、不支持 Range 回退单连接、哈希不符清理临时文件、分片失败保留续传状态；
- `engine::queue::*`：终态判定、批次组名归类、错误建议文案、显式任务组盖章（本轮新增断言）；
- `engine::install::tests`：natives 解压黑名单、Java 主版本推断；
- 其余覆盖 net 重试策略、memory、modpack、skin、netplay 等。

## 二、执行过但被忽略（明确不算通过）

- `real_network_segmented_download`（真实下载 BMCLAPI 客户端 jar）
- `real_profile_yields_a_png_skin`（真实访问 Mojang sessionserver）
- `real_probe`（真实网络探测）

这三项需要外网访问，未在本次运行中执行。

## 三、未执行（环境不具备，不得视为通过）

1. **Windows 构建与实机运行**：本机为 Linux，未执行 `npm run tauri build` 的 Windows 目标，未验证 Windows 控制台闪现、注册表找 Java、explorer 定位等 Windows 专属路径（相关代码均为 `#[cfg(windows)]` 分支，只做了编译期确认——`cargo check` 在 Linux 下不编译 windows 分支，这点也如实说明）。
2. **Linux 桌面实机运行**：无图形桌面/WebView 运行环境，未执行 `tauri dev` / 安装包安装、真实安装一个 Minecraft 版本、真实启动游戏。
3. **端到端安装链路实测**（安装原版 → 下载抽屉显示单组 → 整组取消 → 失败重试）：静态审查 + 单元测试覆盖了各环节，但未在真实 Tauri 窗口里跑通全流程。
4. **真实整合包导入、Forge/NeoForge 安装器实机流程**：依赖外网与图形环境，未执行。

## 四、结论

- 静态检查、编译、单元测试、前后端命令对账：全部通过。
- 实机/端到端：未执行，需在 Windows 与带桌面的 Linux 上按上述清单补测后才可声称"实机验证通过"。
