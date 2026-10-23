//! 扫描摘要：交给 AI 的那一小份事实。
//!
//! 这一步是整个功能的成本闸门。设计目标是「一份摘要的 token 量与磁盘大小无关」：
//!
//! - 只发**元数据**：路径（相对家目录）、类型、逻辑大小、实际占用、文件数、
//!   最后修改/访问时间、判定、命中的规则。这些字段在扫描阶段本来就量好了，
//!   再读一遍文件也不会更准。
//! - 条目数有硬上限，超出按体积取前 N 并如实告诉 AI「还有多少条被省略」——
//!   让它知道这是抽样，而不是全部。
//! - **读内容默认关闭**，且即使打开也只对「满足触发条件」的文件读，并受
//!   每文件字节、总字节、文件数三个上限同时约束（见 `ContentPolicy`）。
//! - 读到的内容先过 `molt-safety` 的密钥指纹：命中就整段丢掉，只上报
//!   「这里疑似有密钥」——密钥绝不能因为一次「分析」被发到第三方。

use std::path::{Path, PathBuf};

use molt_core::{Candidate, Verdict};

/// 内容采样的硬上限。三个上限同时生效，任一先到就停。
#[derive(Debug, Clone, Copy)]
pub struct ContentPolicy {
    /// 只有小于这个尺寸的文件才会被采样。大文件一律只看元数据。
    pub max_file_bytes: u64,
    /// 每个文件最多读这么多字节。
    pub per_file_bytes: usize,
    /// 一次分析总共最多读这么多字节。
    pub total_bytes: usize,
    /// 最多采样多少个文件。
    pub max_files: usize,
}

impl Default for ContentPolicy {
    fn default() -> Self {
        Self {
            max_file_bytes: 256 * 1024,
            per_file_bytes: 4096,
            total_bytes: 32 * 1024,
            max_files: 24,
        }
    }
}

/// 摘要里的一条。字段名短，因为每个字段都要占 token。
#[derive(Debug, Clone, serde::Serialize)]
pub struct DigestItem {
    /// 相对家目录的显示路径，例如 `~/project/x/target`
    pub path: String,
    pub kind: &'static str,
    pub bytes: u64,
    pub alloc: u64,
    pub files: u64,
    /// 最后修改时间，ISO 日期（只到天，够 AI 判断「多久没用」）
    pub modified: String,
    /// `reclaimable` / `costly` / `protected`
    pub verdict: &'static str,
    /// 判定说明（有代价时是重建代价，受保护时是原因）
    pub why: String,
    pub rule: String,
    /// 采样到的头部内容。仅在明确允许且满足条件时才有。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample: Option<String>,
    /// 采样被跳过的原因（命中密钥 / 太大 / 二进制）。让 AI 知道「这里没看内容」
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_skipped: Option<&'static str>,
}

/// 一次分析的完整摘要。
#[derive(Debug, Clone, serde::Serialize)]
pub struct Digest {
    pub home: String,
    pub generated_at: String,
    pub disk_total: u64,
    pub disk_available: u64,
    /// 交给 AI 的条目，已按体积降序
    pub items: Vec<DigestItem>,
    /// 因为上限而被省略的条目数
    pub omitted: usize,
    /// 是否启用了内容采样
    pub sampled: bool,
    /// 摘要本身的粗略 token 估算（按 4 字符 ≈ 1 token）
    pub est_tokens: usize,
}

impl Digest {
    /// 按模型窗口裁剪：超了就继续砍条目，绝不把超长请求发出去。
    ///
    /// 留出 35% 给提示词与回复：摘要占满窗口时模型没有空间产出 JSON，
    /// 结果是「调用成功但解析失败」，比直接裁掉更难排查。
    pub fn fit_to(&mut self, context_tokens: u32) {
        let budget = (context_tokens as usize * 65 / 100).max(2_000);
        let mut dropped = 0;
        while self.est_tokens > budget && self.items.len() > 1 {
            self.items.pop();
            dropped += 1;
            self.est_tokens = estimate_tokens(self);
        }
        self.omitted += dropped;
    }
}

