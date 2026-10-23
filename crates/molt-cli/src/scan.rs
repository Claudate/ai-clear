//! 一次完整扫描：发现 → 收集安全信号 → 判定。
//!
//! 这是把三个 crate 接起来的唯一地方，也是「判定只有一份实现」的保证：
//! CLI 的 `scan` 与 `clean` 都走这里，GUI 将来也走这里。
//!
//! 它**只读**。所有写操作都在 `molt-act` 里，并且要用户确认。

use molt_core::{Candidate, Verdict};
use molt_safety::{skip_subtrees, Signals};
use molt_scan::{discover, human_bytes, Discovered, WalkOptions};

/// 扫描结果。
#[derive(Debug, Default)]
pub struct ScanReport {
    pub items: Vec<ScanItem>,
    /// 因权限不足没扫到的地方，用于告诉用户「这次没扫全」
    pub denied_roots: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ScanItem {
    pub discovered: Discovered,
    pub candidate: Candidate,
}

impl ScanReport {
    pub fn by_verdict(&self, want: fn(&Verdict) -> bool) -> Vec<&ScanItem> {
        self.items
            .iter()
            .filter(|i| want(&i.candidate.verdict))
            .collect()
    }

    /// 可以回收的总量（只算默认勾选的项）。
    ///
    /// 用 `alloc_bytes` 而不是 `bytes`：前者已按 inode 去重，
    /// 是用户真正会多出来的空间。两者混用就会出现「预计释放 20 G，实际只有 8 G」。
    pub fn reclaimable_bytes(&self) -> u64 {
        self.items
            .iter()
            .filter(|i| i.candidate.verdict == Verdict::Reclaimable)
            .map(|i| i.candidate.alloc_bytes.max(i.candidate.bytes))
            .sum()
    }

    /// 有代价项的总量。这是本产品最想让人看见的数字——
    /// 它代表「能腾出来，但你得决定值不值」的那部分。
    pub fn costly_bytes(&self) -> u64 {
        self.items
            .iter()
            .filter(|i| matches!(i.candidate.verdict, Verdict::Costly { .. }))
            .map(|i| i.candidate.alloc_bytes.max(i.candidate.bytes))
            .sum()
    }

    pub fn protected_bytes(&self) -> u64 {
        self.items
            .iter()
            .filter(|i| matches!(i.candidate.verdict, Verdict::Protected { .. }))
            .map(|i| i.candidate.alloc_bytes.max(i.candidate.bytes))
            .sum()
    }

    /// 「建议清理」的项数：有代价、但已经很久没动过。
    ///
    /// 这个数只用于展示与筛选，**绝不参与选择**——`clean` 与 `auto` 都不看它。
    pub fn suggest_clean_count(&self) -> usize {
        self.items
            .iter()
            .filter(|i| i.candidate.suggest_clean)
            .count()
    }

    pub fn suggest_clean_bytes(&self) -> u64 {
        self.items
            .iter()
            .filter(|i| i.candidate.suggest_clean)
            .map(|i| i.candidate.alloc_bytes.max(i.candidate.bytes))
            .sum()
    }

    pub fn summary_line(&self) -> String {
        format!(
            "可回收 {} · 有代价 {} · 受保护 {}",
            human_bytes(self.reclaimable_bytes()),
            human_bytes(self.costly_bytes()),
            human_bytes(self.protected_bytes())
        )
    }
}

/// 执行一次扫描。
pub fn run(rules: &molt_core::RuleSet, home: &str, check_locks: bool) -> ScanReport {
    let roots = molt_safety::normalize_roots(&[std::path::PathBuf::from(home)]);
    let opts = WalkOptions {
        skip: skip_subtrees(home),
        ..Default::default()
    };

    let found = discover(rules, &opts, home);
    let mut report = ScanReport::default();

    // 占用检查一次抬完再查表。以前是每个候选 fork 一次 lsof，
    // 一两百项就是一两百次进程启动，占掉整次扫描近一半时间。
    let snapshot = if check_locks {
        molt_safety::LocksSnapshot::capture()
    } else {
        None
    };

    for (discovered, meta) in found {
        let signals: Signals = molt_safety::collect_signals_with(
            &discovered.path,
            home,
            &roots,
            check_locks,
            snapshot.as_ref(),
        );
        let candidate = molt_core::decide(&meta, rules, &signals, &roots);

        // 记录没读到的路径：用户需要知道「这次没扫全」，而不是以为干净了
        if meta.files == 0
            && meta.bytes == 0
            && discovered.path.is_dir()
            && std::fs::read_dir(&discovered.path).is_err()
        {
            report
                .denied_roots
                .push(discovered.path.display().to_string());
        }

        report.items.push(ScanItem {
            discovered,
            candidate,
        });
    }

    report
}
