//! 后台扫描：把「看磁盘」这件事搬出 UI 线程。
//!
//! 为什么必须后台化：一次全量扫描在真机上是 50 秒量级。放在 UI 线程里，
//! 窗口会整整一分钟没有响应，macOS 还会弹「应用未响应」——用户第一反应是
//! 强制退出。而这恰恰是清理软件最不该给人的印象。
//!
//! 这里只负责「跑、报进度、把结果送回来」。判定本身仍然在 `molt-core`。

use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};

use molt_core::RuleSet;
use molt_scan::DuplicateScan;

/// 重复文件扫描进展。
#[derive(Debug, Clone)]
pub enum DuplicateProgress {
    /// 走到哪一步了：阶段 + 已处理 / 总量
    Working {
        phase: molt_scan::DuplicatePhase,
        done: usize,
        total: usize,
        files_seen: u64,
    },
    Done(Arc<DuplicateScan>),
    Failed(String),
}

/// 一次正在跑的重复文件扫描。
pub struct DuplicateJob {
    rx: Receiver<DuplicateProgress>,
    latest: DuplicateProgress,
    finished: bool,
}

impl DuplicateJob {
    /// 开一个后台扫描。立刻返回，不等结果。
    pub fn start(home: String) -> Self {
        let (tx, rx) = channel();
        let fail_tx = tx.clone();
        std::thread::spawn(move || {
            let roots = molt_safety::normalize_roots(&[std::path::PathBuf::from(&home)]);
            let opts = molt_scan::WalkOptions {
                skip: molt_safety::skip_subtrees(&home),
                budget: Some(std::time::Duration::from_secs(180)),
                ..Default::default()
            };
            let _ = roots;
            // 进度推送频度：每 64 次回调推一次。逐次推会把通道堵成瓶颈，
            // 而几千个文件只有 64 个粒度的进度就已经看不出卡顿了。
            const TICK_EVERY: usize = 64;
            let mut ticks = 0usize;
            let progress_tx = tx.clone();
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                molt_scan::find_duplicates_with_progress(
                    std::path::Path::new(&home),
                    &opts,
                    move |t| {
                        ticks += 1;
                        if ticks % TICK_EVERY == 0 {
                            let _ = progress_tx.send(DuplicateProgress::Working {
                                phase: t.phase,
                                done: t.done,
                                total: t.total,
                                files_seen: t.files_seen,
                            });
                        }
                    },
                )
            }));
            match result {
                Ok(scan) => {
                    let _ = tx.send(DuplicateProgress::Done(Arc::new(scan)));
                }
                Err(payload) => {
                    let what = payload
                        .downcast_ref::<&str>()
                        .map(|s| s.to_string())
                        .or_else(|| payload.downcast_ref::<String>().cloned())
                        .unwrap_or_else(|| "未知错误".to_string());
                    let _ = fail_tx.send(DuplicateProgress::Failed(format!(
                        "重复文件扫描中断：{what}"
                    )));
                }
            }
        });
        Self {
            rx,
            latest: DuplicateProgress::Working {
                phase: molt_scan::DuplicatePhase::Walking,
                done: 0,
                total: 0,
                files_seen: 0,
            },
            finished: false,
        }
    }

    pub fn poll(&mut self) -> &DuplicateProgress {
        while let Ok(p) = self.rx.try_recv() {
            self.latest = p;
        }
        if matches!(
            self.latest,
            DuplicateProgress::Done(_) | DuplicateProgress::Failed(_)
        ) {
            self.finished = true;
        }
        &self.latest
    }

    pub fn is_finished(&self) -> bool {
        self.finished
    }

    pub fn peek(&self) -> &DuplicateProgress {
        &self.latest
    }
}

/// 扫描控制句柄：暂停 / 继续 / 取消。后台线程在每个检查点询问它。
///
/// 暂停不是「杀掉线程」，而是让遍历循环在检查点停下来等；
/// 所以恢复后能从同一进度继续，不会从头再来。
pub struct ScanControl {
    state: Mutex<ScanControlState>,
    wake: Condvar,
}

#[derive(Default)]
struct ScanControlState {
    paused: bool,
    cancelled: bool,
}

impl ScanControl {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(ScanControlState::default()),
            wake: Condvar::new(),
        })
    }

    pub fn pause(&self) {
        self.state.lock().unwrap().paused = true;
    }

    pub fn resume(&self) {
        {
            let mut st = self.state.lock().unwrap();
            st.paused = false;
        }
        self.wake.notify_all();
    }

    pub fn cancel(&self) {
        {
            let mut st = self.state.lock().unwrap();
            st.cancelled = true;
            st.paused = false;
        }
        self.wake.notify_all();
    }

    pub fn is_paused(&self) -> bool {
        self.state.lock().unwrap().paused
    }

    /// 暂停时阻塞在这里，直到被恢复或取消。返回是否已被取消。
    pub fn wait_if_paused(&self) -> bool {
        let mut st = self.state.lock().unwrap();
        while st.paused && !st.cancelled {
            st = self.wake.wait(st).unwrap();
        }
        st.cancelled
    }

    pub fn is_cancelled(&self) -> bool {
        self.state.lock().unwrap().cancelled
    }
}

