//! 单文件下载：分片并行 + 断点续传 + 失败换源 + SHA-1 校验。
//!
//! 和 PCL 的 `NetTask` 同思路，只保留用得上的部分：
//! - **按大小自适应拆段**：小于 2 MB 的文件拆开反而更慢（握手占大头）；
//! - **段直写成品文件**：每段 `seek` 到自己的区间写，不再产生 `.partN` 副本，
//!   也就没有"合并"这一趟额外的读 + 写；段进度另存一份小表用于续传；
//! - **段级重试**：某段失败只补那一段，不从零开始；
//! - **源不支持 Range 就整体回退**单连接，不硬撑。
//!
//! 校验和尽量在读取时顺带算：单流边下边算；分片是并发写的，SHA-1 只能顺序算，
//! 所以分片路径下完还要读一遍——但比"合并写一遍 + 校验读一遍"少了一整趟写。
//!
//! **所有整盘读写（SHA-1、复用检查）都走 `spawn_blocking`**：这些是纯阻塞调用，
//! 留在 async 任务里会占住 tokio 的 worker 线程，别的命令（点按钮、切页面）
//! 就得排队等它读完。
use std::collections::HashMap;
use std::io::SeekFrom;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use sha1::{Digest, Sha1};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};

use crate::engine::net::{self, Errored, FailureKind, RetryPolicy};
use crate::error::{Error, Result};

/// 进度回调。用 `Fn` 而不是 `FnMut`，是为了分片并发时能被多段同时调用。
pub type Progress<'a> = &'a (dyn Fn(u64, u64) + Send + Sync);

/// 小于这个大小的文件不拆段。
///
/// 2 MB 是个折中：更小的文件拆段的收益抵不过多一次请求的开销，
/// 而 4 MB 时很多"中等"文件（mod 常见 1–5 MB）会白白单线程跑。
const SPLIT_MIN: u64 = 2 * 1024 * 1024;
/// 每段大约多少字节。
const CHUNK: u64 = 4 * 1024 * 1024;
/// 最多拆几段。
///
/// 16 段能明显吃满 BMCLAPI 的单连接限速；再多容易触发对端限流，
/// 而且并发写同一个文件的寻道开销开始变得可观。
const MAX_SEGMENTS: usize = 16;

/// 分片进度多久落一次盘。
///
/// 段是并发写的，文件本身一开始就按最终大小预分配，靠长度看不出下到哪了，
/// 所以另存一份"每段写了多少"。挂在每个 chunk 上写太费，5 秒一次足够：
/// 真被杀进程，最多重下 5 秒的数据。
const PROGRESS_FLUSH: Duration = Duration::from_secs(5);

/// Range 支持探测结果，按主机缓存。
///
/// 不缓存的话每个待分片的文件都要先发一次 `bytes=0-0` 探测，
/// 一个 300 mod 的整合包就是几百次多余的往返。
static RANGE_OK: OnceLock<Mutex<HashMap<String, bool>>> = OnceLock::new();

fn range_ok() -> &'static Mutex<HashMap<String, bool>> {
    RANGE_OK.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 十六进制摘要（小写）。
pub fn hex(digest: &[u8]) -> String {
    const TABLE: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push(TABLE[(byte >> 4) as usize] as char);
        out.push(TABLE[(byte & 0x0f) as usize] as char);
    }
    out
}

/// 计算文件 SHA-1；文件不存在返回 `Ok(None)`。
pub fn sha1_of_file(path: &Path) -> Result<Option<String>> {
    use std::io::Read;
    if !path.is_file() {
        return Ok(None);
    }
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha1::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(Some(hex(&hasher.finalize())))
}

/// 整盘读 + SHA-1 是纯阻塞 I/O，挪到 blocking 池执行。
///
/// 直接 `await` 一个 `spawn_blocking` 的结果，调用方的写法不用变。
async fn sha1_in_pool(path: &Path) -> Result<Option<String>> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || sha1_of_file(&path))
        .await
        .map_err(|e| Error::msg(format!("校验任务异常：{e}")))?
}

/// 已存在的文件能否直接复用：大小对上，并且（有哈希时）哈希对上。
///
/// 这里刻意**不做**「mtime + size 命中就跳过哈希」的缓存：Windows 上文件被
/// 快速改写时修改时间可能完全不变（写入时间戳是延迟更新的），一旦命中就会把
/// 被改坏的文件当成好的放过去。一次原版安装的整盘校验摊到 blocking 池里
/// 也就几百毫秒，不值得拿正确性换。
fn reusable(dest: &Path, sha1: Option<&str>, size: Option<u64>) -> bool {
    let Ok(meta) = std::fs::metadata(dest) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    if let Some(want) = size {
        if meta.len() != want {
            return false;
        }
    }
    match sha1 {
        None => true,
        Some(want) => matches!(sha1_of_file(dest), Ok(Some(real)) if real.eq_ignore_ascii_case(want)),
    }
}

