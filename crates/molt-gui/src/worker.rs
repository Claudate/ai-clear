//! 后台扫描：把「看磁盘」这件事搬出 UI 线程。
//!
//! 为什么必须后台化：一次全量扫描在真机上是 50 秒量级。放在 UI 线程里，
//! 窗口会整整一分钟没有响应，macOS 还会弹「应用未响应」——用户第一反应是
//! 强制退出。而这恰恰是清理软件最不该给人的印象。
//!
//! 这里只负责「跑、报进度、把结果送回来」。判定本身仍然在 `molt-core`。

use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use molt_core::RuleSet;

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
}

impl ScanJob {
    /// 开一个后台扫描。立刻返回，不等结果。
    pub fn start(rules: RuleSet, home: String, check_locks: bool) -> Self {
        let (tx, rx) = channel();
        let fail_tx = tx.clone();
        std::thread::spawn(move || {
            // 扫描里任何一处 panic 都不能只留在 stderr：窗口那时会永远停在
            // 「正在扫描…」，用户唯一能做的就是强制退出。这里转成一条可见的失败。
            let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_scan(rules, home, check_locks, tx)
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
fn run_scan(rules: RuleSet, home: String, check_locks: bool, tx: Sender<Progress>) {
    let started = std::time::Instant::now();
    let _ = tx.send(Progress::Walking { found: 0 });
    let result = scan_with(&rules, &home, check_locks, |p| {
        let _ = tx.send(p);
    });
    let _ = tx.send(Progress::Done(Arc::new(ScanResult {
        elapsed_secs: started.elapsed().as_secs_f32(),
        ..result
    })));
}

/// 同步扫一次。定时任务与端到端验证用它——它们不画界面，不需要进展回调。
pub fn scan_all(rules: &RuleSet, home: &str, check_locks: bool) -> ScanResult {
    scan_with(rules, home, check_locks, |_| {})
}

/// 扫描主体。进度通过 `report` 回调发出，两种调用方式共用同一份逻辑，
/// 否则「手动扫描」与「定时扫描」会各自有一套遍历与判定。
fn scan_with(
    rules: &RuleSet,
    home: &str,
    check_locks: bool,
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
            let outcome = exec.clean(&candidates, &confirmation);

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
}

#[cfg(test)]
mod tests {
    use super::*;
    use molt_core::{CostKind, ItemKind, RebuildCost, Verdict};

    fn cand(bytes: u64, v: Verdict) -> molt_core::Candidate {
        molt_core::Candidate::new("/x", ItemKind::Dir, v).with_size(bytes, bytes, 1)
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
        };
        job.poll();
        assert!(job.is_finished(), "最后一条是 Done，应算结束");
        assert!(job.result().is_some());
    }
}