/// 扫描进展。
#[derive(Debug, Clone)]
pub enum Progress {
    /// 正在展开规则、遍历目录。`found` 是已经发现的条目数。
    Walking { found: usize },
    /// 走到了某个路径，用于让用户知道「它在动」
    Touching { path: String },
    /// 开始逐条判定
    Judging { done: usize, total: usize },
    /// 全部完成
    Done(Arc<ScanResult>),
    /// 扫描没能完成。界面必须把它显示出来——
    /// 静默失败会被误读成「这块盘很干净」。
    Failed(String),
}

/// 一次扫描的完整产出。用 `Arc` 是因为要跨线程共享给 UI，
/// 而且历史页与清单页会同时读它。
#[derive(Debug, Default)]
pub struct ScanResult {
    pub items: Vec<molt_core::Candidate>,
    /// 规则 id，与 `items` 一一对应。
    /// 单独存一份而不是塞进 Candidate：`Candidate` 是跨 CLI / GUI 共用的模型，
    /// 为了界面方便额外加字段会让两边的语义开始分叉。
    pub rule_ids: Vec<String>,
    pub denied_roots: Vec<String>,
    /// 这次扫描耗时，秒
    pub elapsed_secs: f32,
    /// 这份结果是**上次**扫描留下的缓存，不是刚刚扫的。
    ///
    /// 界面据此写「上次扫描的结果」而不是假装是最新的——数字可能已经过时，
    /// 而清理页上的每个数字都会引出一次删除动作，不能让它冒充实时。
    pub from_cache: bool,
}

/// 落盘用的形状。与 `ScanResult` 分开，因为 `elapsed_secs`/`from_cache`
/// 这类字段是「这一次运行」的元信息，不该跟着历史进文件。
#[derive(serde::Serialize, serde::Deserialize)]
struct CachedScan {
    /// 写这份缓存的时间（unix 秒）
    at: u64,
    items: Vec<molt_core::Candidate>,
    rule_ids: Vec<String>,
    denied_roots: Vec<String>,
}

impl ScanResult {
    /// 把结果写进缓存文件。
    ///
    /// 失败一律忽略：缓存是加速手段，写不进去只意味着下次还得多扫一遍，
    /// 不该因此给用户报错。
    pub fn save_cache(&self, path: &std::path::Path) {
        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let cached = CachedScan {
            at,
            items: self.items.clone(),
            rule_ids: self.rule_ids.clone(),
            denied_roots: self.denied_roots.clone(),
        };
        let Ok(json) = serde_json::to_vec(&cached) else {
            return;
        };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, json);
    }

    /// 读回上次的扫描结果，连同它**扫出来的时刻**（`HH:MM`）。
    ///
    /// 时刻要跟着结果一起回来：页面上要写「上面是 10:34 那次扫描的结果」，
    /// 拿「现在」去顶会让那句话和数字互相矛盾。
    ///
    /// 过期就丢掉：超过 `max_age` 的缓存不再代表现在的磁盘，显示它比显示
    /// 「正在扫描…」更糟——用户会照着过时的数字做决定。
    pub fn load_cache(
        path: &std::path::Path,
        max_age: std::time::Duration,
    ) -> Option<(ScanResult, String)> {
        let raw = std::fs::read(path).ok()?;
        let cached: CachedScan = serde_json::from_slice(&raw).ok()?;

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        // `max_age` 为零是唯一要特判的一档（「不接受任何缓存」），
        // 因为刚写下的缓存 `now - at` 也是 0，用 `>` 会把它放过去。
        let too_old = if max_age.is_zero() {
            true
        } else {
            now.saturating_sub(cached.at) > max_age.as_secs()
        };
        if too_old {
            return None;
        }
        Some((
            ScanResult {
                items: cached.items,
                rule_ids: cached.rule_ids,
                denied_roots: cached.denied_roots,
                elapsed_secs: 0.0,
                from_cache: true,
            },
            hhmm_from_unix(cached.at),
        ))
    }
}

/// unix 秒 → 本地 `HH:MM`。
///
/// 复用 `molt_act` 那份 epoch→ISO 的换算（项目里已有、也测过），再套一层
/// 本机时区偏移。为了一句「上次是几点」不值得引 chrono，更不该为此起子进程。
fn hhmm_from_unix(secs: u64) -> String {
    // ISO 是 UTC（结尾 Z）。页面写给人看，要换成本地时间。
    let utc_min = (secs % 86_400) / 60;
    let total = (utc_min as i64 + local_offset_minutes()).rem_euclid(24 * 60);
    format!("{:02}:{:02}", total / 60, total % 60)
}