/// 复用检查同样要读盘，放 blocking 池里，别占 async worker。
async fn reusable_in_pool(dest: &Path, sha1: Option<&str>, size: Option<u64>) -> bool {
    let dest = dest.to_path_buf();
    let sha1 = sha1.map(str::to_string);
    tokio::task::spawn_blocking(move || reusable(&dest, sha1.as_deref(), size))
        .await
        .unwrap_or(false)
}

/// 文件名工具：所有临时文件都挂在目标文件旁边，便于一起清理。
struct Names {
    file: String,
}

impl Names {
    fn new(dest: &Path) -> Self {
        Self {
            file: dest
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "download".to_string()),
        }
    }

    /// 旧版遗留的分段临时文件（`.partN`）。已经不再产生，但要负责清理。
    fn part(&self, parent: &Path, index: usize) -> PathBuf {
        parent.join(format!("{}.part{index}", self.file))
    }

    /// 单连接下载的临时文件。
    fn single(&self, parent: &Path) -> PathBuf {
        parent.join(format!("{}.part", self.file))
    }

    /// 分片直写的目标文件：下完直接改名成正式文件，不需要合并副本。
    fn merging(&self, parent: &Path) -> PathBuf {
        parent.join(format!("{}.merging", self.file))
    }

    /// 分片的段进度表（只有分片路径会用）。
    fn merging_state(&self, parent: &Path) -> PathBuf {
        parent.join(format!("{}.merging.json", self.file))
    }
}

/// 下载一个文件。
///
/// - `urls` 是主备地址，第一段失败会换第二个，每轮带退避；
/// - 目标已存在且校验通过 → 直接跳过；
/// - 分片直接写进成品临时文件，失败不会留下半截"正式文件"。
pub async fn fetch_to_file(
    client: &reqwest::Client,
    urls: &[String],
    dest: &Path,
    expected_sha1: Option<&str>,
    expected_size: Option<u64>,
    on_progress: Progress<'_>,
) -> Result<()> {
    if urls.is_empty() {
        return Err(Error::msg("没有可用的下载地址"));
    }
    if reusable_in_pool(dest, expected_sha1, expected_size).await {
        let total = expected_size.unwrap_or(0);
        on_progress(total, total);
        return Ok(());
    }

    let parent = dest
        .parent()
        .ok_or_else(|| Error::msg(format!("目标路径没有父目录：{}", dest.display())))?;
    tokio::fs::create_dir_all(parent).await?;
    let names = Names::new(dest);

    // 1) 大小已知且够大 → 分片并行，每段直写目标文件
    let mut source: Option<PathBuf> = None;
    let mut digest: Option<String> = None;
    if let Some(size) = expected_size.filter(|size| segment_count(*size) > 1) {
        if fetch_segmented(client, urls, parent, &names, size, on_progress)
            .await
            .is_ok()
        {
            let path = names.merging(parent);
            // 段是并发写的，哈希只能顺序算：这里还剩一趟读，
            // 但已经省掉了原来"合并时写一遍"的那趟。没有期望哈希时就别白读。
            if expected_sha1.is_some() {
                digest = sha1_in_pool(&path).await?;
            }
            source = Some(path);
        }
    }

    // 2) 单连接（含断点续传），顺带在流里算哈希
    let source = match source {
        Some(path) => path,
        None => {
            let (path, streamed) =
                fetch_single(client, urls, parent, &names, expected_size, on_progress).await?;
            digest = streamed;
            path
        }
    };

    // 3) 校验
    if let Some(want) = expected_sha1 {
        let actual = match digest {
            Some(digest) => digest,
            None => sha1_in_pool(&source).await?.unwrap_or_default(),
        };
        if !actual.eq_ignore_ascii_case(want) {
            let _ = tokio::fs::remove_file(&source).await;
            clean_parts(parent, &names);
            return Err(Error::Checksum {
                expected: want.to_string(),
                actual,
            });
        }
    } else if let Some(want) = expected_size {
        let got = tokio::fs::metadata(&source)
            .await
            .map(|m| m.len())
            .unwrap_or(0);
        if want != got {
            let _ = tokio::fs::remove_file(&source).await;
            clean_parts(parent, &names);
            return Err(Error::msg(format!(
                "下载不完整：期望 {want} 字节，实际 {got} 字节"
            )));
        }
    }

    // 4) 部署（Windows 上 rename 不能覆盖已有文件）
    if tokio::fs::metadata(dest).await.is_ok() {
        tokio::fs::remove_file(dest).await?;
    }
    tokio::fs::rename(&source, dest).await?;
    clean_parts(parent, &names);
    Ok(())
}

fn clean_parts(parent: &Path, names: &Names) {
    for index in 0..MAX_SEGMENTS {
        let _ = std::fs::remove_file(names.part(parent, index));
    }
    let _ = std::fs::remove_file(names.single(parent));
    let _ = std::fs::remove_file(names.merging(parent));
    let _ = std::fs::remove_file(names.merging_state(parent));
}

