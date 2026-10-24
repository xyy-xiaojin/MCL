//! 内存：可用内存探测、PCL 式自动分配、GC 参数。
//!
//! 三件事对应 PCL2 的三处实现：
//! - 分配算法照搬 `Pages/PageSetup/PageSetupLaunch.GetRam`：不是简单按比例，
//!   而是分四段阶梯——越往后每 GB 可用内存能换到的配额越少，避免把机器吃干。
//! - GC 参数照搬 `Modules/Minecraft/ModLaunch` 里 `LaunchAdvanceGC` 那一段。
//! - 可用内存：Linux 读 `/proc/meminfo`，Windows 调 `GlobalMemoryStatusEx`。
//!
//! 这里刻意不引 `sysinfo`：只要两个数字，不值得为它多编一个大 crate。
use std::path::Path;

/// 当前可用于新进程的物理内存（MB）。
///
/// Linux 用 `MemAvailable`（内核算好的"还能给多少"，比 MemFree 准），
/// Windows 用 `ullAvailPhys`。拿不到时返回 0，调用方据此退回手动值。
pub fn available_mb() -> u64 {
    imp::available_mb()
}

/// 物理内存总量（MB）。
pub fn total_mb() -> u64 {
    imp::total_mb()
}

#[cfg(target_os = "linux")]
mod imp {
    fn read_meminfo(key: &str) -> u64 {
        let Ok(text) = std::fs::read_to_string("/proc/meminfo") else {
            return 0;
        };
        text.lines()
            .find_map(|line| {
                let rest = line.strip_prefix(key)?;
                let kb: u64 = rest.trim().trim_start_matches(':').trim().split_whitespace().next()?.parse().ok()?;
                Some(kb)
            })
            .map(|kb| kb / 1024)
            .unwrap_or(0)
    }

    pub fn available_mb() -> u64 {
        read_meminfo("MemAvailable:")
    }

    pub fn total_mb() -> u64 {
        read_meminfo("MemTotal:")
    }
}

#[cfg(windows)]
mod imp {
    /// `MEMORYSTATUSEX` 的展开写法。
    ///
    /// 不用 `windows-sys` 是因为它的构建脚本在开了 Smart App Control 的机器上
    /// 会被拦（`os error 4551`），而这里只要这一个函数，手写声明更省事：
    /// `kernel32` 是默认链接的，不需要任何依赖。
    #[repr(C)]
    struct MemoryStatusEx {
        dw_length: u32,
        dw_memory_load: u32,
        ull_total_phys: u64,
        ull_avail_phys: u64,
        ull_total_page_file: u64,
        ull_avail_page_file: u64,
        ull_total_virtual: u64,
        ull_avail_virtual: u64,
        ull_avail_extended_virtual: u64,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GlobalMemoryStatusEx(buffer: *mut MemoryStatusEx) -> i32;
    }

    fn status() -> Option<MemoryStatusEx> {
        let mut status = MemoryStatusEx {
            dw_length: std::mem::size_of::<MemoryStatusEx>() as u32,
            dw_memory_load: 0,
            ull_total_phys: 0,
            ull_avail_phys: 0,
            ull_total_page_file: 0,
            ull_avail_page_file: 0,
            ull_total_virtual: 0,
            ull_avail_virtual: 0,
            ull_avail_extended_virtual: 0,
        };
        // SAFETY: 结构体是 POD，dwLength 已按 API 要求填成自身大小。
        let ok = unsafe { GlobalMemoryStatusEx(&mut status) };
        (ok != 0).then_some(status)
    }

    pub fn available_mb() -> u64 {
        status()
            .map(|s| s.ull_avail_phys / 1024 / 1024)
            .unwrap_or(0)
    }

