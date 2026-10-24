//! 全局下载队列。
//!
//! 所有下载（客户端 jar / 类库 / 资源 / 整合包模组）都走同一个队列，
//! 这样"下载页"只需要渲染一份状态，暂停/继续也是全局生效的。
//!
//! 并发模型：队列状态用 `std::sync::Mutex`（临界区里没有 `.await`），
//! 拉取任务用 `buffer_unordered` 限流，进度通过 Tauri Event 节流推送。
use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio::sync::Notify;

use crate::engine::download;

/// 单个任务的进度事件。
pub const EVENT_PROGRESS: &str = "queue://progress";
/// 队列结构变化（增删、状态迁移）。
pub const EVENT_CHANGED: &str = "queue://changed";

/// 同时下载几个文件。
///
/// assets 是几千个小文件，这类工作量靠"文件级并发"提速最有效；
/// 单个大文件则靠 [`download`] 的分片并发。
/// 10 是个经验值：再高容易触发 BMCLAPI 的限流，反而更慢。
const CONCURRENT_FILES: usize = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ItemState {
    Pending,
    Running,
    Done,
    Failed,
    Cancelled,
}

impl ItemState {
    fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Cancelled)
    }
}

/// 推给前端的队列条目。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueItem {
    pub id: String,
    pub label: String,
    /// `client` / `library` / `native` / `asset` / `mod` / `modpack`
    pub kind: String,
    /// 一次入队批次的分组 ID，前端可折叠显示并整组取消。
    pub group_id: String,
    pub group_label: String,
    pub state: ItemState,
    pub total: u64,
    pub done: u64,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProgressPayload {
    id: String,
    label: String,
    kind: String,
    done: u64,
    total: u64,
}

/// 入队请求。路径由后端算好，前端不参与。
#[derive(Clone, Debug)]
pub struct NewTask {
    pub label: String,
    pub kind: String,
    pub url: String,
    /// 主地址失败后换的备用地址（通常是镜像/官方互换）
    pub fallbacks: Vec<String>,
    pub dest: PathBuf,
    pub sha1: Option<String>,
    pub size: Option<u64>,
    /// 显式指定的任务组（id, 显示名）。
    ///
    /// 一次原版安装要分两批入队（先游戏文件、后资源），不显式指定的话
    /// 每批各成一个组，玩家在下载抽屉里会看到同一次安装被拆成两条。
    /// 安装流程先用 [`Queue::alloc_group`] 领一个组号，再盖章到每条任务上。
    pub group: Option<(String, String)>,
}

impl NewTask {
    pub fn new(
        kind: impl Into<String>,
        label: impl Into<String>,
        url: impl Into<String>,
        dest: PathBuf,
    ) -> Self {
        Self {
            label: label.into(),
            kind: kind.into(),
            url: url.into(),
            fallbacks: Vec::new(),
            dest,
            sha1: None,
            size: None,
            group: None,
        }
    }

    /// 从**官方原始地址**构造：自动配好「当前源优先、另一头兜底」两条路。
    ///
    /// 传原始地址而不是改写后的地址，是因为镜像与官方的域名不是一一对应，
    /// 只有拿得到原始 URL 才能反推出备用那条。
    pub fn from_official(
        kind: impl Into<String>,
        label: impl Into<String>,
        original_url: &str,
        source: &str,
        dest: PathBuf,
    ) -> Self {
        Self::new(kind, label, "", dest).with_source_fallback(original_url, source)
    }

    /// 按当前下载源和官方原始地址，自动配好「镜像 ↔ 官方」两条路。
    pub fn with_source_fallback(mut self, original_url: &str, source: &str) -> Self {
        let list = crate::engine::net::endpoints(original_url, source);
        if let Some(first) = list.first() {
            self.url = first.clone();
            self.fallbacks = list.into_iter().skip(1).collect();
        }
        self
    }