/// 文件多大才值得拆几段。
pub fn segment_count(size: u64) -> usize {
    if size < SPLIT_MIN {
        return 1;
    }
    // 用向上取整：2–4 MB 的文件 size / CHUNK == 0，会退化成单连接，
    // 和 SPLIT_MIN（2 MB 起拆段）的本意不符。
    let by_size = size.div_ceil(CHUNK) as usize;
    by_size.clamp(2, MAX_SEGMENTS)
}

/// 把 `[0, size)` 均分成 `count` 段，返回闭区间。
///
/// `count` 会被压到不超过字节数，否则会出现空段（`end < start` 直接下溢）。
pub fn segment_ranges(size: u64, count: usize) -> Vec<(u64, u64)> {
    let size = size.max(1);
    let count = (count.max(1) as u64).min(size) as usize;
    let chunk = size / count as u64;
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let start = i as u64 * chunk;
        let end = if i + 1 == count {
            size - 1
        } else {
            start + chunk - 1
        };
        out.push((start, end));
    }
    out
}

// ---- 单连接 ----

/// 单连接下载（支持从已有 `.part` 续传），返回内容所在的临时文件和流内算出的哈希。
///
/// 哈希只在"从 0 开始下"时有效：续传时前半段已经落盘，没法凭空补算，
/// 返回 `None` 交给调用方整盘读一遍。
async fn fetch_single(
    client: &reqwest::Client,
    urls: &[String],
    parent: &Path,
    names: &Names,
    size: Option<u64>,
    on_progress: Progress<'_>,
) -> Result<(PathBuf, Option<String>)> {
    let tmp = names.single(parent);
    let policy = RetryPolicy::default();
    let digest: Mutex<Option<String>> = Mutex::new(None);

    let tmp_for_op = tmp.clone();
    let digest_ref = &digest;
    let result = net::for_each_endpoint(urls, policy, |url| {
        let tmp = tmp_for_op.clone();
        async move {
            match stream_once(client, &url, &tmp, size, on_progress).await {
                Ok(maybe_digest) => {
                    if let Some(found) = maybe_digest {
                        if let Ok(mut slot) = digest_ref.lock() {
                            *slot = Some(found);
                        }
                    }
                    Ok(())
                }
                Err((kind, message)) => Err(Errored::new(kind, message)),
            }
        }
    })
    .await;

    match result {
        Ok(()) => {
            let found = digest
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            // 失败时保留 .part：下次进来还能续传
            Ok((tmp, found))
        }
        Err(e) => Err(e),
    }
}

/// 一次单连接尝试：从 `tmp` 现有长度处续传（服务器支持 Range 的话）。
async fn stream_once(
    client: &reqwest::Client,
    url: &str,
    tmp: &Path,
    size: Option<u64>,
    on_progress: Progress<'_>,
) -> std::result::Result<Option<String>, (FailureKind, String)> {
    let already = std::fs::metadata(tmp).map(|m| m.len()).unwrap_or(0);
    if let Some(total) = size {
        if already == total && total > 0 {
            on_progress(total, total);
            return Ok(None);
        }
        if already > total {
            // 上次留下的 .part 比目标还大，只能扔了重来
            let _ = std::fs::remove_file(tmp);
        }
    }
    let already = if size.map(|t| already <= t).unwrap_or(true) {
        already
    } else {
        0
    };

    let mut request = client.get(url);
    if already > 0 {
        request = request.header(reqwest::header::RANGE, format!("bytes={already}-"));
    }
    let response = request
        .send()
        .await
        .map_err(|e| (FailureKind::Transient, format!("网络请求失败：{e}")))?;

    let status = response.status();
    if !status.is_success() {
        return Err((
            net::classify_status(status.as_u16()),
            format!("HTTP {status}（{url}）"),
        ));
    }

    // 服务器不认 Range 会重发整个文件，那就得从头写
    let appending = already > 0 && status.as_u16() == 206;
    let total = response
        .content_length()
        .map(|len| if appending { len + already } else { len })
        .or(size)
        .unwrap_or(0);

    let mut file = if appending {
        tokio::fs::OpenOptions::new()
            .append(true)
            .open(tmp)
            .await
            .map_err(|e| (FailureKind::Transient, format!("打开临时文件失败：{e}")))?
    } else {
        tokio::fs::File::create(tmp)
            .await
            .map_err(|e| (FailureKind::Transient, format!("创建临时文件失败：{e}")))?
    };

    // 从 0 写才有完整的内容，才能边下边算哈希
    let mut hasher = if appending { None } else { Some(Sha1::new()) };
    let mut written = if appending { already } else { 0 };
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let bytes = chunk.map_err(|e| (FailureKind::Transient, format!("传输中断：{e}")))?;
        if let Some(hasher) = hasher.as_mut() {
            hasher.update(&bytes);
        }
        file.write_all(&bytes)
            .await
            .map_err(|e| (FailureKind::Transient, format!("写入失败：{e}")))?;
        written += bytes.len() as u64;
        on_progress(written, total);
    }
    file.flush()
        .await
        .map_err(|e| (FailureKind::Transient, format!("刷新失败：{e}")))?;
    // 刷到磁盘再改名，避免断电后"名字对了内容没落盘"
    file.sync_all()
        .await
        .map_err(|e| (FailureKind::Transient, format!("落盘失败：{e}")))?;
    drop(file);

    if let Some(total) = size {
        if written < total {
            return Err((
                FailureKind::Corrupt,
                format!("下载不完整：期望 {total} 字节，实际 {written} 字节"),
            ));
        }
    }
    Ok(hasher.map(|hasher| hex(&hasher.finalize())))
}