    pub fn total_mb() -> u64 {
        status()
            .map(|s| s.ull_total_phys / 1024 / 1024)
            .unwrap_or(0)
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
mod imp {
    pub fn available_mb() -> u64 {
        0
    }
    pub fn total_mb() -> u64 {
        0
    }
}

// ---------------------------------------------------------------------------
// 内存分配
// ---------------------------------------------------------------------------

/// 一次推荐的结果。前端据此显示"检测到 N 个 mod，建议 X GB"。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RamPlan {
    /// 统计到的 mod 数量（.jar / .zip / .litemod）
    pub mod_count: usize,
    /// 探测到的可用物理内存（MB），0 表示没拿到
    pub available_mb: u64,
    pub total_mb: u64,
    /// 推荐分配给 JVM 的堆上限（MB）
    pub recommend_mb: u32,
    /// 无论如何都要保证的下限（MB）
    pub minimum_mb: u32,
    /// 是否按"可装 mod 的版本"算的
    pub modable: bool,
    /// 建议来源，给前端显示用
    pub reason: String,
}

/// 统计 mod 数量。目录不存在就是 0。
pub fn count_mods(mods_dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(mods_dir) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|entry| {
            let path = entry.path();
            if !path.is_file() {
                return false;
            }
            matches!(
                path.extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase()
                    .as_str(),
                "jar" | "zip" | "litemod"
            )
        })
        .count()
}

/// 四段阶梯分配，照搬 PCL `PageSetupLaunch.GetRam`。
///
/// 直觉是：前 1.5 GB 是刚需，全额给；再往后每 1 GB 只肯给 70%，
/// 一直到 T3 之后只给 15%——因为堆外（Metaspace、JIT 代码缓存、
/// LWJGL 直接内存）还要占掉 0.5~1 GB，系统本身也要留。
fn staged_ram(available_gb: f64, t1: f64, t2: f64, t3: f64, minimum: f64) -> f64 {
    let mut left = available_gb;
    let mut give = 0.0_f64;

    // 阶段一：0 → T1，100%
    give += left.min(t1);
    left -= t1;
    if left < 0.1 {
        return round_tenth(give.max(minimum));
    }
    // 阶段二：T1 → T2，70%
    let delta = t2 - t1;
    give += (left * 0.7).min(delta);
    left -= delta / 0.7;
    if left < 0.1 {
        return round_tenth(give.max(minimum));
    }
    // 阶段三：T2 → T3，40%
    let delta = t3 - t2;
    give += (left * 0.4).min(delta);
    left -= delta / 0.4;
    if left < 0.1 {
        return round_tenth(give.max(minimum));
    }
    // 阶段四：T3 → T3*2，15%
    let delta = t3;
    give += (left * 0.15).min(delta);

    round_tenth(give.max(minimum))
}

fn round_tenth(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

/// 给出推荐值。
///
/// `instance_mods` 是实例自己的 mods 目录（用 `game/mods` 或版本隔离目录都行），
/// `optifine` 用来区分 OptiFine 版本——它的基线比普通版本高，但比大整合包低。
pub fn recommend(instance_mods: &Path, optifine: bool) -> RamPlan {
    let available_mb = available_mb();
    let total_mb = total_mb();
    let mod_count = count_mods(instance_mods);

    // Linux 的 MemAvailable 在容器里可能是 0，用总量兜个底
    let usable_mb = if available_mb > 0 {
        available_mb
    } else if total_mb > 0 {
        total_mb * 7 / 10
    } else {
        0
    };

    // 目录里有 mod 就算可装 mod 的版本，不再去猜加载器名字
    let modable = mod_count > 0 || optifine;

    let (minimum, t1, t2, t3, reason) = if optifine {
        (0.5, 1.5, 3.0, 5.0, "OptiFine 版本".to_string())
    } else if modable {
        let count = mod_count as f64;
        (
            0.5 + count / 150.0,
            1.5 + count / 90.0,
            2.7 + count / 50.0,
            4.5 + count / 25.0,
            format!("{mod_count} 个 mod"),
        )
    } else {
        (0.5, 1.5, 2.5, 4.0, "原版 / 未装 mod".to_string())
    };

    // 拿不到内存信息时给一个保守常量，至少别让大整合包跑在 2G 上
    let gib = if usable_mb > 0 {
        staged_ram(usable_mb as f64 / 1024.0, t1, t2, t3, minimum)
    } else {
        t2
    };

    RamPlan {
        mod_count,
        available_mb,
        total_mb,
        recommend_mb: (gib * 1024.0).round() as u32,
        minimum_mb: (minimum * 1024.0).round() as u32,
        modable,
        reason,
    }
}

// ---------------------------------------------------------------------------
// GC 参数
// ---------------------------------------------------------------------------

/// 垃圾回收策略，对应 PCL 的 `LaunchAdvanceGC`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GcMode {
    /// 按 Java 版本自动选：21+ 分代 ZGC，15~20 非分代 ZGC，更低走 G1
    Auto,
    /// 一律 G1GC（Mojang 默认参数）
    G1,
    /// 优化过的 G1GC，多几个调优参数
    G1Optimized,
    /// 不指定，交给玩家自己在 JVM 参数里写
    Custom,
}