/// 本机相对 UTC 的分钟偏移（北京是 +480）。
fn local_offset_minutes() -> i64 {
    // 标准库的 SystemTime 不带时区，只能问系统。`%z` 形如 `+0800`。
    std::process::Command::new("date")
        .arg("+%z")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| parse_tz_offset(s.trim()))
        .unwrap_or(0)
}

/// `+0800` / `-0530` → 分钟数。解析不出返回 `None`，由调用方退回 0（UTC）。
fn parse_tz_offset(s: &str) -> Option<i64> {
    if s.len() != 5 {
        return None;
    }
    let sign = match s.as_bytes()[0] {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let h: i64 = s[1..3].parse().ok()?;
    let m: i64 = s[3..5].parse().ok()?;
    Some(sign * (h * 60 + m))
}

impl ScanResult {
    pub fn reclaimable_bytes(&self) -> u64 {
        self.sum_where(|v| matches!(v, molt_core::Verdict::Reclaimable))
    }

    pub fn costly_bytes(&self) -> u64 {
        self.sum_where(|v| matches!(v, molt_core::Verdict::Costly { .. }))
    }

    pub fn protected_bytes(&self) -> u64 {
        self.sum_where(|v| matches!(v, molt_core::Verdict::Protected { .. }))
    }

    fn sum_where(&self, f: impl Fn(&molt_core::Verdict) -> bool) -> u64 {
        self.items
            .iter()
            .filter(|c| f(&c.verdict))
            .map(|c| c.alloc_bytes.max(c.bytes))
            .sum()
    }

    /// 按体积降序，界面用这个顺序展示——用户最先要看到的是最占地的东西。
    pub fn sorted_indices(&self) -> Vec<usize> {
        let mut idx: Vec<usize> = (0..self.items.len()).collect();
        idx.sort_by_key(|&i| std::cmp::Reverse(self.items[i].alloc_bytes.max(self.items[i].bytes)));
        idx
    }
}

/// 一次正在跑的扫描。持有接收端，drop 之后后台线程的输出就没人要了。
pub struct ScanJob {
    rx: Receiver<Progress>,
    pub latest: Progress,
    finished: bool,
    /// 暂停 / 继续 / 取消的控制句柄，界面按钮直接操作它。
    pub control: Arc<ScanControl>,
}

impl ScanJob {
    /// 开一个后台扫描。立刻返回，不等结果。
    pub fn start(rules: RuleSet, home: String, check_locks: bool) -> Self {
        let (tx, rx) = channel();
        let fail_tx = tx.clone();
        let control = ScanControl::new();
        let ctl = control.clone();
        std::thread::spawn(move || {
            // 扫描里任何一处 panic 都不能只留在 stderr：窗口那时会永远停在
            // 「正在扫描…」，用户唯一能做的就是强制退出。这里转成一条可见的失败。
            let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_scan(rules, home, check_locks, tx, Some(ctl.as_ref()))
            }));
            if let Err(payload) = ran {
                let what = payload
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| payload.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "未知错误".to_string());
                let _ = fail_tx.send(Progress::Failed(format!("扫描中断：{what}")));
            }
        });
        Self {
            rx,
            latest: Progress::Walking { found: 0 },
            finished: false,
            control,
        }
    }

    /// 取走目前为止的所有进展，只保留最后一条。
    ///
    /// 用「排空」而不是「每帧读一条」：扫描线程可能一瞬间推很多条，
    /// 逐条消费会让 UI 显示落后于真实进度。
    pub fn poll(&mut self) -> &Progress {
        while let Ok(p) = self.rx.try_recv() {
            self.latest = p;
        }
        if matches!(self.latest, Progress::Done(_) | Progress::Failed(_)) {
            self.finished = true;
        }
        &self.latest
    }

    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// 只读地看一眼当前进展，不消费通道。
    ///
    /// 与 `poll` 分开是因为侧栏是只读上下文：那里不该推进状态，
    /// 否则一帧里两处都调 `poll` 会让进度跳着走。
    pub fn peek(&self) -> &Progress {
        &self.latest
    }

    /// 只在测试里用：断言未完成时没有结果。
    #[cfg(test)]
    pub fn result(&self) -> Option<&Arc<ScanResult>> {
        match &self.latest {
            Progress::Done(r) => Some(r),
            _ => None,
        }
    }
}

/// 真正的扫描。与 CLI 的 `scan::run` 是同一套步骤，
/// 只是这里多做了进度上报。
fn run_scan(
    rules: RuleSet,
    home: String,
    check_locks: bool,
    tx: Sender<Progress>,
    control: Option<&ScanControl>,
) {
    let started = std::time::Instant::now();
    let _ = tx.send(Progress::Walking { found: 0 });
    let result = scan_with(&rules, &home, check_locks, control, |p| {
        let _ = tx.send(p);
    });
    // 取消时绝不能补发 `Done`：`poll()` 只保留最后一条，`Done` 会盖掉
    // 「扫描已取消」，而界面收到 `Done` 会把它 `save_cache` 成扫描结果——
    // 于是「我取消了」变成「盘里很干净」，下次打开窗口看到的正是这份空缓存。
    //
    // 但也不能什么都不发：取消可能发生在扫描已经跑完之后（那时 `scan_with`
    // 不会走到检查点、也就没发过任何失败），任务就永远等不到终态，
    // `device` 一侧会一直停在 Scanning、所有按钮永久置灰。所以这里统一补一条终态。
    if control.is_some_and(|c| c.is_cancelled()) {
        let _ = tx.send(Progress::Failed("扫描已取消".to_string()));
        return;
    }
    let _ = tx.send(Progress::Done(Arc::new(ScanResult {
        elapsed_secs: started.elapsed().as_secs_f32(),
        ..result
    })));
}