/// 粗估 token。中文按 1.5 字符/token、ASCII 按 3.5 字符/token 折中取 2.5。
/// 宁可高估：低估会让请求被上游拒绝。
fn estimate_tokens(d: &Digest) -> usize {
    let json = serde_json::to_string(d).unwrap_or_default();
    json.chars().count() / 2 + 200
}

/// 摘要里最多放这么多条。与窗口无关的硬上限：
/// 200 条足够让 AI 看出「有几个大件、谁在重复」，再多只会让它开始编。
pub const MAX_ITEMS: usize = 120;

impl Digest {
    /// 从扫描结果生成摘要。
    ///
    /// `items` 已经是扫描后的候选；这里只挑「大的、要动手的」——
    /// 受保护项也带上，但只带少量最大的几个，因为用户真正想知道的是
    /// 「这些大件里哪些不能删、为什么」。
    pub fn build(
        home: &str,
        items: &[Candidate],
        disk: Option<&molt_scan::disk::DiskUsage>,
        policy: Option<ContentPolicy>,
    ) -> Self {
        let mut sorted: Vec<&Candidate> = items.iter().collect();
        sorted.sort_by_key(|c| std::cmp::Reverse(c.alloc_bytes.max(c.bytes)));

        let total = sorted.len();
        let keep = total.min(MAX_ITEMS);
        let mut sampled_files = 0usize;
        let mut sampled_bytes = 0usize;

        let mut out: Vec<DigestItem> = Vec::with_capacity(keep);
        for c in sorted.into_iter().take(keep) {
            let mut item = DigestItem {
                path: display(home, &c.path),
                kind: kind_label(c.kind),
                bytes: c.bytes,
                alloc: c.alloc_bytes,
                files: c.files,
                modified: c
                    .last_used
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| date_of(d.as_secs() as i64))
                    .unwrap_or_else(|| "未知".into()),
                verdict: verdict_label(&c.verdict),
                why: c.verdict.explanation(),
                rule: c.rule_id.clone().unwrap_or_default(),
                sample: None,
                sample_skipped: None,
            };

            if let Some(policy) = policy {
                if sampled_files < policy.max_files && sampled_bytes < policy.total_bytes {
                    match sample_head(&c.path, c, policy) {
                        Ok(Some(text)) => {
                            sampled_bytes += text.len();
                            sampled_files += 1;
                            item.sample = Some(text);
                        }
                        Ok(None) => {}
                        Err(reason) => item.sample_skipped = Some(reason),
                    }
                } else {
                    item.sample_skipped = Some("超出本次采样额度");
                }
            }

            out.push(item);
        }

        let mut d = Self {
            home: home.to_string(),
            generated_at: molt_act_now(),
            disk_total: disk.map(|d| d.total).unwrap_or(0),
            disk_available: disk.map(|d| d.available).unwrap_or(0),
            items: out,
            omitted: total.saturating_sub(keep),
            sampled: policy.is_some(),
            est_tokens: 0,
        };
        d.est_tokens = estimate_tokens(&d);
        d
    }
}

/// 读一个文件的开头。返回 `Ok(None)` 表示「按策略不该读」，`Err` 表示
/// 「该读但没读成，原因是这个」——两者在给 AI 的摘要里含义不同。
fn sample_head(
    path: &Path,
    c: &Candidate,
    policy: ContentPolicy,
) -> Result<Option<String>, &'static str> {
    // 触发条件一：只看文件，目录不猜内容
    if !matches!(c.kind, molt_core::ItemKind::File) {
        return Ok(None);
    }
    // 触发条件二：大文件一律不读
    if c.bytes > policy.max_file_bytes {
        return Err("文件过大，未读取内容");
    }
    let meta = std::fs::symlink_metadata(path).map_err(|_| "读不到文件信息")?;
    if meta.file_type().is_symlink() {
        return Ok(None);
    }
    let bytes = std::fs::read(path).map_err(|_| "没有读取权限")?;
    let head: &[u8] = &bytes[..bytes.len().min(policy.per_file_bytes)];

    // 触发条件三：只读文本。二进制开头出现 NUL 就整段跳过，
    // 把二进制当 UTF-8 塞进提示词既浪费 token 也没有信息量。
    if head.contains(&0) {
        return Err("二进制文件，未读取内容");
    }
    let text = String::from_utf8_lossy(head).to_string();

    // 触发条件四：**命中密钥指纹就整段丢弃**。
    // 这是这一层最重要的一条：分析功能不能成为密钥外流的通道。
    if looks_like_secret(&text) {
        return Err("疑似包含密钥，已拒绝上传");
    }
    Ok(Some(text))
}

