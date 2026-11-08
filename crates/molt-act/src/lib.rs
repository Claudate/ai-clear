//! `molt-act`：**整个产品里唯一能写盘的层**。
//!
//! 把写权限收拢到一个 crate，是为了让「不会误删」这件事有结构性保证：
//! 扫描层拿到的是只读元数据，判定层是纯函数，只有这里能动文件，
//! 而这里的每个入口都要求一个 `Confirmation`——它只能由用户显式确认产生。

pub mod archive;
pub mod audit;
pub mod create;
pub mod hash;
pub mod paths;
pub mod permadelete;
pub mod quarantine;
pub mod uninstall;

pub use audit::{ActionKind, ActionRecord, AuditStore, CleanSource, OpKind};
pub use create::{CreateKind, CreatePlan, CreateReport, CreateSpec, SpecCheck};
pub use quarantine::Quarantine;
pub use uninstall::{AppEntry, AppMeta, Leftover, UninstallPlan};

use molt_core::{Candidate, Verdict};
use std::path::PathBuf;

/// 用户对某批条目的显式确认。
///
/// 故意不实现 `Default`、也没有公开构造器之外的其他创建方式，
/// 这样「没确认就执行」在类型层面就写不出来。
#[derive(Debug, Clone)]
pub struct Confirmation {
    accepted: Vec<PathBuf>,
}

impl Confirmation {
    /// 无人值守的确认：来自 `molt auto` 或定时任务。
    ///
    /// 它只对**规则自己声明过 `auto = true`** 的候选有效——调用方传进来的清单
    /// 里混进一条件没声明过的，这里会直接筛掉。这一层不信任调用方，
    /// 因为“自动清理”是本产品唯一没有人在现场把关的路径。
    pub fn for_auto(candidates: &[Candidate], rules: &molt_core::RuleSet) -> Self {
        Self {
            accepted: candidates
                .iter()
                .filter(|c| c.verdict == Verdict::Reclaimable)
                .filter(|c| match &c.rule_id {
                    Some(id) => rules.is_auto(id),
                    // 没有规则归属的条目不敢自动清——没人能说清它是什么
                    None => false,
                })
                .map(|c| c.path.clone())
                .collect(),
        }
    }

    /// 由调用方在用户确认之后创建。传入的每一项都必须仍是可动的判定——
    /// 这里再挡一次，是因为从「扫描」到「确认」之间，文件可能被别的进程改了状态。
    pub fn new(candidates: &[Candidate]) -> Self {
        Self {
            accepted: candidates
                .iter()
                .filter(|c| c.verdict.is_actionable())
                .map(|c| c.path.clone())
                .collect(),
        }
    }

    pub fn is_accepted(&self, path: &std::path::Path) -> bool {
        self.accepted.iter().any(|p| p == path)
    }

    pub fn len(&self) -> usize {
        self.accepted.len()
    }

    pub fn is_empty(&self) -> bool {
        self.accepted.is_empty()
    }

    pub fn paths(&self) -> &[PathBuf] {
        &self.accepted
    }
}

/// 单次清理的结果。逐项记录成功与失败——
/// 一次清理里有一项失败不该让整批回滚，但用户必须知道是哪一项。
#[derive(Debug, Clone, Default)]
pub struct CleanOutcome {
    pub moved: Vec<MovedItem>,
    pub deleted: Vec<DeletedItem>,
    pub failed: Vec<FailedItem>,
}

#[derive(Debug, Clone)]
pub struct MovedItem {
    pub id: i64,
    pub original: PathBuf,
    pub quarantine: PathBuf,
    pub bytes: u64,
}

/// 永久删除成功的一项。
#[derive(Debug, Clone)]
pub struct DeletedItem {
    pub id: i64,
    pub path: PathBuf,
    pub bytes: u64,
}

#[derive(Debug, Clone)]
pub struct FailedItem {
    pub path: PathBuf,
    pub reason: String,
}

impl CleanOutcome {
    /// 处理掉的总字节数（移走的 + 删掉的）。
    pub fn freed_bytes(&self) -> u64 {
        self.moved.iter().map(|m| m.bytes).sum::<u64>()
            + self.deleted.iter().map(|d| d.bytes).sum::<u64>()
    }

    /// **真正**让磁盘可用空间变多的字节数。
    ///
    /// 只有直接删除才算。移入隔离区用的是 `rename`，文件仍在这块盘上，
    /// 可用空间一个字节都没变——把两者混在一个「已释放」数字里，
    /// 正是「清完了空间没变」这类信任事故的来源。
    pub fn deleted_bytes(&self) -> u64 {
        self.deleted.iter().map(|d| d.bytes).sum()
    }
}

/// 清理动作的落地方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CleanMode {
    /// 移入隔离区：可还原，但**不释放**磁盘空间
    #[default]
    Quarantine,
    /// 永久删除：立即释放空间，不可还原
    Delete,
}

/// 把候选移入隔离区并写审计。**不删除任何东西。**
pub struct Executor {
    store: AuditStore,
    quarantine: Quarantine,
    /// 压缩归档的落点。与隔离区并列，默认在应用数据目录下。
    archive: archive::Archive,
    /// 隔离保留天数
    retain_days: i64,
    /// 本次执行是人看着做的，还是定时任务自作主张——写进审计，事后可查
    source: CleanSource,
    /// 移入隔离区还是永久删除
    mode: CleanMode,
    /// 永久删除的授权根。默认家目录；显式设置后可以让清理范围与扫描范围保持一致。
    authorized_root: std::path::PathBuf,
}