    pub fn with_fallbacks(mut self, urls: Vec<String>) -> Self {
        self.fallbacks = urls;
        self
    }

    pub fn with_sha1(mut self, sha1: Option<String>) -> Self {
        self.sha1 = sha1;
        self
    }

    pub fn with_size(mut self, size: Option<u64>) -> Self {
        self.size = size;
        self
    }

    /// 盖章到指定的任务组（跨批次共用一个组时用）。
    pub fn set_group(&mut self, group_id: &str, group_label: &str) {
        self.group = Some((group_id.to_string(), group_label.to_string()));
    }
}

struct Entry {
    id: String,
    label: String,
    kind: String,
    group_id: String,
    group_label: String,
    url: String,
    fallbacks: Vec<String>,
    dest: PathBuf,
    sha1: Option<String>,
    size: Option<u64>,
    state: ItemState,
    done: u64,
    total: u64,
    error: Option<String>,
}

/// `wait()` 用来区分「没下完就被清掉」和「下完后被清掉」的历史长度上限。
const COMPLETED_HISTORY_CAP: usize = 4096;

pub struct Queue {
    app: AppHandle,
    client: reqwest::Client,
    entries: Mutex<Vec<Entry>>,
    /// 已请求取消的任务 ID。
    cancel: Mutex<HashSet<String>>,
    /// 最近真正下完的任务 ID。`clear_finished` 会把条目从表里删掉，
    /// 而 `wait()` 还得告诉安装流程"这条其实是成功的"——不记历史的话，
    /// 安装进行中点一次「清除已完成」，全部下完也会被报成失败。
    completed: Mutex<VecDeque<String>>,
    paused: AtomicBool,
    running: AtomicBool,
    counter: AtomicU64,
    group_counter: AtomicU64,
    source: Mutex<String>,
    /// 是否已经排了一次"合并推送"（见 `emit_changed_coalesced`）
    flush_scheduled: AtomicBool,
    /// 有任务结束时的唤醒信号，`wait()` 靠它即时返回
    finished_signal: Notify,
}

impl Queue {
    pub fn new(app: AppHandle, client: reqwest::Client, source: impl Into<String>) -> Arc<Self> {
        Arc::new(Self {
            app,
            client,
            entries: Mutex::new(Vec::new()),
            cancel: Mutex::new(HashSet::new()),
            completed: Mutex::new(VecDeque::new()),
            paused: AtomicBool::new(false),
            running: AtomicBool::new(false),
            counter: AtomicU64::new(1),
            group_counter: AtomicU64::new(1),
            source: Mutex::new(source.into()),
            flush_scheduled: AtomicBool::new(false),
            finished_signal: Notify::new(),
        })
    }

    fn guard(&self) -> MutexGuard<'_, Vec<Entry>> {
        // 队列是纯内存状态，某次 panic 毒化锁不该让整个下载功能瘫痪。
        self.entries.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn set_source(&self, source: impl Into<String>) {
        *self.source.lock().unwrap_or_else(|e| e.into_inner()) = source.into();
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }

    /// 领一个全局唯一的组号。给"一次业务安装跨多次入队"的场景用：
    /// 安装流程先领号，再盖章到每一批任务上，前端就能把它们折叠成一条。
    pub fn alloc_group(&self) -> String {
        format!("g{}", self.group_counter.fetch_add(1, Ordering::Relaxed))
    }