/// 朴素但保守的密钥判定：宁可多拦。
///
/// 与 `molt-safety::fingerprint` 的取舍相反——那里要精确（避免误判整个目录），
/// 这里要安全（误判只是少一份样本，代价可以忽略）。
fn looks_like_secret(text: &str) -> bool {
    const MARKERS: [&str; 10] = [
        "BEGIN PRIVATE KEY",
        "BEGIN RSA PRIVATE KEY",
        "BEGIN OPENSSH PRIVATE KEY",
        "sk-",
        "AKIA",
        "api_key",
        "apikey",
        "password",
        "secret",
        "token",
    ];
    let lower = text.to_lowercase();
    MARKERS.iter().any(|m| lower.contains(&m.to_lowercase()))
}

fn kind_label(k: molt_core::ItemKind) -> &'static str {
    match k {
        molt_core::ItemKind::File => "file",
        molt_core::ItemKind::Dir => "dir",
        molt_core::ItemKind::Symlink => "symlink",
        molt_core::ItemKind::SqliteCluster => "sqlite",
    }
}

fn verdict_label(v: &Verdict) -> &'static str {
    match v {
        Verdict::Reclaimable => "reclaimable",
        Verdict::Costly { .. } => "costly",
        Verdict::Protected { .. } => "protected",
    }
}

fn display(home: &str, path: &Path) -> String {
    molt_act_display(home, path)
}

/// 本层不做路径缩写的自有实现：目标只是给 AI 一份短字符串，
/// 不引入对写盘层的依赖。
fn molt_act_display(home: &str, path: &Path) -> String {
    match path.strip_prefix(Path::new(home)) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

/// 当前 UTC 时间，ISO 8601（秒级）。只用于摘要的时间戳。
fn molt_act_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    format_utc(secs)
}