impl Executor {
    pub fn new(store: AuditStore, quarantine: Quarantine, retain_days: i64) -> Self {
        let batch = quarantine
            .batch_dir()
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "batch".to_string());
        let root = quarantine
            .root()
            .parent()
            .map(|p| p.join("ai-archive"))
            .unwrap_or_else(|| std::path::PathBuf::from("ai-archive"));
        Self {
            store,
            quarantine,
            archive: archive::Archive::new(root, batch),
            retain_days,
            source: CleanSource::Manual,
            mode: CleanMode::Quarantine,
            authorized_root: paths::home_dir(),
        }
    }

    /// 指定永久删除的授权根。传进来的路径必须与扫描范围一致——
    /// 否则会出现「扫得到但删不掉」这种自相矛盾的结果。
    pub fn with_authorized_root(mut self, root: impl Into<std::path::PathBuf>) -> Self {
        self.authorized_root = root.into();
        self
    }

    /// 标记本次执行为无人值守。
    pub fn with_source(mut self, source: CleanSource) -> Self {
        self.source = source;
        self
    }

    /// 指定清理动作。默认移入隔离区（可还原）。
    ///
    /// 要「释放磁盘空间」只能选 `Delete`——而且它需要用户在界面上
    /// 明确选择后才会传进来，这个类型本身不表达「推荐」。
    pub fn with_mode(mut self, mode: CleanMode) -> Self {
        self.mode = mode;
        self
    }

    pub fn mode(&self) -> CleanMode {
        self.mode
    }

    pub fn store(&self) -> &AuditStore {
        &self.store
    }

    pub fn quarantine(&self) -> &Quarantine {
        &self.quarantine
    }

    /// 执行一次清理。按 `mode` 决定移入隔离区还是永久删除。
    ///
    /// 关键约束：只接受 `Reclaimable` 与 `Costly` 的条目。
    /// `Protected` 一律跳过并记为失败——这不是防御性编程，而是产品承诺：
    /// 「受保护的东西不会被这个工具移动」，用户应该能靠这一点放心按确认。
    pub fn clean(&self, candidates: &[Candidate], confirmation: &Confirmation) -> CleanOutcome {
        self.clean_with_progress(candidates, confirmation, |_, _| {})
    }

    /// 和 `clean` 执行相同的动作，并在每个候选项处理或跳过后报告已检查数与总数。
    /// 回调在执行线程同步调用，跳过项也计入已检查数；它不表示成功清理数。
    pub fn clean_with_progress(
        &self,
        candidates: &[Candidate],
        confirmation: &Confirmation,
        mut progress: impl FnMut(usize, usize),
    ) -> CleanOutcome {
        match self.mode {
            CleanMode::Quarantine => {
                self.clean_to_quarantine(candidates, confirmation, &mut progress)
            }
            CleanMode::Delete => self.clean_permanently(candidates, confirmation, &mut progress),
        }
    }

    /// 永久删除。**这是唯一会真正让可用空间变多的路径。**
    ///
    /// 不能还原，所以每条都先过 `permadelete::validate`，再删，再记审计。
    /// 审计写失败时无法回滚（内容已经没了），只能如实记进 `failed`——
    /// 这种情况下用户至少要知道「东西已删，但记录没留下」。
    fn clean_permanently(
        &self,
        candidates: &[Candidate],
        confirmation: &Confirmation,
        progress: &mut impl FnMut(usize, usize),
    ) -> CleanOutcome {
        let now = now_iso8601();
        let mut outcome = CleanOutcome::default();
        let bytes_of = |c: &Candidate| c.alloc_bytes.max(c.bytes);

        // 先子后父：同时勾了父目录和子目录时，顺序错了会让其中一条必然报「不存在」。
        // 这里重排候选本身，保持 outcome 里的顺序与执行顺序一致。
        let mut ordered: Vec<&Candidate> = candidates.iter().collect();
        ordered.sort_by_key(|c| std::cmp::Reverse(c.path.components().count()));

        for (index, c) in ordered.into_iter().enumerate() {
            if matches!(c.verdict, Verdict::Protected { .. }) {
                outcome.failed.push(FailedItem {
                    path: c.path.clone(),
                    reason: "受保护项，已跳过".to_string(),
                });
                progress(index + 1, candidates.len());
                continue;
            }
            if !confirmation.is_accepted(&c.path) {
                progress(index + 1, candidates.len());
                continue;
            }
            // 父目录刚被删掉时，子项已经不存在了。这不是失败，
            // 它的空间已经算在父目录那一条里——报失败会让用户以为漏清了。
            if !c.path.exists() && std::fs::symlink_metadata(&c.path).is_err() {
                progress(index + 1, candidates.len());
                continue;
            }

            let bytes = bytes_of(c);
            match permadelete::remove_within(&c.path, &self.authorized_root) {
                Ok(()) => match self
                    .store
                    .record_direct_delete(&c.path, bytes, &now, self.source)
                {
                    Ok(id) => outcome.deleted.push(DeletedItem {
                        id,
                        path: c.path.clone(),
                        bytes,
                    }),
                    Err(e) => outcome.failed.push(FailedItem {
                        path: c.path.clone(),
                        reason: format!("已删除，但写审计失败：{e}"),
                    }),
                },
                Err(e) => outcome.failed.push(FailedItem {
                    path: c.path.clone(),
                    reason: e,
                }),
            }
            progress(index + 1, candidates.len());
        }

        outcome
    }

    /// 移入隔离区：可还原，但不释放空间。
    fn clean_to_quarantine(
        &self,
        candidates: &[Candidate],
        confirmation: &Confirmation,
        progress: &mut impl FnMut(usize, usize),
    ) -> CleanOutcome {
        let now = now_iso8601();
        let purge_after = add_days(&now, self.retain_days);
        let mut outcome = CleanOutcome::default();

        for (index, c) in candidates.iter().enumerate() {
            if matches!(c.verdict, Verdict::Protected { .. }) {
                outcome.failed.push(FailedItem {
                    path: c.path.clone(),
                    reason: "受保护项，已跳过".to_string(),
                });
                progress(index + 1, candidates.len());
                continue;
            }
            if !confirmation.is_accepted(&c.path) {
                progress(index + 1, candidates.len());
                continue;
            }

            // 先算哈希再移动：移动之后就再也读不到原位置了
            let digest = hash::path_sha256(&c.path).unwrap_or_default();

            // 需要先拿到 id 才能命名隔离目录里的条目，所以用一个临时 id 占位、
            // 拿到真实 id 后不重命名——审计库里存的是实际路径，不依赖命名规则。
            match self.quarantine.move_in(&c.path, self.next_id()) {
                Ok(dest) => {
                    let record = audit::NewQuarantine {
                        absolute: &c.path,
                        quarantine: &dest,
                        bytes: c.alloc_bytes.max(c.bytes),
                        sha256: &digest,
                        performed_at: &now,
                        purge_after: &purge_after,
                        source: self.source,
                    };
                    match self.store.record_quarantine(&record) {
                        Ok(id) => outcome.moved.push(MovedItem {
                            id,
                            original: c.path.clone(),
                            quarantine: dest,
                            bytes: c.alloc_bytes.max(c.bytes),
                        }),
                        Err(e) => {
                            // 审计写不进去就把东西放回去——没有记录的清理不该发生
                            let _ = self.quarantine.move_out(&dest, &c.path);
                            outcome.failed.push(FailedItem {
                                path: c.path.clone(),
                                reason: format!("写审计失败，已回滚：{e}"),
                            });
                        }
                    }
                }
                Err(e) => outcome.failed.push(FailedItem {
                    path: c.path.clone(),
                    reason: format!("移动失败：{e}"),
                }),
            }
            progress(index + 1, candidates.len());
        }

        self.quarantine.tidy_batch();
        outcome
    }

    /// 还原一条。还原前校验哈希：隔离期间被改动过就不放回去。
    ///
    /// 压缩归档的条目走另一条还原路径（解包），两者不能混：
    /// 把 `.tar.gz` 当目录 rename 回去，用户看到的会是一个包而不是他的文件。
    pub fn restore(&self, id: i64) -> Result<PathBuf, String> {
        let rec = self
            .store
            .find_pending(id)
            .map_err(|e| format!("查询记录失败：{e}"))?
            .ok_or_else(|| format!("找不到编号为 {id} 的待还原项"))?;

        if rec.restored {
            return Err(format!("编号 {id} 已经还原过了"));
        }
        if !rec.quarantine_path.exists() {
            return Err(format!(
                "隔离区里的内容已丢失：{}",
                rec.quarantine_path.display()
            ));
        }

        match rec.op {
            OpKind::Compress => {
                archive::unpack(&rec.quarantine_path, &rec.absolute_path)
                    .map_err(|e| format!("还原失败：{e}"))?;
                self.store
                    .record_restore(id, &now_iso8601())
                    .map_err(|e| format!("写审计失败：{e}"))?;
                return Ok(rec.absolute_path);
            }
            OpKind::Quarantine => {}
        }

        let current = hash::path_sha256(&rec.quarantine_path).unwrap_or_default();
        if let Some(expected) = &rec.sha256 {
            if !expected.is_empty() && &current != expected {
                return Err(format!(
                    "内容校验不一致，未还原：{}。隔离期间它被改动过，请人工确认。",
                    rec.quarantine_path.display()
                ));
            }
        }

        self.quarantine
            .move_out(&rec.quarantine_path, &rec.absolute_path)
            .map_err(|e| format!("还原失败：{e}"))?;

        self.store
            .record_restore(id, &now_iso8601())
            .map_err(|e| format!("写审计失败：{e}"))?;

        Ok(rec.absolute_path)
    }

    /// 压缩归档：打包、校验、删原件、写审计。
    ///
    /// 与 `clean` 的区别只在落库与打包；核对过的东西是同一批候选，
    /// 因此这里同样拒绝 `Protected`，也同样要求 `Confirmation`。
    ///
    /// 压缩比不划算时**不删原件**：用户要的是腾空间，一趟包比原件还大就不是腾空间。
    pub fn compress(&self, candidates: &[Candidate], confirmation: &Confirmation) -> CleanOutcome {
        let now = now_iso8601();
        let purge_after = add_days(&now, self.retain_days);
        let mut outcome = CleanOutcome::default();

        for c in candidates {
            if matches!(c.verdict, Verdict::Protected { .. }) {
                outcome.failed.push(FailedItem {
                    path: c.path.clone(),
                    reason: "受保护项，已跳过".to_string(),
                });
                continue;
            }
            if !confirmation.is_accepted(&c.path) {
                continue;
            }
            if c.path.as_os_str().is_empty() || !c.path.exists() {
                outcome.failed.push(FailedItem {
                    path: c.path.clone(),
                    reason: "路径已不存在".to_string(),
                });
                continue;
            }

            let bytes = c.alloc_bytes.max(c.bytes);
            let packed = match self.archive.pack(&c.path, self.next_id()) {
                Ok(p) => p,
                Err(e) => {
                    outcome.failed.push(FailedItem {
                        path: c.path.clone(),
                        reason: format!("打包失败，原件未动：{e}"),
                    });
                    continue;
                }
            };

            if packed.saved_bytes() <= 0 {
                // 包不比原件小：删掉包，原件留着。
                let _ = archive::purge(&packed.path);
                outcome.failed.push(FailedItem {
                    path: c.path.clone(),
                    reason: format!(
                        "压缩后没有变小（{} 字节 → {} 字节），原件保留",
                        packed.original_bytes, packed.bytes
                    ),
                });
                continue;
            }

            // 包已经校验通过，这时才动原件
            if let Err(e) = archive::Archive::purge_original(&c.path) {
                let _ = archive::purge(&packed.path);
                outcome.failed.push(FailedItem {
                    path: c.path.clone(),
                    reason: format!("包已生成但删不掉原件，已回滚：{e}"),
                });
                continue;
            }

            let record = audit::NewQuarantine {
                absolute: &c.path,
                quarantine: &packed.path,
                bytes,
                sha256: "",
                performed_at: &now,
                purge_after: &purge_after,
                source: self.source,
            };
            match self.store.record_archive(&record) {
                Ok(id) => outcome.moved.push(MovedItem {
                    id,
                    original: c.path.clone(),
                    quarantine: packed.path,
                    bytes,
                }),
                Err(e) => {
                    // 审计写不进去时不能只报错：原件已经删了。
                    // 这里只能先恢复原件（从包里解开），如果连这也失败，
                    // 就把包的位置写进原因，让用户自己拿得到。
                    let restored = archive::unpack(&packed.path, &c.path).is_ok();
                    let detail = if restored {
                        "写审计失败，原件已从包中恢复".to_string()
                    } else {
                        format!(
                            "写审计失败，且原件未能自动恢复；内容在 {}",
                            packed.path.display()
                        )
                    };
                    outcome.failed.push(FailedItem {
                        path: c.path.clone(),
                        reason: format!("{detail}：{e}"),
                    });
                }
            }
        }

        self.archive.tidy_batch();
        outcome
    }

    /// 清掉到期的隔离项。
    ///
    /// 每条成功都要进 `deleted`——否则调用方拿到的永远是 0，
    /// 而内容其实已经被删了。这个错误很难发现：现场看文件确实没了，
    /// 命令却报告「已清除 0 项」。**数字对不上比数字难看更伤信任。**
    pub fn purge_due(&self) -> CleanOutcome {
        let now = now_iso8601();
        let mut outcome = CleanOutcome::default();
        let due = match self.store.due_for_purge(&now, 10_000) {
            Ok(d) => d,
            Err(_) => return outcome,
        };
        for rec in due {
            let removed = match rec.op {
                OpKind::Compress => archive::purge(&rec.quarantine_path),
                OpKind::Quarantine => self.quarantine.purge(&rec.quarantine_path),
            };
            match removed {
                Ok(()) => match self.store.record_purge(rec.id, &now) {
                    Ok(()) => outcome.deleted.push(DeletedItem {
                        id: rec.id,
                        path: rec.absolute_path.clone(),
                        bytes: rec.bytes,
                    }),
                    // 内容已经没了，只是记录没写入：如实报，不假装成功
                    Err(e) => outcome.failed.push(FailedItem {
                        path: rec.absolute_path.clone(),
                        reason: format!("内容已删除，但写审计失败：{e}"),
                    }),
                },
                Err(e) => outcome.failed.push(FailedItem {
                    path: rec.quarantine_path.clone(),
                    reason: format!("清除失败：{e}"),
                }),
            }
        }

        self.quarantine.tidy_batch();
        outcome
    }

    /// 用户在隔离区里主动「彻底删除」若干条。
    ///
    /// 与 `purge_due` 的区别：这里不等到期，由用户逐条确认后执行，所以每条失败
    /// 都要带回原路径与原因——用户刚看着那份清单按下确认，得知道哪一条没成。
    ///
    /// 删内容与写审计不是原子的：先删、再记。审计写失败时内容已经没了，
    /// 只能如实记进 `failed`，不能假装成功——那会让隔离区里留下一条指向
    /// 已不存在文件的记录，用户下次点「还原」才发现。
    pub fn purge_ids(&self, ids: &[i64]) -> CleanOutcome {
        let now = now_iso8601();
        let mut outcome = CleanOutcome::default();

        for &id in ids {
            let rec = match self.store.find_pending(id) {
                Ok(Some(r)) => r,
                Ok(None) => {
                    // 已经被还原或已清除过了：不是失败，但也不能算这次删掉了。
                    continue;
                }
                Err(e) => {
                    outcome.failed.push(FailedItem {
                        path: PathBuf::from(format!("编号 {id}")),
                        reason: format!("读审计记录失败：{e}"),
                    });
                    continue;
                }
            };

            // 内容已经不在（比如被别的清理工具动了）：仍然要把记录收尾，
            // 否则它会在隔离区列表里永远挂着一条点不动的东西。
            let existed = rec.quarantine_path.exists()
                || std::fs::symlink_metadata(&rec.quarantine_path).is_ok();

            if existed {
                let removed = match rec.op {
                    OpKind::Compress => archive::purge(&rec.quarantine_path),
                    OpKind::Quarantine => self.quarantine.purge(&rec.quarantine_path),
                };
                if let Err(e) = removed {
                    outcome.failed.push(FailedItem {
                        path: rec.absolute_path.clone(),
                        reason: format!("删除隔离区内容失败：{e}"),
                    });
                    continue;
                }
            }

            match self.store.record_purge(id, &now) {
                Ok(()) => outcome.deleted.push(DeletedItem {
                    id,
                    path: rec.absolute_path.clone(),
                    bytes: rec.bytes,
                }),
                Err(e) => outcome.failed.push(FailedItem {
                    path: rec.absolute_path.clone(),
                    reason: format!("内容已删除，但写审计失败：{e}"),
                }),
            }
        }

        self.quarantine.tidy_batch();
        outcome
    }

    /// 只是取一个唯一的数字给隔离文件名用；真正的 id 由数据库分配。
    fn next_id(&self) -> i64 {
        use std::sync::atomic::{AtomicI64, Ordering};
        static COUNTER: AtomicI64 = AtomicI64::new(1);
        let base = COUNTER.fetch_add(1, Ordering::Relaxed);
        // 加上毫秒时间戳，避免进程重启后序号重复导致隔离目录撞名
        let millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64 % 1_000_000)
            .unwrap_or(0);
        base * 1_000_000 + millis
    }
}