    /// 入队一批任务，返回它们的 ID。
    pub fn enqueue(self: &Arc<Self>, tasks: Vec<NewTask>) -> Vec<String> {
        if tasks.is_empty() {
            return Vec::new();
        }
        let mut ids = Vec::with_capacity(tasks.len());
        let mut added = 0usize;
        let group_label = batch_label(&tasks);
        let mut batch_group: Option<String> = None;
        {
            let mut entries = self.guard();
            for mut task in tasks {
                // 同一个目标文件已经在下载（或排队）时，直接复用那条任务。
                // 不这样的话，重复点「安装」会让两个任务同时写同一个 jar：
                // 一个 `rename` 覆盖另一个正在写的临时文件，最后得到一个
                // 校验不过、但看起来"下完了"的文件。
                if let Some(existing) = entries.iter().find(|e| {
                    e.dest == task.dest
                        && matches!(e.state, ItemState::Pending | ItemState::Running)
                }) {
                    ids.push(existing.id.clone());
                    continue;
                }
                let id = format!("t{}", self.counter.fetch_add(1, Ordering::Relaxed));
                // 显式指定组的任务用它自己的组号和组名；没指定的才按批次归组
                let (task_group, task_group_label) = match task.group.take() {
                    Some(explicit) => explicit,
                    None => (
                        batch_group
                            .get_or_insert_with(|| {
                                format!("g{}", self.group_counter.fetch_add(1, Ordering::Relaxed))
                            })
                            .clone(),
                        group_label.clone(),
                    ),
                };
                entries.push(Entry {
                    id: id.clone(),
                    label: task.label,
                    kind: task.kind,
                    group_id: task_group,
                    group_label: task_group_label,
                    url: task.url,
                    fallbacks: task.fallbacks,
                    dest: task.dest,
                    sha1: task.sha1,
                    size: task.size,
                    state: ItemState::Pending,
                    done: 0,
                    total: task.size.unwrap_or(0),
                    error: None,
                });
                ids.push(id);
                added += 1;
            }
        }
        // 全是重复任务时队列结构没变，不用推整表
        if added > 0 {
            self.emit_changed();
        }
        self.kick();
        ids
    }

    /// 当前快照，前端首屏渲染用。
    pub fn snapshot(&self) -> Vec<QueueItem> {
        self.guard()
            .iter()
            .map(|e| QueueItem {
                id: e.id.clone(),
                label: e.label.clone(),
                kind: e.kind.clone(),
                group_id: e.group_id.clone(),
                group_label: e.group_label.clone(),
                state: e.state,
                total: e.total,
                done: e.done,
                error: e.error.clone(),
            })
            .collect()
    }

    pub fn pause(&self) {
        self.paused.store(true, Ordering::SeqCst);
        self.emit_changed();
    }

    pub fn resume(self: &Arc<Self>) {
        self.paused.store(false, Ordering::SeqCst);
        self.emit_changed();
        self.kick();
    }

    /// 取消：排队中的立刻取消；正在下载的会在当前文件结束后停止
    /// （HTTP 流已开始，强行中断会留下半截文件，反而更麻烦）。
    pub fn cancel(&self, id: &str) {
        {
            let mut cancel = self.cancel.lock().unwrap_or_else(|e| e.into_inner());
            cancel.insert(id.to_string());
        }
        {
            let mut entries = self.guard();
            if let Some(entry) = entries.iter_mut().find(|e| e.id == id) {
                if entry.state == ItemState::Pending {
                    entry.state = ItemState::Cancelled;
                }
            }
        }
        self.emit_changed();
        // 排队中被取消的任务也已经是终态了，叫一下 wait()，安装流程好立刻收尾
        self.finished_signal.notify_one();
    }

    /// 取消同一批次的所有任务。排队任务立即取消，正在下载的任务在当前文件结束后停止。
    pub fn cancel_group(&self, group_id: &str) {
        let mut cancel = self.cancel.lock().unwrap_or_else(|e| e.into_inner());
        let mut entries = self.guard();
        for entry in entries.iter_mut().filter(|e| e.group_id == group_id) {
            if matches!(entry.state, ItemState::Pending | ItemState::Running) {
                cancel.insert(entry.id.clone());
                if entry.state == ItemState::Pending {
                    entry.state = ItemState::Cancelled;
                }
            }
        }
        drop(entries);
        drop(cancel);
        self.emit_changed();
        self.finished_signal.notify_one();
    }