/// 同步扫一次。定时任务与端到端验证用它——它们不画界面，不需要进展回调。
pub fn scan_all(rules: &RuleSet, home: &str, check_locks: bool) -> ScanResult {
    scan_with(rules, home, check_locks, None, |_| {})
}

/// 扫描主体。进度通过 `report` 回调发出，两种调用方式共用同一份逻辑，
/// 否则「手动扫描」与「定时扫描」会各自有一套遍历与判定。
fn scan_with(
    rules: &RuleSet,
    home: &str,
    check_locks: bool,
    control: Option<&ScanControl>,
    report: impl Fn(Progress),
) -> ScanResult {
    report(Progress::Walking { found: 0 });

    let roots = molt_safety::normalize_roots(&[std::path::PathBuf::from(home)]);
    let opts = molt_scan::WalkOptions {
        skip: molt_safety::skip_subtrees(home),
        ..Default::default()
    };

    let found = molt_scan::discover(rules, &opts, home);
    let total = found.len();
    report(Progress::Judging { done: 0, total });

    let mut result = ScanResult::default();
    for (i, (discovered, meta)) in found.into_iter().enumerate() {
        if i % 25 == 0 {
            // 暂停/取消检查点：暂停时在这里等待，取消则直接结束。
            if let Some(ctl) = control {
                if ctl.wait_if_paused() {
                    report(Progress::Failed("扫描已取消".to_string()));
                    return result;
                }
            }
            report(Progress::Judging { done: i, total });
            report(Progress::Touching {
                path: discovered.path.display().to_string(),
            });
        }

        let signals = molt_safety::collect_signals(&discovered.path, home, &roots, check_locks);
        let candidate = molt_core::decide(&meta, rules, &signals, &roots);

        if meta.files == 0
            && meta.bytes == 0
            && discovered.path.is_dir()
            && std::fs::read_dir(&discovered.path).is_err()
        {
            result
                .denied_roots
                .push(discovered.path.display().to_string());
        }

        result.rule_ids.push(discovered.rule_id);
        result.items.push(candidate);
    }

    result
}

/// 一次清理任务。删除几十 GB 会持续几十秒，放在 UI 线程里窗口会整段无响应——
/// 而这恰恰发生用户最紧张的时刻（东西正在被删），卡住会让人以为程序崩了。
pub struct CleanJob {
    rx: Receiver<CleanProgress>,
    latest: CleanProgress,
    finished: bool,
}

/// 清理进展。
#[derive(Debug, Clone)]
pub enum CleanProgress {
    /// 已经处理过 n 项 / 共 total 项
    Working { done: usize, total: usize },
    /// 完成
    Done(Arc<CleanReport>),
}

/// 一次清理的结果摘要。保留逐项失败原因，界面才能回答「哪一项没成、为什么」。
#[derive(Debug, Clone, Default)]
pub struct CleanReport {
    pub deleted: usize,
    /// 真正释放的字节数（只算永久删除）
    pub deleted_bytes: u64,
    pub moved: usize,
    pub moved_bytes: u64,
    pub failures: Vec<(String, String)>,
    /// 用的哪种模式，决定成功文案怎么写
    pub mode: molt_act::CleanMode,
}

/// 隔离区「彻底删除」任务。
///
/// 与 `CleanJob` 分开而不是复用它：这一批的对象是隔离区里的记录编号，
/// 不是扫描候选——混进同一个类型里，两种失败原因的措辞就会开始互相污染。
pub struct PurgeJob {
    rx: Receiver<PurgeProgress>,
    latest: PurgeProgress,
    finished: bool,
}

#[derive(Debug, Clone)]
pub enum PurgeProgress {
    Working { done: usize, total: usize },
    Done(Arc<PurgeReport>),
}

/// 隔离区彻底删除的结果。
#[derive(Debug, Clone, Default)]
pub struct PurgeReport {
    pub deleted: usize,
    pub deleted_bytes: u64,
    /// (原位置, 原因)
    pub failures: Vec<(String, String)>,
}