/// Unix 秒 → `YYYY-MM-DDTHH:MM:SSZ`。不引时间库：只需要一种格式。
fn format_utc(secs: i64) -> String {
    let (y, mo, d, h, mi, s) = civil_from_unix(secs);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

fn date_of(secs: i64) -> String {
    let (y, mo, d, ..) = civil_from_unix(secs);
    format!("{y:04}-{mo:02}-{d:02}")
}

/// 公历换算（Howard Hinnant 算法）。避免为一行日期引入 chrono。
fn civil_from_unix(secs: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (
        if m <= 2 { y + 1 } else { y },
        m as u32,
        d as u32,
        (rem / 3600) as u32,
        ((rem % 3600) / 60) as u32,
        (rem % 60) as u32,
    )
}

/// 未使用时也保留：`molt_ai::Digest` 的构造方可能用到。
#[allow(dead_code)]
fn _paths() -> PathBuf {
    PathBuf::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use molt_core::{ItemKind, Verdict};

    fn cand(path: &str, bytes: u64, v: Verdict) -> Candidate {
        Candidate::new(path, ItemKind::Dir, v).with_size(bytes, bytes, 3)
    }

    fn tmpdir(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "molt-ai-digest-{}-{}-{}",
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

    #[test]
    fn metadata_only_by_default() {
        let items = vec![cand("/Users/me/.npm", 100, Verdict::Reclaimable)];
        let d = Digest::build("/Users/me", &items, None, None);
        assert!(!d.sampled);
        assert!(d.items[0].sample.is_none());
        assert_eq!(d.items[0].path, "~/.npm");
    }

    #[test]
    fn items_are_sorted_by_size_and_capped() {
        let mut items: Vec<Candidate> = (0..MAX_ITEMS + 20)
            .map(|i| {
                cand(
                    &format!("/Users/me/c{i}"),
                    i as u64 * 1024,
                    Verdict::Reclaimable,
                )
            })
            .collect();
        items.push(cand("/Users/me/huge", 1 << 40, Verdict::Reclaimable));

        let d = Digest::build("/Users/me", &items, None, None);
        assert_eq!(d.items.len(), MAX_ITEMS);
        assert_eq!(d.items[0].path, "~/huge", "最大的排第一");
        assert!(d.omitted > 0, "超出部分要如实告知");
    }

    #[test]
    fn content_sample_respects_the_size_trigger() {
        let dir = tmpdir("big");
        let f = dir.join("big.bin");
        std::fs::write(&f, vec![b'a'; 4096]).unwrap();
        let c = Candidate::new(&f, ItemKind::File, Verdict::Reclaimable).with_size(4096, 4096, 1);

        let policy = ContentPolicy {
            max_file_bytes: 1024, // 比文件小
            ..Default::default()
        };
        let d = Digest::build(dir.to_str().unwrap(), &[c], None, Some(policy));
        assert!(d.items[0].sample.is_none());
        assert_eq!(d.items[0].sample_skipped, Some("文件过大，未读取内容"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn content_sample_caps_bytes_per_file() {
        let dir = tmpdir("cap");
        let f = dir.join("a.txt");
        std::fs::write(&f, "x".repeat(9000)).unwrap();
        let c = Candidate::new(&f, ItemKind::File, Verdict::Reclaimable).with_size(9000, 9000, 1);

        let policy = ContentPolicy {
            max_file_bytes: 64 * 1024,
            per_file_bytes: 100,
            ..Default::default()
        };
        let d = Digest::build(dir.to_str().unwrap(), &[c], None, Some(policy));
        let s = d.items[0].sample.as_ref().expect("应采到内容");
        assert_eq!(s.len(), 100, "每文件字节上限必须生效");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn secrets_are_never_uploaded_even_when_sampling() {
        let dir = tmpdir("secret");
        let f = dir.join("config.txt");
        std::fs::write(&f, "openai api_key = sk-abcdef123456\n").unwrap();
        let c = Candidate::new(&f, ItemKind::File, Verdict::Reclaimable).with_size(40, 40, 1);

        let d = Digest::build(
            dir.to_str().unwrap(),
            &[c],
            None,
            Some(ContentPolicy::default()),
        );
        assert!(d.items[0].sample.is_none(), "命中密钥的文件不能被采样上传");
        assert_eq!(d.items[0].sample_skipped, Some("疑似包含密钥，已拒绝上传"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn binary_files_are_not_sampled() {
        let dir = tmpdir("bin");
        let f = dir.join("a.bin");
        std::fs::write(&f, [0u8, 1, 2, 3, 4, 5]).unwrap();
        let c = Candidate::new(&f, ItemKind::File, Verdict::Reclaimable).with_size(6, 6, 1);

        let d = Digest::build(
            dir.to_str().unwrap(),
            &[c],
            None,
            Some(ContentPolicy::default()),
        );
        assert_eq!(d.items[0].sample_skipped, Some("二进制文件，未读取内容"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn fit_to_trims_items_for_small_contexts() {
        let items: Vec<Candidate> = (0..MAX_ITEMS)
            .map(|i| cand(&format!("/Users/me/c{i}"), i as u64, Verdict::Reclaimable))
            .collect();
        let mut d = Digest::build("/Users/me", &items, None, None);
        let before = d.items.len();
        d.fit_to(4_000);
        assert!(d.items.len() < before, "小窗口必须裁条目");
        assert!(d.omitted >= before - d.items.len());
    }

    #[test]
    fn timestamps_are_iso_utc() {
        // 2026-10-22T00:00:00Z
        assert_eq!(format_utc(1_792_627_200), "2026-10-22T00:00:00Z");
    }
}