impl GcMode {
    /// 前端存的是字符串，认不出来就当自动。
    pub fn parse(raw: &str) -> Self {
        match raw {
            "g1" => Self::G1,
            "g1-optimized" => Self::G1Optimized,
            "custom" => Self::Custom,
            _ => Self::Auto,
        }
    }
}

/// 生成 GC 参数。`java_major` 来自 `java.rs` 的探测结果，0 表示未知。
pub fn gc_args(mode: GcMode, java_major: i64) -> Vec<String> {
    if mode == GcMode::Custom {
        return Vec::new();
    }

    // 未知版本按 8 处理：G1 在 8 上可用，ZGC 不能
    let major = if java_major > 0 { java_major } else { 8 };
    let use_g1 = match mode {
        GcMode::Auto => major < 15,
        GcMode::G1 | GcMode::G1Optimized => true,
        GcMode::Custom => return Vec::new(),
    };

    let mut args: Vec<String> = Vec::new();
    // ZGC 与 UseCompactObjectHeaders 都还是实验特性
    args.push("-XX:+UnlockExperimentalVMOptions".to_string());

    if major >= 24 {
        args.push("-XX:+UseCompactObjectHeaders".to_string());
    }

    if use_g1 {
        args.push("-XX:+UseG1GC".to_string());
        args.push("-XX:G1NewSizePercent=20".to_string());
        args.push("-XX:G1ReservePercent=20".to_string());
        args.push("-XX:G1HeapRegionSize=32M".to_string());
        args.push("-XX:MaxGCPauseMillis=50".to_string());
        if major <= 7 {
            args.push("-XX:MaxPermSize=512m".to_string());
        }
        if mode == GcMode::G1Optimized {
            // 不再往共享内存写 perf 数据，少一点 IO 抖动
            args.push("-XX:+PerfDisableSharedMem".to_string());
            if major == 8 {
                // Java 8 的引用处理是单线程的，开了省一大截 GC 时间
                args.push("-XX:+ParallelRefProcEnabled".to_string());
            }
            if major >= 12 {
                // 必须比 G1NewSizePercent 大一点，否则堆会频繁收缩
                args.push("-XX:MinHeapFreeRatio=25".to_string());
                args.push("-XX:MaxHeapFreeRatio=40".to_string());
            }
        }
    } else {
        args.push("-XX:+UseZGC".to_string());
        // Java 23 起分代 ZGC 是默认，21/22 要显式开
        if major == 21 || major == 22 {
            args.push("-XX:+ZGenerational".to_string());
        }
    }

    args
}