// ---- 分片 ----

/// 取 URL 的主机名，作为 Range 支持情况的缓存键。
fn host_of(url: &str) -> String {
    url.split("://")
        .nth(1)
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or(url)
        .to_ascii_lowercase()
}

/// 探测源是否支持 Range（结果按主机缓存）。
///
/// 只缓存**确定的**答案：206 = 支持，200 = 不支持。网络错误、403、429、5xx
/// 都说明不了这个源到底支不支持 Range，一旦把 `false` 记进去，整个进程生命周期
/// 里这个主机的大文件都会退化成单连接——一次抖动换掉一整轮的分片并发。
async fn supports_range(client: &reqwest::Client, url: &str) -> bool {
    let host = host_of(url);
    if let Some(known) = range_ok()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&host)
        .copied()
    {
        return known;
    }
    let verdict = match client
        .get(url)
        .header(reqwest::header::RANGE, "bytes=0-0")
        .send()
        .await
    {
        Ok(resp) => match resp.status().as_u16() {
            206 => Some(true),
            200 => Some(false),
            _ => None,
        },
        Err(_) => None,
    };
    match verdict {
        Some(ok) => {
            range_ok()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(host, ok);
            ok
        }
        // 没有结论：这次按"不支持"处理（回退单连接），但别写进缓存
        None => false,
    }
}

/// 分片的段进度表。
///
/// 段是并发写进同一个文件的，文件长度看不出进度（一开始就按最终大小预分配），
/// 所以每段自己记"写了多少"，隔一段时间落一次盘。
struct SegmentProgress {
    file: PathBuf,
    offsets: Mutex<Vec<u64>>,
    saved_at: Mutex<Instant>,
}

impl SegmentProgress {
    /// 读回上次的进度；目标文件不存在、大小对不上、进度表坏了，一律从零开始。
    fn load(target: &Path, state: &Path, ranges: &[(u64, u64)]) -> Self {
        let mut offsets = vec![0u64; ranges.len()];
        let expect = target
            .metadata()
            .map(|m| m.len())
            .ok()
            .and_then(|len| ranges.last().map(|(_, end)| (len, end + 1)))
            .filter(|(len, total)| len == total);
        if expect.is_some() {
            if let Ok(text) = std::fs::read_to_string(state) {
                if let Ok(saved) = serde_json::from_str::<Vec<u64>>(&text) {
                    if saved.len() == ranges.len() {
                        for (index, (start, end)) in ranges.iter().enumerate() {
                            offsets[index] = saved[index].min(end - start + 1);
                        }
                    }
                }
            }
        }
        Self {
            file: state.to_path_buf(),
            offsets: Mutex::new(offsets),
            saved_at: Mutex::new(Instant::now()),
        }
    }

    fn get(&self, index: usize) -> u64 {
        self.offsets
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(index)
            .copied()
            .unwrap_or(0)
    }

    fn add(&self, index: usize, bytes: u64) {
        if let Some(slot) = self
            .offsets
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_mut(index)
        {
            *slot += bytes;
        }
    }

    fn done_total(&self) -> u64 {
        self.offsets
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .sum()
    }

    /// 距上次落盘超过 [`PROGRESS_FLUSH`] 才写，不要在 chunk 循环里每次都落。
    fn maybe_save(&self) {
        {
            let mut at = self.saved_at.lock().unwrap_or_else(|e| e.into_inner());
            if at.elapsed() < PROGRESS_FLUSH {
                return;
            }
            *at = Instant::now();
        }
        self.save();
    }

    fn save(&self) {
        let offsets = self
            .offsets
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if let Ok(text) = serde_json::to_string(&offsets) {
            let _ = std::fs::write(&self.file, text);
        }
    }
}

