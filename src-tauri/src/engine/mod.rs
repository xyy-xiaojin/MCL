//! 启动核心。
//!
//! 这一层扮演的是 **Theseus（Modrinth 的启动核心）** 在 Modrinth App 里的角色：
//! 版本元数据、下载、安装、启动、整合包全部收在 Rust 侧，前端只负责渲染。
//! 关于为什么不直接依赖真实 Theseus，见仓库根目录的 `THESEUS.md`。
//!
//! 分工：
//! - [`net`]：HTTP 客户端、镜像重写、失败归类与重试策略
//! - [`meta`]：版本清单 / version.json / 资源索引模型，rules 判定与继承合并
//! - [`download`]：单文件下载（分片并行 / 断点续传 / 换源）+ SHA-1 校验
//! - [`queue`]：全局下载队列（暂停/继续/取消），通过 Tauri Event 推进度
//! - [`install`]：安装计划 → 入队 → 解压 natives
//! - [`launch`]：参数拼装并拉起 Java 子进程
//! - [`memory`]：可用内存探测、PCL 式自动分配、GC 参数
//! - [`imports`]：拖拽导入，按包内结构分发到对应版本的目录
//! - [`loader`]：加载器安装——Fabric / Quilt 走 meta 服务，Forge / NeoForge 走官方安装器
//! - [`forge`]：Forge / NeoForge 无头安装（BMCLAPI 列表 + 官方 installer + 版本目录归化）
//! - [`browse`]：按分类搜索 Modrinth（热门 / 搜索）并一键入队
//! - [`java`]、[`jdk`]、[`mods`]、[`modpack`]：运行环境与资源管理（jdk 是 Java 自动下载）
//! - [`disk`]：磁盘剩余空间预检
//! - [`skin`]：玩家头像（皮肤地址查询、贴图缓存、Steve/Alex 判定）
//! - [`netplay`]：联机（EasyTier 组网 / 端口映射直连 / 樱花 FRP 隧道）

pub mod auth;
pub mod browse;
pub mod disk;
pub mod download;
pub mod export;
pub mod forge;
pub mod imports;
pub mod install;
pub mod java;
pub mod jdk;
pub mod launch;
pub mod loader;
pub mod memory;
pub mod meta;
pub mod modpack;
pub mod mods;
pub mod net;
pub mod netplay;
pub mod queue;
pub mod skin;
pub mod speedtest;
