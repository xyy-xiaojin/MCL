# 安装向导实现说明

## 本次改动

- 在「版本」页新增「安装游戏…」安装向导，默认配置为 Minecraft 1.21.1 + Fabric + 自动推荐加载器版本 + 自动 Java。
- 游戏版本选择使用后端现有的真实 Minecraft 版本清单；加载器选择通过现有后端接口读取真实可用版本列表。
- 支持原版、Fabric、Forge、NeoForge、Quilt。选择加载器时先安装/修复原版基础版本，再调用既有加载器安装流程，完成后刷新本地版本列表并选中新版本。
- Fabric / Quilt 的「推荐稳定版」由后端现有的官方 Meta API 选择；Forge / NeoForge 复用现有官方安装器流程。
- Java 自动策略根据 Minecraft 版本主线选择 Java 8 / 16 / 17 / 21；如果当前检测到的 Java 主版本不匹配，则调用已有 Temurin JRE 安装流程。手动使用已有 Java 时会先校验主版本。
- 原有单独安装原版、已安装版本上追加加载器、下载队列、启动、导入导出等入口保留。

## 验证状态

- 已完成源码静态检查及差异检查。
- 本环境没有 `cargo`，无法运行 Rust 编译检查。
- `npm ci --ignore-scripts --no-audit --no-fund` 在当前环境超时，前端依赖未安装成功，因此没有声称前端构建通过。
- 尚未执行真实联网安装测试；请在具备网络和 Rust/Node 构建环境的机器上运行 `npm ci`、`npm run build` 与 `cargo check --manifest-path src-tauri/Cargo.toml --locked`，再进行 1.21.1 + Fabric 实际安装验证。