/// 分片下载：每段 `seek` 到自己的区间直接写目标文件。
///
/// 关键差别是**没有合并步骤**——旧实现把 16 个 `.partN` 再顺序拼一遍，
/// 对一个 300 MB 的文件等于多写 300 MB，HDD 上这一趟就是好几秒。
async fn fetch_segmented(
    client: &reqwest::Client,
    urls: &[String],
    parent: &Path,
    names: &Names,
    size: u64,
    on_progress: Progress<'_>,
) -> Result<()> {
    // 先确认地址支持分段，否则后面的活都白干
    if !supports_range(client, &urls[0]).await {
        return Err(Error::msg("源不支持 Range，回退单连接"));
    }

    let ranges = segment_ranges(size, segment_count(size));
    let target = names.merging(parent);
    let progress = Arc::new(SegmentProgress::load(
        &target,
        &names.merging_state(parent),
        &ranges,
    ));
    let resumed = progress.done_total();

    // 预分配成最终大小：之后每段随便 seek，不用等别人写完
    {
        let file = tokio::fs::OpenOptions::new()
            .create(true)
            .truncate(resumed == 0)
            .write(true)
            .open(&target)
            .await?;
        file.set_len(size).await?;
    }
    if resumed > 0 {
        on_progress(resumed, size);
    }

    let done = Arc::new(AtomicU64::new(resumed));
    let policy = RetryPolicy::default();

    let mut futures = Vec::with_capacity(ranges.len());
    for (index, (start, end)) in ranges.iter().copied().enumerate() {
        let progress = Arc::clone(&progress);
        let done = Arc::clone(&done);
        let target = target.clone();
        futures.push(async move {
            let want = end - start + 1;
            if progress.get(index) >= want {
                return Ok::<(), Error>(());
            }
            net::for_each_endpoint(urls, policy, |url| {
                let target = target.clone();
                let progress = Arc::clone(&progress);
                let done = Arc::clone(&done);
                async move {
                    if progress.get(index) >= want {
                        return Ok(());
                    }
                    download_range(
                        client,
                        &url,
                        start,
                        end,
                        index,
                        &target,
                        &progress,
                        &done,
                        size,
                        on_progress,
                    )
                    .await
                    .map_err(|(kind, message)| Errored::new(kind, message))
                }
            })
            .await
        });
    }
    let results = futures_util::future::join_all(futures).await;
    // 出错也要落一次：这样重试能接着下，而不是从头再来。
    // **注意别在这里 clean_parts**：那会把 `.merging` 和段进度一起删掉，
    // 一个 300 MB 的文件下到 99% 断网就得从 0 重来——段级续传就白做了。
    progress.save();
    if let Some(Err(e)) = results.into_iter().find(|r| r.is_err()) {
        return Err(e);
    }
    let got = done.load(Ordering::SeqCst);
    if got < size {
        return Err(Error::msg(format!(
            "分片下载不完整：期望 {size} 字节，实际 {got} 字节"
        )));
    }
    on_progress(size, size);
    Ok(())
}

