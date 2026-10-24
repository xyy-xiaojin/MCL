# 联机组件（不进 Git 仓库）

这个目录里的二进制是第三方成品，**不进 Git 仓库**（见 `.gitignore`）。构建 Windows 安装包前请自行下载并放到本目录：

| 文件 | 来源 | 用途 |
| --- | --- | --- |
| `easytier-core.exe` / `easytier-cli.exe` | https://github.com/EasyTier/EasyTier/releases | 虚拟组网（联机页「EasyTier」通道） |
| `frpc.exe` | https://github.com/fatedier/frp/releases | FRP 隧道（联机页「FRP」通道） |
| `wintun.dll` | https://www.wintun.net/ | EasyTier 建虚拟网卡依赖 |
| `Packet.dll` / `WinDivert64.sys` | 随 WinDivert / Npcap 发行 | 端口映射探测 |

只需要 **Windows 版**：Linux 上联机功能不可用，构建 Linux 包时本目录可以为空（已验证：目录里没有这些文件时 `cargo check` 与构建流程均正常，仅联机页会提示"组件缺失"）。