/// ISO8601 时间戳，秒级，UTC。
pub fn now_iso8601() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    epoch_to_iso8601(secs as i64)
}

/// 把 epoch 秒转成 `YYYY-MM-DDTHH:MM:SSZ`。
///
/// 自己算而不引入 chrono：只服务这一个格式，多一个依赖不值得；
/// 而且这个算法可以用固定输入直接测，不会有「时区/夏令时」的歧义。
pub fn epoch_to_iso8601(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        y,
        m,
        d,
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// 在 ISO8601 时间上加天数。只支持正数天，够用且不会出现「负数天」的歧义。
pub fn add_days(iso: &str, days: i64) -> String {
    let secs = parse_iso8601(iso).unwrap_or(0);
    epoch_to_iso8601(secs + days * 86_400)
}

fn parse_iso8601(iso: &str) -> Option<i64> {
    let b = iso.as_bytes();
    if b.len() < 20 {
        return None;
    }
    let num = |s: &str| -> Option<i64> { s.parse().ok() };
    let y = num(&iso[0..4])?;
    let mo = num(&iso[5..7])?;
    let d = num(&iso[8..10])?;
    let h = num(&iso[11..13])?;
    let mi = num(&iso[14..16])?;
    let s = num(&iso[17..19])?;
    Some(days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + s)
}

/// Howard Hinnant 的 civil-from-days 算法（公有领域）。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let mp = if m > 2 { m - 3 } else { m + 9 } as u64;
    let doy = (153 * mp + 2) / 5 + d as u64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe as i64 - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;
    use molt_core::{CostKind, ItemKind, RebuildCost};

    fn tmpdir(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "molt-exec-{}-{}-{}",
            std::process::id(),
            tag,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn setup(tag: &str) -> (PathBuf, Executor) {
        let base = tmpdir(tag);
        let store = AuditStore::open(&base.join("audit.sqlite")).unwrap();
        let q = Quarantine::new(base.join("q"), "batch");
        (base.clone(), Executor::new(store, q, 7))
    }

    #[test]
    fn clean_moves_and_records() {
        let (base, ex) = setup("clean");
        let cache = base.join("cache");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(cache.join("a"), b"1234567890").unwrap();

        let c = Candidate::new(&cache, ItemKind::Dir, Verdict::Reclaimable).with_size(10, 4096, 1);
        let outcome = ex.clean(
            &[c],
            &Confirmation::new(&[Candidate::new(&cache, ItemKind::Dir, Verdict::Reclaimable)]),
        );

        assert_eq!(outcome.moved.len(), 1);
        assert!(outcome.failed.is_empty());
        assert!(!cache.exists(), "原位置应已空");
        assert!(outcome.moved[0].quarantine.exists());

        let pending = ex.store().list_pending(10).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].bytes, 4096);

        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn protected_items_are_never_moved() {
        let (base, ex) = setup("protected");
        let keep = base.join("sessions");
        std::fs::create_dir_all(&keep).unwrap();
        std::fs::write(keep.join("chat.json"), b"history").unwrap();

        let c = Candidate::new(
            &keep,
            ItemKind::Dir,
            Verdict::Protected {
                reason: molt_core::ProtectReason::UserRecord,
            },
        )
        .with_size(7, 4096, 1);
        let conf = Confirmation::new(std::slice::from_ref(&c));
        let outcome = ex.clean(std::slice::from_ref(&c), &conf);

        assert!(outcome.moved.is_empty());
        assert_eq!(outcome.failed.len(), 1);
        assert!(outcome.failed[0].reason.contains("受保护"));
        // 文件必须还在原地
        assert!(keep.join("chat.json").exists());

        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn restore_round_trips_every_byte() {
        let (base, ex) = setup("roundtrip");
        let cache = base.join("cache");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(cache.join("a.txt"), vec![7u8; 5000]).unwrap();

        let c =
            Candidate::new(&cache, ItemKind::Dir, Verdict::Reclaimable).with_size(5000, 8192, 1);
        let conf = Confirmation::new(std::slice::from_ref(&c));
        let outcome = ex.clean(std::slice::from_ref(&c), &conf);
        let id = outcome.moved[0].id;

        let restored = ex.restore(id).unwrap();
        assert_eq!(restored, cache);
        assert_eq!(std::fs::read(cache.join("a.txt")).unwrap(), vec![7u8; 5000]);

        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn compress_then_restore_returns_the_original_files() {
        let (base, ex) = setup("compress");
        let logs = base.join("logs");
        std::fs::create_dir_all(&logs).unwrap();
        // 用可压缩的内容：重复文本的压缩率才足以让「压缩划算」成立
        std::fs::write(logs.join("a.log"), "error line\n".repeat(2000)).unwrap();

        let c =
            Candidate::new(&logs, ItemKind::Dir, Verdict::Reclaimable).with_size(22_000, 22_000, 1);
        let conf = Confirmation::new(std::slice::from_ref(&c));
        let outcome = ex.compress(std::slice::from_ref(&c), &conf);

        assert_eq!(outcome.moved.len(), 1, "失败：{:?}", outcome.failed);
        assert!(!logs.exists(), "原件应已腾空");
        let rec = ex
            .store()
            .find_pending(outcome.moved[0].id)
            .unwrap()
            .unwrap();
        assert_eq!(rec.op, OpKind::Compress, "审计要记下这是压缩包");

        ex.restore(rec.id).unwrap();
        let text = std::fs::read_to_string(logs.join("a.log")).unwrap();
        assert_eq!(text, "error line\n".repeat(2000), "还原必须逐字节一致");

        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn compress_keeps_the_original_when_it_does_not_shrink() {
        let (base, ex) = setup("incompressible");
        let blob = base.join("blob");
        std::fs::create_dir_all(&blob).unwrap();
        // 已压缩过的数据：再压一遍不会变小。
        // 必须用真伪随机字节——`(i * K) >> 24` 是周期序列，gzip 能压到几十分之一，
        // 用它测「压不小」只会得到一个假通过。
        let mut state: u64 = 0x2545_F491_4F6C_DD1D;
        let random: Vec<u8> = (0..40_000)
            .map(|_| {
                // xorshift64
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                (state >> 33) as u8
            })
            .collect();
        std::fs::write(blob.join("a.bin"), &random).unwrap();

        let c = Candidate::new(&blob, ItemKind::Dir, Verdict::Reclaimable).with_size(
            random.len() as u64,
            random.len() as u64,
            1,
        );
        let conf = Confirmation::new(std::slice::from_ref(&c));
        let outcome = ex.compress(std::slice::from_ref(&c), &conf);

        assert!(outcome.moved.is_empty());
        assert!(blob.join("a.bin").exists(), "压缩不划算时必须保留原件");
        assert!(
            outcome.failed[0].reason.contains("没有变小"),
            "失败原因要说清是压缩不划算：{}",
            outcome.failed[0].reason
        );

        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn restore_detects_tampering() {
        let (base, ex) = setup("tamper");
        let cache = base.join("cache");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(cache.join("a.txt"), b"original").unwrap();

        let c = Candidate::new(&cache, ItemKind::Dir, Verdict::Reclaimable).with_size(8, 4096, 1);
        let conf = Confirmation::new(std::slice::from_ref(&c));
        let outcome = ex.clean(std::slice::from_ref(&c), &conf);
        let id = outcome.moved[0].id;

        // 模拟隔离期间被改动
        std::fs::write(outcome.moved[0].quarantine.join("evil.txt"), b"changed").unwrap();

        let err = ex.restore(id).unwrap_err();
        assert!(err.contains("校验不一致"), "实际错误：{err}");
        assert!(!cache.exists(), "校验失败时不能放回去");

        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn costly_items_can_be_cleaned_when_confirmed() {
        let (base, ex) = setup("costly");
        let target = base.join("target");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("bin"), b"x").unwrap();

        let c = Candidate::new(
            &target,
            ItemKind::Dir,
            Verdict::Costly {
                rebuild: RebuildCost::new(CostKind::Recompile, "全量重编"),
            },
        )
        .with_size(1, 4096, 1);
        let conf = Confirmation::new(std::slice::from_ref(&c));
        let outcome = ex.clean(std::slice::from_ref(&c), &conf);
        assert_eq!(outcome.moved.len(), 1);

        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn unconfirmed_candidates_are_skipped() {
        let (base, ex) = setup("unconfirmed");
        let cache = base.join("cache");
        std::fs::create_dir_all(&cache).unwrap();

        let c = Candidate::new(&cache, ItemKind::Dir, Verdict::Reclaimable).with_size(0, 0, 1);
        let empty = Confirmation::new(&[]);
        let outcome = ex.clean(&[c], &empty);

        assert!(outcome.moved.is_empty());
        assert!(cache.exists(), "未确认的项必须原地不动");

        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn confirmation_only_accepts_actionable_items() {
        let ok = Candidate::new("/a", ItemKind::Dir, Verdict::Reclaimable);
        let bad = Candidate::new(
            "/b",
            ItemKind::Dir,
            Verdict::Protected {
                reason: molt_core::ProtectReason::UserRecord,
            },
        );
        let conf = Confirmation::new(&[ok, bad]);
        assert_eq!(conf.len(), 1);
        assert!(conf.is_accepted(std::path::Path::new("/a")));
        assert!(!conf.is_accepted(std::path::Path::new("/b")));
    }

    #[test]
    fn delete_progress_counts_processed_and_skipped_candidates() {
        let base = tmpdir("delete-progress");
        let target = base.join("cache.bin");
        let keep = base.join("keep.bin");
        let later = base.join("later.bin");
        for path in [&target, &keep, &later] {
            std::fs::write(path, b"data").unwrap();
        }
        let ex = Executor::new(
            AuditStore::in_memory().unwrap(),
            Quarantine::new(base.join("q"), "b"),
            7,
        )
        .with_mode(CleanMode::Delete)
        .with_authorized_root(&base);
        let item = Candidate::new(&target, ItemKind::File, Verdict::Reclaimable);
        let protected = Candidate::new(
            &keep,
            ItemKind::File,
            Verdict::Protected {
                reason: molt_core::ProtectReason::UserRecord,
            },
        );
        let unconfirmed = Candidate::new(&later, ItemKind::File, Verdict::Reclaimable);
        let candidates = [item.clone(), item.clone(), protected, unconfirmed];
        let confirmation = Confirmation::new(std::slice::from_ref(&item));
        let mut observed = Vec::new();
        let outcome = ex.clean_with_progress(&candidates, &confirmation, |done, total| {
            if done == 1 {
                assert!(!target.exists(), "进度只能在文件删除后推进");
            }
            observed.push((done, total));
        });

        assert_eq!(observed, [(1, 4), (2, 4), (3, 4), (4, 4)]);
        assert_eq!(outcome.deleted.len(), 1);
        assert_eq!(outcome.failed.len(), 1);
        assert!(!target.exists());
        assert!(keep.exists());
        assert!(later.exists());
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn delete_mode_removes_files_and_reports_released_bytes() {
        let base = tmpdir("delete");
        let cache = base.join("cache");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(cache.join("a.bin"), vec![0u8; 8192]).unwrap();

        let store = AuditStore::in_memory().unwrap();
        let q = Quarantine::new(base.join("q"), "b");
        // 授权根指向临时目录：测试不该去动真实家目录，
        // 而清理范围本身在真实运行时就是家目录。
        let ex = Executor::new(store, q, 7)
            .with_mode(CleanMode::Delete)
            .with_authorized_root(&base);

        let c =
            Candidate::new(&cache, ItemKind::Dir, Verdict::Reclaimable).with_size(8192, 8192, 1);
        let conf = Confirmation::new(std::slice::from_ref(&c));
        let outcome = ex.clean(std::slice::from_ref(&c), &conf);

        assert_eq!(outcome.deleted.len(), 1, "应有一条删除记录");
        assert!(outcome.failed.is_empty(), "失败：{:?}", outcome.failed);
        assert!(!cache.exists(), "文件必须真的从磁盘上消失");
        assert_eq!(outcome.deleted_bytes(), 8192);
        // 永久删除不进隔离区，所以「可还原的」不该有东西
        assert!(outcome.moved.is_empty());
        assert_eq!(ex.store().list_pending(10).unwrap().len(), 0);

        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn delete_mode_refuses_paths_outside_the_authorized_root() {
        let base = tmpdir("outside");
        let other = tmpdir("outside-other");
        std::fs::create_dir_all(other.join("cache")).unwrap();

        let store = AuditStore::in_memory().unwrap();
        let q = Quarantine::new(base.join("q"), "b");
        let ex = Executor::new(store, q, 7)
            .with_mode(CleanMode::Delete)
            .with_authorized_root(&base);

        let target = other.join("cache");
        let c = Candidate::new(&target, ItemKind::Dir, Verdict::Reclaimable).with_size(10, 4096, 1);
        let conf = Confirmation::new(std::slice::from_ref(&c));
        let outcome = ex.clean(std::slice::from_ref(&c), &conf);

        assert!(outcome.deleted.is_empty(), "越界路径不能删");
        assert_eq!(outcome.failed.len(), 1);
        assert!(target.exists(), "越界目标必须还在");

        std::fs::remove_dir_all(&base).ok();
        std::fs::remove_dir_all(&other).ok();
    }

    #[test]
    fn quarantine_progress_counts_processed_and_skipped_candidates() {
        let (base, ex) = setup("quarantine-progress");
        let target = base.join("cache.bin");
        let keep = base.join("keep.bin");
        let later = base.join("later.bin");
        for path in [&target, &keep, &later] {
            std::fs::write(path, b"data").unwrap();
        }
        let item = Candidate::new(&target, ItemKind::File, Verdict::Reclaimable);
        let protected = Candidate::new(
            &keep,
            ItemKind::File,
            Verdict::Protected {
                reason: molt_core::ProtectReason::UserRecord,
            },
        );
        let unconfirmed = Candidate::new(&later, ItemKind::File, Verdict::Reclaimable);
        let candidates = [item.clone(), protected, unconfirmed];
        let confirmation = Confirmation::new(std::slice::from_ref(&item));
        let mut observed = Vec::new();
        let outcome = ex.clean_with_progress(&candidates, &confirmation, |done, total| {
            if done == 1 {
                assert!(!target.exists(), "进度只能在文件移入临时空间后推进");
            }
            observed.push((done, total));
        });

        assert_eq!(observed, [(1, 3), (2, 3), (3, 3)]);
        assert_eq!(outcome.moved.len(), 1);
        assert_eq!(outcome.failed.len(), 1);
        assert!(!target.exists());
        assert!(keep.exists());
        assert!(later.exists());
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn quarantine_mode_does_not_report_released_bytes() {
        // 这条守的是「移入隔离区不释放空间」这个事实：
        // freed_bytes 会算上它（内容确实离开了原位置），但 deleted_bytes 必须是 0，
        // 界面用后者说「释放了多少」，才不会出现清完空间没变的信任事故。
        let (base, _) = setup("nomove");
        let store = AuditStore::in_memory().unwrap();
        let q = Quarantine::new(base.join("q"), "b");
        let ex = Executor::new(store, q, 7);

        let cache = base.join("cache");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(cache.join("a"), b"1234567890").unwrap();
        let c = Candidate::new(&cache, ItemKind::Dir, Verdict::Reclaimable).with_size(10, 4096, 1);
        let conf = Confirmation::new(std::slice::from_ref(&c));
        let outcome = ex.clean(std::slice::from_ref(&c), &conf);

        assert_eq!(outcome.moved.len(), 1);
        assert_eq!(outcome.deleted_bytes(), 0, "隔离不释放空间");

        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn protected_items_are_never_deleted_permanently() {
        let base = tmpdir("keep");
        let store = AuditStore::in_memory().unwrap();
        let q = Quarantine::new(base.join("q"), "b");
        let ex = Executor::new(store, q, 7).with_mode(CleanMode::Delete);

        let keep = base.join("sessions");
        std::fs::create_dir_all(&keep).unwrap();
        std::fs::write(keep.join("chat.json"), b"history").unwrap();

        let c = Candidate::new(
            &keep,
            ItemKind::Dir,
            Verdict::Protected {
                reason: molt_core::ProtectReason::UserRecord,
            },
        )
        .with_size(7, 4096, 1);
        let conf = Confirmation::new(std::slice::from_ref(&c));
        let outcome = ex.clean(std::slice::from_ref(&c), &conf);

        assert!(outcome.deleted.is_empty());
        assert!(keep.join("chat.json").exists(), "受保护内容必须原地不动");

        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn iso8601_round_trips() {
        assert_eq!(epoch_to_iso8601(0), "1970-01-01T00:00:00Z");
        assert_eq!(parse_iso8601("1970-01-01T00:00:00Z"), Some(0));
        let now = now_iso8601();
        assert!(parse_iso8601(&now).is_some());
    }

    #[test]
    fn add_days_crosses_month_boundaries() {
        assert_eq!(add_days("2026-10-27T10:00:00Z", 7), "2026-11-03T10:00:00Z");
        assert_eq!(add_days("2026-12-31T23:00:00Z", 1), "2027-01-01T23:00:00Z");
    }

    #[test]
    fn purge_due_removes_expired_items() {
        let base = tmpdir("purge");
        let store = AuditStore::open(&base.join("audit.sqlite")).unwrap();
        let q = Quarantine::new(base.join("q"), "batch");
        // 保留 0 天 → 立即到期
        let ex = Executor::new(store, q, 0);

        let cache = base.join("cache");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(cache.join("a"), b"1").unwrap();
        let c = Candidate::new(&cache, ItemKind::Dir, Verdict::Reclaimable).with_size(1, 4096, 1);
        let conf = Confirmation::new(std::slice::from_ref(&c));
        let outcome = ex.clean(std::slice::from_ref(&c), &conf);
        let qp = outcome.moved[0].quarantine.clone();

        ex.purge_due();
        assert!(!qp.exists(), "到期项应被真正删除");

        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn purge_due_reports_what_it_actually_deleted() {
        // 回归测试：这里曾经只删文件、不往 outcome 里记录，
        // 命令于是永远打印「已清除 0 项」，而磁盘上东西已经没了。
        let base = tmpdir("purgecount");
        let store = AuditStore::open(&base.join("audit.sqlite")).unwrap();
        let q = Quarantine::new(base.join("q"), "batch");
        let ex = Executor::new(store, q, 0);

        let cache = base.join("cache");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(cache.join("a"), vec![0u8; 4096]).unwrap();
        let c =
            Candidate::new(&cache, ItemKind::Dir, Verdict::Reclaimable).with_size(4096, 4096, 1);
        let conf = Confirmation::new(std::slice::from_ref(&c));
        ex.clean(std::slice::from_ref(&c), &conf);

        let outcome = ex.purge_due();
        assert_eq!(outcome.deleted.len(), 1, "必须报告真实删除条数");
        assert_eq!(outcome.deleted_bytes(), 4096, "必须报告真实释放字节");
        assert!(outcome.failed.is_empty());

        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn purge_ids_removes_content_and_leaves_the_pending_list() {
        let (base, ex) = setup("purgeids");
        let cache = base.join("cache");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(cache.join("a"), b"1234").unwrap();

        let c = Candidate::new(&cache, ItemKind::Dir, Verdict::Reclaimable).with_size(4, 4096, 1);
        let conf = Confirmation::new(std::slice::from_ref(&c));
        let moved = ex.clean(std::slice::from_ref(&c), &conf).moved[0].clone();

        let outcome = ex.purge_ids(&[moved.id]);
        assert_eq!(outcome.deleted.len(), 1, "失败：{:?}", outcome.failed);
        assert!(outcome.failed.is_empty());
        assert!(!moved.quarantine.exists(), "隔离区内容必须真的没了");
        assert!(
            ex.store().list_pending(10).unwrap().is_empty(),
            "已彻底删除的条目不能继续挂在待还原列表里"
        );

        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn purge_ids_keeps_going_when_one_item_is_missing() {
        // 一条内容已经不在（被别的工具清掉 / 手工删了），其余照常处理，
        // 并且那条失效记录也要收尾，不能永远留在列表里。
        let (base, ex) = setup("purgepartial");
        let a = base.join("a");
        let b = base.join("b");
        for p in [&a, &b] {
            std::fs::create_dir_all(p).unwrap();
            std::fs::write(p.join("f"), b"x").unwrap();
        }
        let ca = Candidate::new(&a, ItemKind::Dir, Verdict::Reclaimable).with_size(1, 4096, 1);
        let cb = Candidate::new(&b, ItemKind::Dir, Verdict::Reclaimable).with_size(1, 4096, 1);
        let conf = Confirmation::new(&[ca.clone(), cb.clone()]);
        let moved = ex.clean(&[ca, cb], &conf).moved;

        std::fs::remove_dir_all(&moved[0].quarantine).unwrap();

        let outcome = ex.purge_ids(&[moved[0].id, moved[1].id]);
        assert_eq!(outcome.deleted.len(), 2, "失败：{:?}", outcome.failed);
        assert!(outcome.failed.is_empty());
        assert!(ex.store().list_pending(10).unwrap().is_empty());

        std::fs::remove_dir_all(&base).ok();
    }
}