/// 下某一段（`start` 是段的起点，实际从这里"已经写了多少"之后开始下）。
///
/// 重试时不清零：进度表记到哪就从哪继续，只补剩下的部分。
#[allow(clippy::too_many_arguments)]
async fn download_range(
    client: &reqwest::Client,
    url: &str,
    start: u64,
    end: u64,
    index: usize,
    target: &Path,
    progress: &SegmentProgress,
    done: &AtomicU64,
    size: u64,
    on_progress: Progress<'_>,
) -> std::result::Result<(), (FailureKind, String)> {
    let from = start + progress.get(index);
    if from > end {
        return Ok(());
    }

    let response = client
        .get(url)
        .header(reqwest::header::RANGE, format!("bytes={from}-{end}"))
        .send()
        .await
        .map_err(|e| (FailureKind::Transient, format!("网络请求失败：{e}")))?;

    let status = response.status();
    if status.as_u16() != 206 {
        // 200 说明服务器无视了 Range，分段方案不成立
        let kind = if status.is_success() {
            FailureKind::Corrupt
        } else {
            net::classify_status(status.as_u16())
        };
        return Err((kind, format!("分段请求失败：HTTP {status}（{url}）")));
    }

    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .open(target)
        .await
        .map_err(|e| (FailureKind::Transient, format!("打开目标文件失败：{e}")))?;
    file.seek(SeekFrom::Start(from))
        .await
        .map_err(|e| (FailureKind::Transient, format!("定位写入位置失败：{e}")))?;

    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let bytes = chunk.map_err(|e| (FailureKind::Transient, format!("传输中断：{e}")))?;
        file.write_all(&bytes)
            .await
            .map_err(|e| (FailureKind::Transient, format!("写入失败：{e}")))?;
        progress.add(index, bytes.len() as u64);
        progress.maybe_save();
        let now = done.fetch_add(bytes.len() as u64, Ordering::SeqCst) + bytes.len() as u64;
        on_progress(now, size);
    }
    file.flush()
        .await
        .map_err(|e| (FailureKind::Transient, format!("刷新失败：{e}")))?;
    file.sync_all()
        .await
        .map_err(|e| (FailureKind::Transient, format!("落盘失败：{e}")))?;
    drop(file);

    // 段长不对（被中间设备截断之类），按内容错误处理
    let want = end - start + 1;
    let got = progress.get(index);
    if got < want {
        return Err((
            FailureKind::Corrupt,
            format!("分段长度不符：期望 {want} 字节，实际 {got} 字节"),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::Arc;

    /// 一个只认 GET 的极简 HTTP 服务，用来验证分段/续传真的按 Range 走。
    ///
    /// 返回 (地址, 收到的 Range 头)。`support_range` 为假时永远回 200，
    /// 用来验证"源不支持 Range → 回退单连接"。
    fn serve(payload: Arc<Vec<u8>>, support_range: bool) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_by_server = Arc::clone(&seen);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let payload = Arc::clone(&payload);
                let seen = Arc::clone(&seen_by_server);
                std::thread::spawn(move || {
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut request = String::new();
                    if reader.read_line(&mut request).is_err() {
                        return;
                    }
                    let mut range: Option<(usize, usize)> = None;
                    loop {
                        let mut header = String::new();
                        if reader.read_line(&mut header).unwrap_or(0) == 0 {
                            break;
                        }
                        let header = header.trim_end().to_string();
                        if header.is_empty() {
                            break;
                        }
                        if let Some(rest) = header.to_ascii_lowercase().strip_prefix("range: bytes=")
                        {
                            seen.lock().unwrap().push(rest.to_string());
                            if let Some((a, b)) = rest.split_once('-') {
                                let start: usize = a.trim().parse().unwrap_or(0);
                                let end: usize = b
                                    .trim()
                                    .parse()
                                    .unwrap_or(payload.len().saturating_sub(1))
                                    .min(payload.len().saturating_sub(1));
                                range = Some((start, end));
                            }
                        }
                    }
                    let (status, body, extra) = match (support_range, range) {
                        (true, Some((start, end))) => {
                            let body = &payload[start..=end];
                            (
                                "206 Partial Content",
                                body,
                                format!(
                                    "Content-Range: bytes {start}-{end}/{}\r\n",
                                    payload.len()
                                ),
                            )
                        }
                        _ => ("200 OK", &payload[..], String::new()),
                    };
                    let head = format!(
                        "HTTP/1.1 {status}\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(head.as_bytes());
                    let _ = stream.write_all(body);
                    let _ = stream.flush();
                });
            }
        });
        (format!("http://{addr}/file.bin"), seen)
    }

    fn payload(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i % 251) as u8).collect()
    }

    /// 只对 `bytes=0-0` 探测回 206、真正的分段请求一律 500 的源。
    ///
    /// 用来验证"分片失败后必须保住续传状态"。
    fn serve_broken_ranges() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                std::thread::spawn(move || {
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut request = String::new();
                    let _ = reader.read_line(&mut request);
                    let mut probe = false;
                    loop {
                        let mut header = String::new();
                        if reader.read_line(&mut header).unwrap_or(0) == 0 {
                            break;
                        }
                        let header = header.trim_end().to_string();
                        if header.is_empty() {
                            break;
                        }
                        if let Some(rest) =
                            header.to_ascii_lowercase().strip_prefix("range: bytes=")
                        {
                            probe = rest.trim() == "0-0";
                        }
                    }
                    let response: &[u8] = if probe {
                        b"HTTP/1.1 206 Partial Content\r\nContent-Length: 1\r\nContent-Range: bytes 0-0/3145728\r\nConnection: close\r\n\r\n\0"
                    } else {
                        b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    };
                    let _ = stream.write_all(response);
                    let _ = stream.flush();
                });
            }
        });
        format!("http://{addr}/file.bin")
    }

    fn digest_of(bytes: &[u8]) -> String {
        let mut hasher = Sha1::new();
        hasher.update(bytes);
        hex(&hasher.finalize())
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mcl-dl-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn client() -> reqwest::Client {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .build()
            .unwrap()
    }

    /// 3 MB → 会被拆成 2 段：验证"每段 seek 到自己的区间写"能拼出正确内容。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn segmented_download_lands_the_right_bytes() {
        let bytes = payload(3 * 1024 * 1024 + 12345);
        let want = digest_of(&bytes);
        let size = bytes.len() as u64;
        let (url, seen) = serve(Arc::new(bytes.clone()), true);
        let dir = temp_dir("seg");

        let dest = dir.join("client.jar");
        let on_progress = |_: u64, _: u64| {};
        fetch_to_file(
            &client(),
            &[url],
            &dest,
            Some(&want),
            Some(size),
            &on_progress,
        )
        .await
        .unwrap();

        assert_eq!(size, std::fs::metadata(&dest).unwrap().len());
        assert_eq!(std::fs::read(&dest).unwrap(), bytes);
        // 没有留下任何临时文件
        assert!(!dir.join("client.jar.merging").exists());
        assert!(!dir.join("client.jar.merging.json").exists());
        assert!(!dir.join("client.jar.part").exists());
        // 探测一次 + 两段各一次
        assert_eq!(seen.lock().unwrap().len(), 3, "{:?}", seen.lock().unwrap());
    }

    /// 预置一份"下到一半"的段进度：续传必须从记录的位置接着要，而不是从 0 重下。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn segmented_download_resumes_from_saved_offsets() {
        let bytes = payload(3 * 1024 * 1024 + 12345);
        let want = digest_of(&bytes);
        let size = bytes.len() as u64;
        let (url, seen) = serve(Arc::new(bytes.clone()), true);
        let dir = temp_dir("resume");
        let dest = dir.join("client.jar");

        // 按真实尺寸预分配，并把第 0 段的前 1000 字节写进去
        let merging = dir.join("client.jar.merging");
        std::fs::write(&merging, vec![0u8; bytes.len()]).unwrap();
        {
            let mut file = std::fs::OpenOptions::new().write(true).open(&merging).unwrap();
            use std::io::{Seek, SeekFrom, Write as _};
            file.seek(SeekFrom::Start(0)).unwrap();
            file.write_all(&bytes[..1000]).unwrap();
        }
        std::fs::write(dir.join("client.jar.merging.json"), "[1000,0]").unwrap();

        let on_progress = |_: u64, _: u64| {};
        fetch_to_file(
            &client(),
            &[url],
            &dest,
            Some(&want),
            Some(size),
            &on_progress,
        )
        .await
        .unwrap();

        assert_eq!(std::fs::read(&dest).unwrap(), bytes);
        let seen = seen.lock().unwrap();
        assert!(
            seen.iter().any(|range| range.starts_with("1000-")),
            "续传应从第 1000 字节开始，实际：{seen:?}"
        );
    }

    /// 源不支持 Range 时回退单连接，并且边下边算的哈希要能直接通过校验。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn single_connection_hashes_while_streaming() {
        let bytes = payload(1024 * 1024 + 7);
        let want = digest_of(&bytes);
        let size = bytes.len() as u64;
        let (url, _seen) = serve(Arc::new(bytes.clone()), false);
        let dir = temp_dir("single");

        let dest = dir.join("lib.jar");
        let on_progress = |_: u64, _: u64| {};
        fetch_to_file(
            &client(),
            &[url],
            &dest,
            Some(&want),
            Some(size),
            &on_progress,
        )
        .await
        .unwrap();

        assert_eq!(std::fs::read(&dest).unwrap(), bytes);
        // 已经校验通过的文件第二次应该直接复用（不再发请求）
        let dead = "http://127.0.0.1:1/file.bin".to_string();
        fetch_to_file(&client(), &[dead], &dest, Some(&want), Some(size), &on_progress)
            .await
            .unwrap();
    }

    /// 哈希不符时必须删掉临时文件并报 Checksum，不能留个坏文件当成品。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn mismatched_hash_cleans_up() {
        let bytes = payload(3 * 1024 * 1024);
        let size = bytes.len() as u64;
        let (url, _seen) = serve(Arc::new(bytes), true);
        let dir = temp_dir("bad");

        let dest = dir.join("client.jar");
        let on_progress = |_: u64, _: u64| {};
        let err = fetch_to_file(
            &client(),
            &[url],
            &dest,
            Some(&"0".repeat(40)),
            Some(size),
            &on_progress,
        )
        .await
        .unwrap_err();

        assert!(matches!(err, Error::Checksum { .. }), "{err}");
        assert!(!dest.exists());
        assert!(!dir.join("client.jar.merging").exists());
    }

    /// 段级续传的进度表不能在失败时被删掉：否则下到 99% 断网就得从 0 重来。
    ///
    /// 这个"坏源"只对探测请求回 206，真正的分段请求全部 500，
    /// 正好复现"分片路径失败"。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn failed_segmented_download_keeps_resume_state() {
        let url = serve_broken_ranges();
        let dir = temp_dir("keep");
        let dest = dir.join("client.jar");
        let size = 3 * 1024 * 1024u64;
        let on_progress = |_: u64, _: u64| {};

        let err = fetch_to_file(
            &client(),
            &[url],
            &dest,
            Some(&"0".repeat(40)),
            Some(size),
            &on_progress,
        )
        .await
        .unwrap_err();
        assert!(!err.to_string().is_empty());
        assert!(!dest.exists(), "失败时不能留下正式文件");
        assert!(
            dir.join("client.jar.merging").exists(),
            "分片文件要留着，下次接着下"
        );
        assert!(
            dir.join("client.jar.merging.json").exists(),
            "段进度要留着，否则续传等于重下"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hex_encodes_lowercase() {
        assert_eq!(hex(&[0x00, 0x0f, 0xa9, 0xff]), "000fa9ff");
    }

    /// 真实网络烟测：BMCLAPI 的客户端 jar 端点（20 MB 级别，必然走分片直写）。
    /// 默认 ignore，手动跑：
    /// `cargo test --lib real_network -- --ignored --nocapture`
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[ignore]
    async fn real_network_segmented_download() {
        let url = "https://bmclapi2.bangbang93.com/version/1.20.1/client".to_string();
        let client = client();
        // 分片要按大小算区间。注意别用 HEAD：BMCLAPI 的 HEAD 不给 Content-Length，
        // 拿 Range 探测的 Content-Range 才靠得住。
        let probe = client
            .get(&url)
            .header(reqwest::header::RANGE, "bytes=0-0")
            .send()
            .await
            .unwrap();
        assert_eq!(probe.status().as_u16(), 206, "镜像应支持 Range");
        let content_range = probe
            .headers()
            .get(reqwest::header::CONTENT_RANGE)
            .expect("206 应带 Content-Range")
            .to_str()
            .unwrap()
            .to_string();
        let size: u64 = content_range
            .rsplit('/')
            .next()
            .and_then(|n| n.parse().ok())
            .expect("Content-Range 里应带总长");
        let dir = temp_dir("net");
        let dest = dir.join("1.20.1.jar");
        let on_progress = |_: u64, _: u64| {};
        fetch_to_file(&client, &[url], &dest, None, Some(size), &on_progress)
            .await
            .unwrap();
        let len = std::fs::metadata(&dest).unwrap().len();
        println!("下载完成：{} MB", len / 1048576);
        assert_eq!(len, size, "落盘大小应该和 Content-Length 一致");
        let content = std::fs::read(&dest).unwrap();
        assert_eq!(&content[..2], b"PK", "下回来的不是 zip");
        assert!(!dir.join("1.20.1.jar.merging").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sha1_matches_known_vector() {
        let dir = std::env::temp_dir().join(format!("mcl-sha1-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("abc.txt");
        std::fs::write(&file, b"abc").unwrap();
        assert_eq!(
            sha1_of_file(&file).unwrap().as_deref(),
            Some("a9993e364706816aba3e25717850c26c9cd0d89d")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_file_has_no_digest() {
        let path = std::env::temp_dir().join("mcl-definitely-missing-file");
        let _ = std::fs::remove_file(&path);
        assert!(sha1_of_file(&path).unwrap().is_none());
    }

    #[test]
    fn small_files_are_not_split() {
        assert_eq!(segment_count(0), 1);
        assert_eq!(segment_count(1024), 1);
        // 低于 SPLIT_MIN（2 MB）一律单线程
        assert_eq!(segment_count(1024 * 1024), 1);
        // 刚过 SPLIT_MIN 但不到一个 CHUNK，按 2 段拆（不能退化成单连接）
        assert_eq!(segment_count(3 * 1024 * 1024), 2);
        // 20 MB / 4 MB 每段 = 5 段
        assert_eq!(segment_count(20 * 1024 * 1024), 5);
        // 超大文件封顶 16 段
        assert_eq!(segment_count(500 * 1024 * 1024), 16);
    }

    #[test]
    fn ranges_cover_the_whole_file_without_overlap() {
        let size = 1000u64;
        let ranges = segment_ranges(size, 4);
        assert_eq!(ranges.first().unwrap().0, 0);
        assert_eq!(ranges.last().unwrap().1, size - 1);
        let covered: u64 = ranges.iter().map(|(s, e)| e - s + 1).sum();
        assert_eq!(covered, size);
        for pair in ranges.windows(2) {
            assert_eq!(pair[0].1 + 1, pair[1].0);
        }
    }

    #[test]
    fn ranges_handle_tiny_and_single() {
        assert_eq!(segment_ranges(10, 1), vec![(0, 9)]);
        // 分段数超过字节数时不能出现空段或下溢
        let ranges = segment_ranges(3, 5);
        assert_eq!(ranges.len(), 3);
        assert_eq!(ranges.last().unwrap().1, 2);
        for (start, end) in ranges {
            assert!(start <= end);
        }
    }

    #[test]
    fn reusable_checks_size_and_hash() {
        let dir = std::env::temp_dir().join(format!("mcl-reuse-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.bin");
        std::fs::write(&file, b"abc").unwrap();

        assert!(reusable(&file, None, Some(3)));
        assert!(!reusable(&file, None, Some(4)));
        assert!(reusable(
            &file,
            Some("a9993e364706816aba3e25717850c26c9cd0d89d"),
            Some(3)
        ));
        assert!(!reusable(&file, Some(&"0".repeat(40)), Some(3)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn segment_progress_round_trips_and_clamps() {
        let dir = std::env::temp_dir().join(format!("mcl-seg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("x.bin");
        let state = dir.join("x.bin.json");
        std::fs::write(&target, vec![0u8; 100]).unwrap();
        std::fs::write(&state, "[1,999]").unwrap();

        let ranges = vec![(0u64, 49u64), (50, 99)];
        let progress = SegmentProgress::load(&target, &state, &ranges);
        assert_eq!(progress.get(0), 1);
        // 超出段长的进度要被夹住，否则续传起点会跑到别人区间里
        assert_eq!(progress.get(1), 50);
        assert_eq!(progress.done_total(), 51);

        progress.add(0, 5);
        progress.save();
        let again = SegmentProgress::load(&target, &state, &ranges);
        assert_eq!(again.get(0), 6);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn segment_progress_is_discarded_when_target_size_mismatches() {
        let dir = std::env::temp_dir().join(format!("mcl-seg2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("y.bin");
        let state = dir.join("y.bin.json");
        std::fs::write(&target, vec![0u8; 10]).unwrap();
        std::fs::write(&state, "[5,5]").unwrap();

        let ranges = vec![(0u64, 49u64), (50, 99)];
        let progress = SegmentProgress::load(&target, &state, &ranges);
        assert_eq!(progress.done_total(), 0, "长度对不上必须重下");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn host_is_lowercased_and_stripped() {
        assert_eq!(host_of("https://BMCLAPI.example.com/a/b"), "bmclapi.example.com");
        assert_eq!(host_of("http://127.0.0.1:8080/x"), "127.0.0.1:8080");
    }
}