impl PurgeJob {
    pub fn start(
        ids: Vec<i64>,
        store: molt_act::AuditStore,
        quarantine: molt_act::Quarantine,
    ) -> Self {
        let total = ids.len();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(PurgeProgress::Working { done: 0, total });
            let exec = molt_act::Executor::new(store, quarantine, 0);
            let outcome = exec.purge_ids(&ids);
            let report = PurgeReport {
                deleted: outcome.deleted.len(),
                deleted_bytes: outcome.deleted_bytes(),
                failures: outcome
                    .failed
                    .iter()
                    .map(|f| (f.path.display().to_string(), f.reason.clone()))
                    .collect(),
            };
            let _ = tx.send(PurgeProgress::Done(Arc::new(report)));
        });
        Self {
            rx,
            latest: PurgeProgress::Working { done: 0, total },
            finished: false,
        }
    }

    pub fn poll(&mut self) -> &PurgeProgress {
        while let Ok(p) = self.rx.try_recv() {
            self.latest = p;
        }
        if matches!(self.latest, PurgeProgress::Done(_)) {
            self.finished = true;
        }
        &self.latest
    }

    pub fn is_finished(&self) -> bool {
        self.finished
    }

    pub fn peek(&self) -> &PurgeProgress {
        &self.latest
    }
}

/// 批量还原隔离区条目。
///
/// 与 `PurgeJob` 同形：每还原一条发一次进度，界面据此更新文字与进度条。
/// 还原可能涉及解压几十 GB，同步跑会把窗口卡到没有任何反馈。
pub struct RestoreJob {
    rx: Receiver<RestoreProgress>,
    latest: RestoreProgress,
    finished: bool,
}

#[derive(Debug, Clone)]
pub enum RestoreProgress {
    Working { done: usize, total: usize },
    Done(Arc<RestoreReport>),
}

/// 批量还原的结果。
#[derive(Debug, Clone, Default)]
pub struct RestoreReport {
    pub restored: usize,
    /// (展示名, 原因)
    pub failures: Vec<(String, String)>,
}

impl RestoreJob {
    pub fn start(
        ids: Vec<i64>,
        store: molt_act::AuditStore,
        quarantine: molt_act::Quarantine,
    ) -> Self {
        let total = ids.len();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(RestoreProgress::Working { done: 0, total });
            let exec = molt_act::Executor::new(store, quarantine, 0);
            let mut failures = Vec::new();
            for (done, id) in ids.iter().enumerate() {
                if let Err(e) = exec.restore(*id) {
                    failures.push((format!("编号 {id}"), e));
                }
                let _ = tx.send(RestoreProgress::Working {
                    done: done + 1,
                    total,
                });
            }
            let _ = tx.send(RestoreProgress::Done(Arc::new(RestoreReport {
                restored: total - failures.len(),
                failures,
            })));
        });
        Self {
            rx,
            latest: RestoreProgress::Working { done: 0, total },
            finished: false,
        }
    }

    pub fn poll(&mut self) -> &RestoreProgress {
        while let Ok(p) = self.rx.try_recv() {
            self.latest = p;
        }
        if matches!(self.latest, RestoreProgress::Done(_)) {
            self.finished = true;
        }
        &self.latest
    }

    pub fn is_finished(&self) -> bool {
        self.finished
    }

    pub fn peek(&self) -> &RestoreProgress {
        &self.latest
    }
}

impl CleanJob {
    /// 开一个后台清理。`candidates` 是用户在界面上选中的那些。
    pub fn start(
        candidates: Vec<molt_core::Candidate>,
        store: molt_act::AuditStore,
        quarantine: molt_act::Quarantine,
        mode: molt_act::CleanMode,
        retain_days: i64,
    ) -> Self {
        let total = candidates.len();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(CleanProgress::Working { done: 0, total });

            let exec = molt_act::Executor::new(store, quarantine, retain_days)
                .with_source(molt_act::CleanSource::Manual)
                .with_mode(mode);
            let confirmation = molt_act::Confirmation::new(&candidates);
            let outcome = exec.clean_with_progress(&candidates, &confirmation, |done, total| {
                let _ = tx.send(CleanProgress::Working { done, total });
            });

            let report = CleanReport {
                deleted: outcome.deleted.len(),
                deleted_bytes: outcome.deleted_bytes(),
                moved: outcome.moved.len(),
                moved_bytes: outcome.moved.iter().map(|m| m.bytes).sum(),
                failures: outcome
                    .failed
                    .iter()
                    .map(|f| (f.path.display().to_string(), f.reason.clone()))
                    .collect(),
                mode,
            };
            let _ = tx.send(CleanProgress::Done(Arc::new(report)));
        });
        Self {
            rx,
            latest: CleanProgress::Working { done: 0, total },
            finished: false,
        }
    }

    pub fn poll(&mut self) -> &CleanProgress {
        while let Ok(p) = self.rx.try_recv() {
            self.latest = p;
        }
        if matches!(self.latest, CleanProgress::Done(_)) {
            self.finished = true;
        }
        &self.latest
    }

    pub fn is_finished(&self) -> bool {
        self.finished
    }

    pub fn peek(&self) -> &CleanProgress {
        &self.latest
    }

    /// 仅供测试与开发版截图预览；正式清理进度始终来自执行器回调。
    #[cfg(any(test, debug_assertions))]
    pub fn preview_progress(done: usize, total: usize) -> Self {
        let (_, rx) = channel();
        Self {
            rx,
            latest: CleanProgress::Working { done, total },
            finished: false,
        }
    }
}