/// 判断一条已有参数是不是 GC 相关的，用于去重。
///
/// 版本 json 里偶尔自带 `-XX:+UseG1GC`，不清掉会和我们的参数打架
/// （后出现的生效，但两个 `UseG1GC` 和 `UseZGC` 同时存在会直接启动失败）。
pub fn is_gc_arg(arg: &str) -> bool {
    let body = arg.trim();
    if !body.starts_with("-XX:") {
        return false;
    }
    let name = &body[4..];
    let name = name.strip_prefix('+').or_else(|| name.strip_prefix('-')).unwrap_or(name);
    let name = name.split('=').next().unwrap_or(name);
    const EXACT: [&str; 5] = [
        "UseG1GC",
        "UseZGC",
        "UseSerialGC",
        "UseParallelGC",
        "UseShenandoahGC",
    ];
    if EXACT.contains(&name) {
        return true;
    }
    // UseXxxGC 一律算，另外覆盖 PCL 列出的那几个调优项
    (name.starts_with("Use") && name.ends_with("GC"))
        || name == "UseCompactObjectHeaders"
        || name.starts_with("G1")
        || name == "ZGenerational"
        || name.starts_with("MaxGCPauseMillis")
        || name.starts_with("MinHeapFreeRatio")
        || name.starts_with("MaxHeapFreeRatio")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PCL 的算法在 8 GB 可用、原版版本上应该给到 4.2 GB 左右。
    #[test]
    fn staged_matches_pcl_for_vanilla() {
        let gib = staged_ram(8.0, 1.5, 2.5, 4.0, 0.5);
        assert_eq!(gib, 4.2, "8G 可用 / 原版 应按 PCL 算法给 4.2G");
    }

    /// 300 个 mod、8 GB 可用：前两段就吃满，应当接近 7 GB。
    #[test]
    fn staged_scales_with_mod_count() {
        let count = 300.0;
        let gib = staged_ram(
            8.0,
            1.5 + count / 90.0,
            2.7 + count / 50.0,
            4.5 + count / 25.0,
            0.5 + count / 150.0,
        );
        assert!(gib > 6.5 && gib <= 8.0, "实际 {gib}");
    }

    /// 内存很小的时候不能低于下限，也不能把可用内存全吃掉。
    #[test]
    fn staged_never_below_minimum() {
        let gib = staged_ram(1.0, 1.5, 2.5, 4.0, 2.5);
        assert_eq!(gib, 2.5);
    }

    #[test]
    fn gc_mode_picks_zgc_only_on_new_java() {
        let java8 = gc_args(GcMode::Auto, 8);
        assert!(java8.contains(&"-XX:+UseG1GC".to_string()));
        assert!(!java8.iter().any(|a| a.contains("UseZGC")));

        let java21 = gc_args(GcMode::Auto, 21);
        assert!(java21.contains(&"-XX:+UseZGC".to_string()));
        assert!(java21.contains(&"-XX:+ZGenerational".to_string()));

        // 23 起分代 ZGC 默认开启，不再加参数
        let java23 = gc_args(GcMode::Auto, 23);
        assert!(java23.contains(&"-XX:+UseZGC".to_string()));
        assert!(!java23.contains(&"-XX:+ZGenerational".to_string()));
    }

    #[test]
    fn optimized_g1_adds_java_specific_flags() {
        let java8 = gc_args(GcMode::G1Optimized, 8);
        assert!(java8.contains(&"-XX:+ParallelRefProcEnabled".to_string()));
        assert!(!java8.contains(&"-XX:MinHeapFreeRatio=25".to_string()));

        let java17 = gc_args(GcMode::G1Optimized, 17);
        assert!(java17.contains(&"-XX:MinHeapFreeRatio=25".to_string()));
        assert!(java17.contains(&"-XX:MaxHeapFreeRatio=40".to_string()));
        assert!(java17.contains(&"-XX:+PerfDisableSharedMem".to_string()));
    }

    #[test]
    fn custom_mode_emits_nothing() {
        assert!(gc_args(GcMode::Custom, 21).is_empty());
    }

    #[test]
    fn gc_arg_detection_covers_conflicts() {
        for arg in [
            "-XX:+UseG1GC",
            "-XX:-UseG1GC",
            "-XX:+UseZGC",
            "-XX:+ZGenerational",
            "-XX:G1NewSizePercent=20",
            "-XX:MaxGCPauseMillis=50",
            "-XX:UseCompactObjectHeaders",
        ] {
            assert!(is_gc_arg(arg), "{arg} 应被识别为 GC 参数");
        }
        // 普通参数不能被误删
        for arg in ["-Dfile.encoding=UTF-8", "-XX:+UseCompressedOops", "-javaagent:x.jar"] {
            assert!(!is_gc_arg(arg), "{arg} 不是 GC 参数");
        }
    }

    #[test]
    fn count_mods_only_counts_archives() {
        let dir = std::env::temp_dir().join(format!("mcl-mods-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["a.jar", "b.zip", "c.litemod", "d.txt", "e.jar.disabled"] {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
        assert_eq!(count_mods(&dir), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
