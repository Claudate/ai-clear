//! AI 分析的后台任务。与 `worker.rs` 一样的原因：一次分析要几十秒，
//! 放在 UI 线程里窗口会整段无响应，而用户正准备对磁盘动手——那时卡住最像崩溃。
//!
//! 分析链路按需求分成七段，其中四段会真的花时间：
//! 扫描（本地，复用 `worker::ScanJob`）、摘要（本地）、分析（网络）、
//! 生成方案（本地）、执行（写盘，复用 `molt-act`）。
//! 这里只包住「网络 + 方案生成」这一段，因为另外几段各自已有入口。
//!
//! 关键约束：**AI 那一步失败不影响本地扫描与手动处理**。
//! 所以 `AiJob` 不持有扫描结果，失败了也只是把自己标成失败，
//! 不复位 `AppState::result`。

use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

use molt_ai::{AiConfig, AiReport, Plan, PlanRoots};
use molt_core::Candidate;

/// 分析进行到哪一步。界面据此显示不同的 loading 文案——
/// 「正在联系 Claude」和「正在整理方案」对用户是两件事，
/// 前者可能要等很久，后者是本地计算。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AiStage {
    /// 正在压缩摘要
    Digesting,
    /// 请求已发出，等模型回复
    Calling { model: String },
    /// 回复已到，正在解析与核对
    Building,
}

/// 一次分析的进展。
#[derive(Debug, Clone)]
pub enum AiProgress {
    Stage(AiStage),
    /// 成功。`plan` 还未执行任何写操作。
    Done(Arc<AiOutcome>),
    /// 失败。原因要能直接显示给用户。
    Failed(AiFailure),
}

/// 失败分两类：配置类（用户可自己修）与运行类（网络/上游）。
/// 分开是因为界面要给不同的下一步：前者引导去配置，后者给「重试」。
#[derive(Debug, Clone)]
pub struct AiFailure {
    pub message: String,
    /// 是否属于「去配置页就能解决」
    pub fixable_by_config: bool,
    /// 模型的原始回复（解析失败时有），用于让用户看到它到底说了什么
    pub raw: Option<String>,
}

/// 一次分析的全部产出。
#[derive(Debug, Clone)]
pub struct AiOutcome {
    pub report: AiReport,
    pub plan: Plan,
    /// 扫描时的绝对路径 → 一句话解释。覆盖整份扫描结果，供清单页逐行显示。
    pub notes: std::collections::HashMap<std::path::PathBuf, String>,
    /// 摘要里发出去多少条、采样了多少个文件——用户有权知道发了什么
    pub sent_items: usize,
    pub sampled_files: usize,
    pub est_tokens: usize,
    /// 用的哪家哪个模型，界面要显示出来（否则用户会怀疑配置没生效）
    pub model: String,
}

/// 一次正在跑的分析。
pub struct AiJob {
    rx: Receiver<AiProgress>,
    latest: AiProgress,
    finished: bool,
}

impl AiJob {
    /// 开一次分析。立刻返回。
    ///
    /// `resolved` 已经在调用前校验过（配置错误要在点按钮之前就显示出来，
    /// 而不是等用户等十几秒再告诉他没配密钥）。
    pub fn start(
        resolved: molt_ai::Resolved,
        config_snapshot: AiConfig,
        candidates: Vec<Candidate>,
        home: String,
        disk: Option<molt_scan::disk::DiskUsage>,
    ) -> Self {
        let (tx, rx) = channel();
        let stage = AiStage::Digesting;
        let first = stage.clone();

        std::thread::spawn(move || {
            let _ = tx.send(AiProgress::Stage(AiStage::Digesting));

            let policy = if config_snapshot.allow_content_sample {
                Some(molt_ai::ContentPolicy::default())
            } else {
                None
            };
            let mut digest = molt_ai::Digest::build(&home, &candidates, disk.as_ref(), policy);
            digest.fit_to(resolved.context_tokens);

            let _ = tx.send(AiProgress::Stage(AiStage::Calling {
                model: resolved.model_label.clone(),
            }));

            let reply = match molt_ai::client::Request::new(&resolved, &digest).send() {
                Ok(text) => text,
                Err(e) => {
                    let _ = tx.send(AiProgress::Failed(AiFailure {
                        message: e.to_string(),
                        fixable_by_config: matches!(
                            e,
                            molt_ai::client::AiError::Status { code: 401, .. }
                                | molt_ai::client::AiError::Status { code: 403, .. }
                                | molt_ai::client::AiError::Status { code: 404, .. }
                        ),
                        raw: None,
                    }));
                    return;
                }
            };

            let _ = tx.send(AiProgress::Stage(AiStage::Building));

            let report = match AiReport::parse(&reply) {
                Ok(r) => r,
                Err(e) => {
                    let _ = tx.send(AiProgress::Failed(AiFailure {
                        message: e.to_string(),
                        fixable_by_config: false,
                        raw: Some(e.raw),
                    }));
                    return;
                }
            };

            let batch = molt_act::now_iso8601().replace(':', "-");
            let roots = PlanRoots::standard(batch);
            let plan = Plan::build(&report, &candidates, &home, &roots, resolved.summary());
            let notes = molt_ai::explanations(&report, &candidates, &home);

            let _ = tx.send(AiProgress::Done(Arc::new(AiOutcome {
                sent_items: digest.items.len(),
                sampled_files: digest.items.iter().filter(|i| i.sample.is_some()).count(),
                est_tokens: digest.est_tokens,
                model: resolved.summary(),
                report,
                plan,
                notes,
            })));
        });

        Self {
            rx,
            latest: AiProgress::Stage(first),
            finished: false,
        }
    }

    /// 排空通道，只留最后一条。与 `ScanJob::poll` 同一套语义。
    pub fn poll(&mut self) -> &AiProgress {
        while let Ok(p) = self.rx.try_recv() {
            self.latest = p;
        }
        if matches!(self.latest, AiProgress::Done(_) | AiProgress::Failed(_)) {
            self.finished = true;
        }
        &self.latest
    }

    pub fn peek(&self) -> &AiProgress {
        &self.latest
    }

    pub fn is_finished(&self) -> bool {
        self.finished
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    #[test]
    fn poll_drains_to_the_last_progress() {
        let (tx, rx) = channel();
        tx.send(AiProgress::Stage(AiStage::Digesting)).unwrap();
        tx.send(AiProgress::Stage(AiStage::Building)).unwrap();
        tx.send(AiProgress::Done(Arc::new(AiOutcome {
            report: AiReport::default(),
            plan: Plan::default(),
            notes: Default::default(),
            sent_items: 1,
            sampled_files: 0,
            est_tokens: 10,
            model: "m".into(),
        })))
        .unwrap();

        let mut job = AiJob {
            rx,
            latest: AiProgress::Stage(AiStage::Digesting),
            finished: false,
        };
        assert!(matches!(job.poll(), AiProgress::Done(_)));
        assert!(job.is_finished());
    }

    #[test]
    fn failure_is_terminal_but_does_not_touch_scan_results() {
        let (tx, rx) = channel();
        tx.send(AiProgress::Failed(AiFailure {
            message: "401".into(),
            fixable_by_config: true,
            raw: None,
        }))
        .unwrap();

        let mut job = AiJob {
            rx,
            latest: AiProgress::Stage(AiStage::Calling { model: "m".into() }),
            finished: false,
        };
        job.poll();
        assert!(job.is_finished());
        match job.peek() {
            AiProgress::Failed(f) => assert!(f.fixable_by_config),
            other => panic!("应为失败，实际 {other:?}"),
        }
    }
}