    /// 重试单个失败任务。下载器会优先复用已下载的临时文件/分片进度。
    pub fn retry_failed(self: &Arc<Self>, id: &str) -> bool {
        let changed = {
            let mut entries = self.guard();
            if let Some(entry) = entries.iter_mut().find(|e| e.id == id && e.state == ItemState::Failed) {
                entry.state = ItemState::Pending;
                entry.error = None;
                entry.done = 0;
                true
            } else {
                false
            }
        };
        if changed {
            self.cancel.lock().unwrap_or_else(|e| e.into_inner()).remove(id);
            self.emit_changed();
            self.kick();
        }
        changed
    }

    /// 清掉已结束的条目。
    pub fn clear_finished(&self) {
        self.guard().retain(|e| !e.state.is_terminal());
        self.emit_changed();
    }

    /// 记下"这个 id 真的下完了"。只增不减（超出上限丢最旧的），
    /// 这样 `wait()` 才能在条目被清掉之后依然给出正确的结论。
    fn remember_completed(&self, id: &str) {
        let mut done = self.completed.lock().unwrap_or_else(|e| e.into_inner());
        done.push_back(id.to_string());
        while done.len() > COMPLETED_HISTORY_CAP {
            done.pop_front();
        }
    }

    fn was_completed(&self, id: &str) -> bool {
        self.completed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .any(|seen| seen == id)
    }

    /// 等一批任务跑完（安装流程需要"下完再解压"）。返回失败的条目。
    ///
    /// 用 `Notify` 等而不是 150ms 轮询：3598 个资源下完的那一刻就返回，
    /// 不用再多等半个心跳；空闲时也不会每秒醒七次去抢一次锁。
    pub async fn wait(&self, ids: &[String]) -> Vec<(String, String)> {
        if ids.is_empty() {
            return Vec::new();
        }
        let deadline = Instant::now() + Duration::from_secs(45 * 60);
        loop {
            // 先注册等待者再检查状态：否则"检查完、还没开始等"这段时间里
            // 恰好完成的通知会被漏掉，只能靠超时兜底。
            let waiter = self.finished_signal.notified();
            {
                let entries = self.guard();
                let finished = ids.iter().all(|id| {
                    entries
                        .iter()
                        .find(|e| &e.id == id)
                        .map(|e| e.state.is_terminal())
                        .unwrap_or(true)
                });
                if finished {
                    // 逐条按 id 找：找不到且不在完成历史里才算失败——条目只会在
                    // 「清空」时消失，而下完的任务也会被用户清掉，不能一概当失败。
                    return ids
                        .iter()
                        .filter_map(|id| match entries.iter().find(|e| &e.id == id) {
                            Some(entry) if entry.state == ItemState::Done => None,
                            Some(entry) => Some((
                                entry.label.clone(),
                                entry
                                    .error
                                    .clone()
                                    .unwrap_or_else(|| "已取消".to_string()),
                            )),
                            None if self.was_completed(id) => None,
                            None => Some((id.clone(), "任务已被清空".to_string())),
                        })
                        .collect();
                }
            }
            if Instant::now() > deadline {
                return vec![("队列".to_string(), "等待下载超时（45 分钟）".to_string())];
            }
            // 1 秒兜底：万一某条路径没发信号，也只多睡一秒，不会卡死
            let _ = tokio::time::timeout(Duration::from_secs(1), waiter).await;
        }
    }

    // ---- 内部 ----

