# 在 Linux 上构建 MCL

> 一句话结论：**能，而且比 Windows 顺。** 现在卡住 Windows 的
> `os error 4551`（Smart App Control 拦截新编译的 build script）是 Windows
> 独有的机制，Linux 没有这一层，`cargo build --release` 直接就能出成品。

## 为什么 Windows 上出不了 release

`release` 构建里有几个 proc-macro 的 DLL（如 `darling_macro`、
`thiserror_impl`）被 Smart App Control 判定为「未知发布者」而拒绝加载，
连跑 20 轮、每次 `cargo clean -p` 重编都过不去。Windows 上只能退到
`debug + opt-level=1` 凑合。

Linux 没有代码签名/信誉拦截，动态库加载只看 `ld.so` 的搜索路径，
同样的代码 `cargo build --release` 一次过。

## 一、装系统依赖

Tauri 2 在 Linux 上链的是 GTK + WebKitGTK，这两样必须由系统提供
（这一点和 Rust、Node 的版本完全无关，是本机装不出来的部分）。

**Debian / Ubuntu（推荐 24.04 LTS）**

```bash
sudo apt update
sudo apt install -y \
  libwebkit2gtk-4.1-dev build-essential curl wget file \
  libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev
```

> ⚠️ 必须 **4.1**，不是 4.0。Ubuntu 22.04 的默认仓库只有 4.0，
> 需要先 `sudo add-apt-repository ppa:ubuntu-webkit-team/ppa` 或用 24.04。
> 这是从 Tauri 1 迁到 Tauri 2 最常见的坑。

**Fedora**

```bash
sudo dnf install -y webkit2gtk4.1-devel openssl-devel \
  curl wget file libappindicator-gtk3-devel librsvg2-devel
sudo dnf group install -y "c-development"
```

**Arch / Manjaro**

```bash
sudo pacman -S --needed webkit2gtk-4.1 base-devel curl wget file \
  openssl libappindicator-gtk3 librsvg xdotool
```

**Rust 与 Node**

```bash
curl --proto '=https' --tlsv1.2 https://sh.rustup.rs -sSf | sh
# Node 20+，用发行版的包或 nvm 都行
```

## 二、构建

把项目拷到 Linux 机器上（U 盘 / `git clone` / 共享目录都行），
**不要拷 `node_modules` 和 `src-tauri/target`**——那是 Windows 的产物，
换个系统装回来更干净。

```bash
cd MCL
bash tools/build_linux.sh            # 出 ./MCL
bash tools/build_linux.sh --bundle   # 额外出一个 .deb
```

脚本做四件事：检查 `webkit2gtk-4.1` 是否存在 → `npm ci && npm run build`
→ `cargo build --release --features custom-protocol` → 校验前端是否真的
嵌进了二进制。

手动等价命令：

```bash
npm ci
npm run build
cd src-tauri
cargo build --release --features custom-protocol
cp target/release/mcl-tauri ../MCL
```

> **`--features custom-protocol` 不能省**。它控制 Tauri 把 `dist/` 内嵌
> 进二进制；漏掉的话程序会去连 `http://localhost:1420`，窗口一片空白。

## 三、直接跑

```bash
./MCL
```

数据目录在标准 XDG 位置，和 Windows 版互不相干：

```
~/.local/share/MCL/
├── versions/     版本 JSON 与客户端 jar
├── libraries/    库与 natives 压缩包
├── assets/       资源对象 + indexes
├── game/         游戏工作目录（mods / resourcepacks / shaderpacks / saves）
└── settings.json
```

## 四、装 JDK

启动器**只探测、不下载** Java。Linux 上先自己装一个（Minecraft 1.20+ 要 17，
1.20.5+ 要 21）：

```bash
# Debian / Ubuntu
sudo apt install -y openjdk-21-jdk

# Fedora
sudo dnf install -y java-21-openjdk-devel

# Arch
sudo pacman -S --needed jdk21-openjdk
```

装完在启动器「设置 → Java 路径」里留空即可，`java.rs` 会依次找
用户设置 → `JAVA_HOME` → `PATH` → `/usr/lib/jvm` → `~/.sdkman/candidates/java`。
首页会显示探测到的路径和主版本号。

## 五、跑 Minecraft 本身还需要的库

这部分**不是**启动器的依赖，是游戏（LWJGL）的依赖。缺了会在启动瞬间
报 `UnsatisfiedLinkError: libGL.so.1` 之类。

```bash
# Debian / Ubuntu
sudo apt install -y libgl1 libglx-mesa0 libopenal1 libx11-6 \
  libxcursor1 libxrandr2 libxinerama1 libxi6 libxxf86vm1

# Fedora
sudo dnf install -y mesa-libGL openal-soft libX11 libXcursor \
  libXrandr libXinerama libXi libXxf86vm

# Arch
sudo pacman -S --needed mesa openal libx11 libxcursor libxrandr \
  libxinerama libxi libxxf86vm
```

Wayland 会话下如果窗口异常，可以试 `WAYLAND_DISPLAY= ./MCL` 强制走 XWayland。

## 六、代码里已经做好的跨平台处理

换系统不需要改这些，列出来是为了知道哪些地方已经在管平台差异：

| 位置 | 处理 |
|---|---|
| `engine/launch.rs` | classpath 分隔符 Windows `;` / 其它 `:` |
| `engine/java.rs` | 探测 `JAVA_HOME`、`PATH`，以及 `/usr/lib/jvm`、`~/.sdkman/candidates/java`、`~/.jdks`、`/opt/java` |
| `engine/meta.rs` | `Platform.os` 按 `cfg!(target_os)` 给出 `windows`/`osx`/`linux`，natives 分类器随之选 `natives-linux` |
| `engine/meta.rs` | `os.version` 规则：Linux 上取 `/etc/os-release` 的 `VERSION_ID`（如 `24.04`） |
| `state.rs` | 数据目录走 `dirs::data_dir()`，Linux 自动落到 `~/.local/share` |
| `style.css` | 字体栈含 Noto Sans CJK / 思源黑体，中文不会掉到 DejaVu |

## 七、做不到的事

- **不能在 Windows 上交叉编译 Linux 版。** Tauri 要链接 `webkit2gtk`
  和 GTK，需要目标系统的 `pkg-config` 与头文件，交叉工具链凑不齐。
  要么用 Linux 机器，要么用 WSL2 / 虚拟机，要么走 CI。
- **AppImage 打包需要额外下载 `linuxdeploy`** 且依赖 FUSE，脚本默认只出
  裸二进制和 `.deb`。要 AppImage 用 `npm run tauri build -- --bundles appimage`。
- **release 版比 debug 小很多**（Windows 上 debug 加一级优化是 14 MB，
  Linux release 通常 8–10 MB），第一次跑记得确认窗口能出来再继续调功能。