/// 枚举已装应用与残留的后台任务。
///
/// 分三段发消息，是为了让界面**立刻有事可看**：
///
/// 1. [`AppScanProgress::Listed`] —— 名字、路径、图标都在了，只是体积还没量
/// 2. [`AppScanProgress::Measured`] —— 某个应用的体积量好了，界面按路径填进去
/// 3. [`AppScanProgress::Done`] —— 残留也找完了，整页到此为止
///
/// 以前是一条消息到底：要等每个 bundle 的几万个文件全数完、每个应用的
/// `~/Library` 全查完才一次性出现。用户点进卸载页看到的是几秒空白，
/// 而这段时间正是「为什么列个应用这么卡」的来源。
pub struct AppScanJob {
    rx: Receiver<AppScanProgress>,
    latest: AppScanProgress,
    finished: bool,
}

#[derive(Debug, Clone)]
pub enum AppScanProgress {
    /// 清单出来了，体积待补
    Listed {
        apps: Vec<molt_act::AppEntry>,
        scanned_dirs: Vec<String>,
    },
    /// 一个应用的体积量好了
    Measured { path: String, bytes: u64 },
    /// 残留也找完了
    Done(Arc<AppScanResult>),
}

/// 应用列表 + 每个应用的残留。
#[derive(Debug, Clone, Default)]
pub struct AppScanResult {
    pub apps: Vec<molt_act::AppEntry>,
    pub scanned_dirs: Vec<String>,
}

/// 一个应用的残留盘点，按需启动。
///
/// 为什么单独一条任务：残留扫描在个别应用上会慢到不可接受（见
/// [`AppScanJob::start`] 里的注释）。它只在用户点开某个应用时才需要，
/// 所以从「列出全部应用」里拆出来——列出永远是快的，点开某一个慢也只慢那一个，
/// 而且能显示「正在找残留…」而不是把整个页面拖住。
pub struct LeftoverJob {
    rx: Receiver<Vec<molt_act::Leftover>>,
    /// 这次是为了哪个应用扫的：结果回来时页面可能已经展开了别的应用。
    pub app_path: String,
    done: bool,
}

impl LeftoverJob {
    pub fn start(app: molt_act::AppEntry, home: String) -> Self {
        let (tx, rx) = channel();
        let app_path = app.path.display().to_string();
        std::thread::spawn(move || {
            let l = molt_act::uninstall::leftovers_for(&app, &home);
            let _ = tx.send(l);
        });
        Self {
            rx,
            app_path,
            done: false,
        }
    }

    pub fn poll(&mut self) -> Option<Vec<molt_act::Leftover>> {
        match self.rx.try_recv() {
            Ok(l) => {
                self.done = true;
                Some(l)
            }
            Err(_) => None,
        }
    }

    pub fn is_finished(&self) -> bool {
        self.done
    }
}