    fn kick(self: &Arc<Self>) {
        // 已经有一个 worker 在跑就不重复拉起
        if self.running.swap(true, Ordering::SeqCst) {
            return;
        }
        let queue = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            queue.work().await;
            queue.running.store(false, Ordering::SeqCst);
            // 收尾期间可能又进了新任务。
            // 暂停时不能拉起：work() 会因「暂停且无正在跑的任务」立即返回，
            // 否则这里就是一轮不停的自旋（spawn → return → 再 spawn）。
            if queue.has_pending() && !queue.is_paused() {
                queue.kick();
            }
        });
    }

    fn has_pending(&self) -> bool {
        self.guard().iter().any(|e| e.state == ItemState::Pending)
    }

    fn has_active(&self) -> bool {
        self.guard().iter().any(|e| e.state == ItemState::Running)
    }

    /// 取最多 `limit` 个待办，同时把它们标成 Running。
    fn take_batch(self: &Arc<Self>, limit: usize) -> Vec<Entry> {
        let mut out = Vec::new();
        {
            let cancelled = self.cancel.lock().unwrap_or_else(|e| e.into_inner()).clone();
            let mut entries = self.guard();
            for entry in entries.iter_mut() {
                if out.len() >= limit {
                    break;
                }
                if entry.state != ItemState::Pending {
                    continue;
                }
                if cancelled.contains(&entry.id) {
                    entry.state = ItemState::Cancelled;
                    continue;
                }
                entry.state = ItemState::Running;
                out.push(Entry { ..clone_entry(entry) });
            }
        }
        if !out.is_empty() {
            // 必须走合并推送：一批 10 个任务，原版安装有 360 批，
            // 每批推两次 3598 条的整表（约 0.5 MB）就是几百 MB 的 IPC。
            self.emit_changed_coalesced();
        }
        out
    }

    async fn work(self: &Arc<Self>) {
        loop {
            // 暂停时原地等；如果连正在跑的都没有，直接退出，等 resume 再 kick。
            while self.is_paused() {
                if !self.has_active() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(150)).await;
            }

            let batch = self.take_batch(CONCURRENT_FILES);
            if batch.is_empty() {
                return;
            }

            let mut futures = Vec::with_capacity(batch.len());
            for entry in batch {
                let queue = Arc::clone(self);
                futures.push(async move {
                    queue.run_one(entry).await;
                });
            }
            futures_util::stream::iter(futures)
                .buffer_unordered(CONCURRENT_FILES)
                .collect::<Vec<()>>()
                .await;
            self.emit_changed_coalesced();
        }
    }

    async fn run_one(self: &Arc<Self>, entry: Entry) {
        // 进度回调要满足 `Fn + Sync`（分片时多个线程同时调），节流状态用 Mutex 存
        let throttle = Mutex::new((Instant::now(), entry.total));
        let on_progress = |done: u64, total: u64| {
            let mut guard = throttle.lock().unwrap_or_else(|e| e.into_inner());
            let last_at = guard.0;
            guard.1 = total;
            // 每个 chunk 都推事件会把 IPC 打满；200ms 对进度条仍足够顺滑，并降低大队列的前端重算频率
            if last_at.elapsed() >= Duration::from_millis(200) {
                guard.0 = Instant::now();
                drop(guard);
                self.update_progress(&entry.id, done, total);
                self.emit_progress(&entry.id, &entry.label, &entry.kind, done, total);
            }
        };

        let mut urls = vec![entry.url.clone()];
        urls.extend(entry.fallbacks.iter().cloned());

        let result = download::fetch_to_file(
            &self.client,
            &urls,
            &entry.dest,
            entry.sha1.as_deref(),
            entry.size,
            &on_progress,
        )
        .await;

        let last_total = throttle.lock().unwrap_or_else(|e| e.into_inner()).1;

        match result {
            Ok(()) => {
                self.remember_completed(&entry.id);
                self.finish(&entry.id, ItemState::Done, None, last_total);
            }
            Err(e) => {
                let cancelled = self
                    .cancel
                    .lock()
                    .unwrap_or_else(|x| x.into_inner())
                    .contains(&entry.id);
                if cancelled {
                    self.finish(&entry.id, ItemState::Cancelled, None, last_total);
                } else {
                    self.finish(
                        &entry.id,
                        ItemState::Failed,
                        Some(friendly_download_error(&e.to_string())),
                        last_total,
                    );
                }
            }
        }
    }

    fn update_progress(&self, id: &str, done: u64, total: u64) {
        let mut entries = self.guard();
        if let Some(entry) = entries.iter_mut().find(|e| e.id == id) {
            entry.done = done;
            if total > 0 {
                entry.total = total;
            }
        }
    }

    fn finish(self: &Arc<Self>, id: &str, state: ItemState, error: Option<String>, total: u64) {
        {
            let mut entries = self.guard();
            if let Some(entry) = entries.iter_mut().find(|e| e.id == id) {
                entry.state = state;
                entry.error = error;
                if total > 0 {
                    entry.total = total;
                }
                // 只有真正下完才把进度顶到 100%，失败/取消的条目保留实际字节数，
                // 否则队列抽屉里失败项会显示一条满格的红色进度条。
                if state == ItemState::Done {
                    if entry.total == 0 {
                        entry.total = entry.done;
                    }
                    entry.done = entry.total;
                }
            }
        }
        self.emit_changed_coalesced();
        // 叫醒 wait()：`notify_one` 在没有等待者时会存一个许可，
        // 所以"刚检查完状态就完成"这种时序也不会漏掉通知。
        self.finished_signal.notify_one();
    }

    /// 完成一个任务时的推送：**合并**，不是每次都整表推。
    ///
    /// 原版 1.20.1 的安装会入队 3598 个资源任务，一个接一个完成；每完成一个
    /// 就 `snapshot()` 整表推一次的话，就是 3598 次 × 约 0.5 MB 的 JSON，
    /// 前端还得把整表重新包一遍响应式对象——界面就是这样被拖住的。
    /// 这里改成"250ms 内最多推一次"：第一个完成的排一次定时推送，
    /// 这期间完成的都折叠进同一次。
    fn emit_changed_coalesced(self: &Arc<Self>) {
        if self.flush_scheduled.swap(true, Ordering::SeqCst) {
            return;
        }
        let queue = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_millis(250)).await;
            queue.flush_scheduled.store(false, Ordering::SeqCst);
            queue.emit_changed();
        });
    }

    fn emit_changed(&self) {
        let _ = self.app.emit(EVENT_CHANGED, self.snapshot());
    }

    fn emit_progress(&self, id: &str, label: &str, kind: &str, done: u64, total: u64) {
        let _ = self.app.emit(
            EVENT_PROGRESS,
            ProgressPayload {
                id: id.to_string(),
                label: label.to_string(),
                kind: kind.to_string(),
                done,
                total,
            },
        );
    }
}

/// 将底层错误和用户可执行的处理建议合并显示，原始错误仍保留用于排查。
fn friendly_download_error(raw: &str) -> String {
    let lower = raw.to_ascii_lowercase();
    let advice = if lower.contains("no space")
        || lower.contains("disk full")
        || lower.contains("空间不足")
        || lower.contains("磁盘已满")
    {
        "建议：清理游戏目录所在磁盘的空间后，点击「重试」。"
    } else if lower.contains("permission denied")
        || lower.contains("access is denied")
        || lower.contains("拒绝访问")
        || lower.contains("权限")
    {
        "建议：检查游戏目录是否有写入权限；不要把游戏安装在受保护的系统目录。"
    } else if lower.contains("http 404") || lower.contains("http 403") {
        "建议：该地址可能暂时不可用或文件不存在；稍后重试，仍失败时更换下载源。"
    } else if lower.contains("http 429") || lower.contains("too many requests") {
        "建议：下载源正在限流，请等待一会儿再重试，避免连续点击。"
    } else if lower.contains("checksum") || lower.contains("哈希校验失败") || lower.contains("校验失败") {
        "建议：文件可能下载损坏；重试会重新校验，若反复失败请更换下载源。"
    } else if lower.contains("timed out")
        || lower.contains("timeout")
        || lower.contains("超时")
        || lower.contains("连接")
        || lower.contains("传输中断")
    {
        "建议：检查网络或代理/VPN；下载器会自动尝试备用地址，仍失败可稍后重试。"
    } else if lower.contains("http 5") || lower.contains("网络请求失败") {
        "建议：服务器或网络可能暂时异常；稍后重试，反复失败时更换下载源。"
    } else {
        "建议：先重试一次；如果持续失败，检查网络、磁盘空间和安装目录权限。"
    };
    format!("{raw}\n{advice}")
}