impl AppScanJob {
    pub fn start(home: String) -> Self {
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let dirs = molt_act::uninstall::app_dirs(&home);
            // 第一段：只读元信息，不等体积。图标与名字在这一步就齐了。
            let mut apps = molt_act::uninstall::list_apps(&dirs);
            let scanned_dirs: Vec<String> = dirs.iter().map(|d| d.display().to_string()).collect();
            let _ = tx.send(AppScanProgress::Listed {
                apps: apps.clone(),
                scanned_dirs: scanned_dirs.clone(),
            });

            // 第二段：体积并行量。每条量好就发一条，界面按路径填。
            //
            // 线程数按可用核数封顶：量体积是 IO + metadata 密集的活，
            // 开太多只会互相抢盘；而每个 bundle 动辄几万文件，
            // 串行是这次卡顿的主因。
            let paths: Vec<(String, std::path::PathBuf)> = apps
                .iter()
                .map(|a| (a.path.display().to_string(), a.path.clone()))
                .collect();
            let workers = std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4)
                .clamp(2, 8);
            let next = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
            // 量到的体积要同时**写回 `apps`**，不能只发消息了事。
            // `Done` 带的是这份 `apps`：以前它上面的 `bytes` 一直是列清单时的 0，
            // 界面收到 `Done` 又按它覆盖一遍，于是逐条量好的数字全被冲回 0、
            // 每个格子永远停在「正在量…」。这里用互斥锁收结果，join 之后再写。
            let measured: std::sync::Arc<std::sync::Mutex<Vec<(usize, u64)>>> =
                std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            let mut handles = Vec::with_capacity(workers);
            for _ in 0..workers {
                let tx = tx.clone();
                let paths = paths.clone();
                let next = next.clone();
                let measured = measured.clone();
                handles.push(std::thread::spawn(move || loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some((key, path)) = paths.get(i) else {
                        break;
                    };
                    let bytes = molt_act::uninstall::measure_path(path);
                    if let Ok(mut m) = measured.lock() {
                        m.push((i, bytes));
                    }
                    let _ = tx.send(AppScanProgress::Measured {
                        path: key.clone(),
                        bytes,
                    });
                }));
            }
            for h in handles {
                let _ = h.join();
            }
            if let Ok(m) = measured.lock() {
                for &(i, bytes) in m.iter() {
                    if let Some(app) = apps.get_mut(i) {
                        app.bytes = bytes;
                    }
                }
            }

            molt_act::uninstall::sort_by_bytes(&mut apps);

            // 残留**不在这里扫**。
            //
            // 以前这里会逐个应用遍历 `~/Library` 的十几条候选路径、并对每条
            // 递归量体积。代价极不均匀：本机一个 Clash Verge 的
            // `Group Containers/io.github.clash-verge-rev.clash-verge-rev`
            // 单次遍历就超过 600 秒没跑完——而这是「列个应用」必须等的事，
            // 于是整页卡死。而残留只在**点开某个应用**时才需要看，
            // 现在就放到 [`LeftoverJob`] 里按需扫。
            let result = AppScanResult { apps, scanned_dirs };
            let _ = tx.send(AppScanProgress::Done(Arc::new(result)));
        });
        Self {
            rx,
            latest: AppScanProgress::Listed {
                apps: Vec::new(),
                scanned_dirs: Vec::new(),
            },
            finished: false,
        }
    }

    pub fn poll(&mut self) -> &AppScanProgress {
        while let Ok(p) = self.rx.try_recv() {
            self.latest = p;
        }
        if matches!(self.latest, AppScanProgress::Done(_)) {
            self.finished = true;
        }
        &self.latest
    }

    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// 把这一帧攒下的消息全部取出来，一条不丢。
    ///
    /// [`Self::poll`] 只保留最新一条，够用于「进度到哪了」，但体积是逐条到的：
    /// 丢掉一条就会有一个格子的体积永远停在 0。所以这个任务走 `drain`。
    pub fn drain(&mut self) -> Vec<AppScanProgress> {
        let mut out = Vec::new();
        while let Ok(p) = self.rx.try_recv() {
            if matches!(p, AppScanProgress::Done(_)) {
                self.finished = true;
            }
            out.push(p);
        }
        out
    }

    pub fn peek(&self) -> &AppScanProgress {
        &self.latest
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use molt_core::{CostKind, ItemKind, RebuildCost, Verdict};

    fn cand(bytes: u64, v: Verdict) -> molt_core::Candidate {
        molt_core::Candidate::new("/x", ItemKind::Dir, v).with_size(bytes, bytes, 1)
    }

    #[test]
    fn pause_blocks_until_resumed() {
        let ctl = ScanControl::new();
        let c2 = ctl.clone();
        let entered = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let e2 = entered.clone();

        ctl.pause();
        let t = std::thread::spawn(move || {
            e2.store(true, std::sync::atomic::Ordering::SeqCst);
            let cancelled = c2.wait_if_paused();
            assert!(!cancelled, "恢复路径不应报告取消");
        });
        // 给线程一个到达检查点的机会
        for _ in 0..100 {
            if entered.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(
            entered.load(std::sync::atomic::Ordering::SeqCst),
            "线程应进入检查点并阻塞"
        );
        ctl.resume();
        t.join().unwrap();
    }

    #[test]
    fn cancel_wakes_and_reports_cancelled() {
        let ctl = ScanControl::new();
        let c2 = ctl.clone();
        let entered = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let e2 = entered.clone();

        ctl.pause();
        let t = std::thread::spawn(move || {
            e2.store(true, std::sync::atomic::Ordering::SeqCst);
            c2.wait_if_paused()
        });
        for _ in 0..100 {
            if entered.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        ctl.cancel();
        assert!(
            t.join().unwrap(),
            "取消后等待应立即带着 cancelled=true 返回"
        );
        assert!(ctl.is_cancelled());
    }

    #[test]
    fn not_paused_never_blocks() {
        let ctl = ScanControl::new();
        assert!(!ctl.wait_if_paused(), "未暂停时不阻塞也不报告取消");
    }

    /// 列举应用之后，`Done` 里带的必须是**量到的真实体积**。
    ///
    /// 这一条抓的是 2026-10-28 那个 bug：背景线程量完体积只发了一条消息，
    /// 从没写回 `apps`，于是 `Done` 带回的全是列清单时的 0；界面收到它又按
    /// 这份 0 覆盖一遍，逐条量好的数字全被冲掉——每个格子永远停在「正在量…」。
    /// 所以在**假家目录里**真跑一遍整个任务，断言终局的体积非 0，
    /// 并且逐条消息里也能拿到同一个数。
    ///
    /// 真 `/Applications` 也会被列进来（那是只读的），这里只断言自己造的那一个，
    /// 免得测试依赖本机装了什么。
    #[test]
    fn app_scan_reports_measured_sizes_in_the_final_result() {
        let home = std::env::temp_dir().join(format!(
            "molt-appscan-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let app = home.join("Applications").join("Probe.app");
        std::fs::create_dir_all(app.join("Contents").join("MacOS")).unwrap();
        // 1 个 400 KiB 的文件：APFS 上一定占出可观的块数，不会被读成 0
        std::fs::write(
            app.join("Contents").join("MacOS").join("probe"),
            vec![3u8; 400 * 1024],
        )
        .unwrap();

        let mut job = AppScanJob::start(home.to_string_lossy().to_string());
        // 等到 Done（正常几秒内结束；这里给足余量，但仍要有上限，不然卡住会挂死测试）
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
        let mut done = None;
        let mut per_item: std::collections::HashMap<String, u64> = std::collections::HashMap::new();
        while std::time::Instant::now() < deadline {
            for p in job.drain() {
                match p {
                    AppScanProgress::Measured { path, bytes } => {
                        per_item.insert(path, bytes);
                    }
                    AppScanProgress::Done(res) => done = Some(res),
                    AppScanProgress::Listed { .. } => {}
                }
            }
            if let Some(d) = done.clone() {
                done = Some(d);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let result = done.expect("应用列举要在时限内结束");

        let probe = result
            .apps
            .iter()
            .find(|a| a.name == "Probe")
            .expect("假 home 里的 Probe.app 要被列出来");
        assert!(
            probe.bytes >= 400 * 1024,
            "终局体积要带回真实值，实际 {} —— 为 0 说明量到的结果没写回 apps，\
             界面会永远停在「正在量…」",
            probe.bytes
        );
        let key = probe.path.display().to_string();
        assert_eq!(
            per_item.get(&key),
            Some(&probe.bytes),
            "逐条消息与终局必须是同一个数"
        );

        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn totals_split_by_verdict() {
        let r = ScanResult {
            items: vec![
                cand(100, Verdict::Reclaimable),
                cand(
                    300,
                    Verdict::Costly {
                        rebuild: RebuildCost::new(CostKind::Recompile, "重编"),
                    },
                ),
                cand(
                    900,
                    Verdict::Protected {
                        reason: molt_core::ProtectReason::UserRecord,
                    },
                ),
            ],
            ..Default::default()
        };
        assert_eq!(r.reclaimable_bytes(), 100);
        assert_eq!(r.costly_bytes(), 300);
        assert_eq!(r.protected_bytes(), 900);
    }

    #[test]
    fn sorted_indices_put_the_biggest_first() {
        let r = ScanResult {
            items: vec![
                cand(10, Verdict::Reclaimable),
                cand(9000, Verdict::Reclaimable),
                cand(500, Verdict::Reclaimable),
            ],
            ..Default::default()
        };
        assert_eq!(r.sorted_indices(), vec![1, 2, 0]);
    }

    #[test]
    fn poll_drains_to_the_latest_progress() {
        let (tx, rx) = channel();
        tx.send(Progress::Walking { found: 1 }).unwrap();
        tx.send(Progress::Judging { done: 5, total: 10 }).unwrap();
        tx.send(Progress::Done(Arc::new(ScanResult::default())))
            .unwrap();

        let mut job = ScanJob {
            rx,
            latest: Progress::Walking { found: 0 },
            finished: false,
            control: ScanControl::new(),
        };
        job.poll();
        assert!(job.is_finished(), "最后一条是 Done，应算结束");
        assert!(job.result().is_some());
    }

    #[test]
    fn scan_cache_round_trips_and_marks_itself_as_stale() {
        let d = std::env::temp_dir().join(format!("molt-cache-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("scan-cache.json");

        let mut r = ScanResult::default();
        r.items.push(
            molt_core::Candidate::new(
                "/Users/me/a/target",
                molt_core::ItemKind::Dir,
                molt_core::Verdict::Reclaimable,
            )
            .with_size(100, 80, 3),
        );
        r.rule_ids.push("rust-target".into());
        r.save_cache(&p);

        let (back, at) = ScanResult::load_cache(&p, std::time::Duration::from_secs(3600))
            .expect("刚写的缓存应当能读回来");
        assert_eq!(back.items.len(), 1);
        assert_eq!(back.items[0].path.to_string_lossy(), "/Users/me/a/target");
        assert_eq!(back.rule_ids, vec!["rust-target".to_string()]);
        // 读回来的必须自报「这是缓存」：界面靠它决定不预勾任何项。
        assert!(back.from_cache, "缓存结果必须标明来源");
        // 时刻也要带回来，且是个 HH:MM 形状
        assert_eq!(at.len(), 5, "时刻应当是 HH:MM，实际 {at:?}");
        assert!(at.contains(':'), "时刻应当是 HH:MM，实际 {at:?}");

        // 过期的缓存不再返回 —— 一天的旧数字不能当成现在的磁盘
        assert!(
            ScanResult::load_cache(&p, std::time::Duration::ZERO).is_none(),
            "过期缓存要丢掉"
        );
        std::fs::remove_dir_all(&d).ok();
    }
}