fn batch_label(tasks: &[NewTask]) -> String {
    let kinds: HashSet<&str> = tasks.iter().map(|t| t.kind.as_str()).collect();
    if kinds.contains("modpack") {
        "整合包文件下载".to_string()
    } else if kinds.contains("mod")
        || kinds.contains("datapack")
        || kinds.contains("resourcepack")
        || kinds.contains("shader")
    {
        "社区资源下载".to_string()
    } else if kinds.contains("asset") || kinds.contains("asset-index") {
        "游戏资源下载".to_string()
    } else if kinds.contains("client") || kinds.contains("library") || kinds.contains("native") {
        "游戏文件下载".to_string()
    } else {
        "下载任务组".to_string()
    }
}

fn clone_entry(entry: &Entry) -> Entry {
    Entry {
        id: entry.id.clone(),
        label: entry.label.clone(),
        kind: entry.kind.clone(),
        group_id: entry.group_id.clone(),
        group_label: entry.group_label.clone(),
        url: entry.url.clone(),
        fallbacks: entry.fallbacks.clone(),
        dest: entry.dest.clone(),
        sha1: entry.sha1.clone(),
        size: entry.size,
        state: entry.state,
        done: entry.done,
        total: entry.total,
        error: entry.error.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn item_state_terminal_flags() {
        assert!(ItemState::Done.is_terminal());
        assert!(ItemState::Failed.is_terminal());
        assert!(ItemState::Cancelled.is_terminal());
        assert!(!ItemState::Pending.is_terminal());
        assert!(!ItemState::Running.is_terminal());
    }

    #[test]
    fn batch_label_groups_game_assets() {
        let tasks = vec![
            NewTask::new("asset", "a.ogg", "https://x/a", PathBuf::from("/tmp/a")),
            NewTask::new("asset", "b.ogg", "https://x/b", PathBuf::from("/tmp/b")),
        ];
        assert_eq!(batch_label(&tasks), "游戏资源下载");
    }

    #[test]
    fn download_error_contains_actionable_advice() {
        let disk = friendly_download_error("写入失败：No space left on device");
        assert!(disk.contains("清理游戏目录所在磁盘的空间"));
        let rate = friendly_download_error("HTTP 429（https://example.invalid/file）");
        assert!(rate.contains("正在限流"));
        let network = friendly_download_error("网络请求失败：operation timed out");
        assert!(network.contains("检查网络"));
    }

    #[test]
    fn new_task_builder_sets_optional_fields() {
        let task = NewTask::new("asset", "a.ogg", "https://x/a", PathBuf::from("/tmp/a"))
            .with_sha1(Some("a".repeat(40)))
            .with_size(Some(12));
        assert_eq!(task.kind, "asset");
        assert_eq!(task.sha1.as_deref(), Some("a".repeat(40).as_str()));
        assert_eq!(task.size, Some(12));
        assert!(task.group.is_none());

        // 显式任务组：一次安装的两批下载要共用同一个组号
        let mut task = NewTask::new("asset", "b.ogg", "https://x/b", PathBuf::from("/tmp/b"));
        assert!(task.group.is_none());
        task.set_group("g99", "安装 Minecraft 1.20.1");
        assert_eq!(
            task.group,
            Some(("g99".to_string(), "安装 Minecraft 1.20.1".to_string()))
        );
    }
}
