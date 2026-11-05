//! 批量 / 单个新建文件夹与文件——产品里第一个**写**动作。
//!
//! 与清理的区别是根本性的：清理面对的是「已经存在的东西」，判错的后果是丢数据，
//! 但至少范围由扫描结果限定；创建面对的是「用户手写的路径」，一个输入错误
//! 就可能把文件写到 `~/.ssh` 里。所以这里的闸与清理那一套不共用：
//!
//! - 七道闸全部在 [`CreatePlan::preflight`] 里，**执行阶段只按预检结果做**，
//!   预览里显示的数字与真正创建的数量永远一致，不存在「预览说 4 项、实际建了 5 项」。
//! - 硬禁区复用 [`molt_safety::deny`]，不另写一份黑名单——两处各写一份必然分叉，
//!   而这里写错就是往钥匙串目录里建文件。
//! - 永不覆盖：目标已存在只跳过。创建是加法，不该顺手改掉已有内容。
//!
//! 撤销只能删「自己刚建的空东西」：目录必须仍为空、文件必须仍是 0 字节、
//! mtime 与记录一致。三条任一不满足就跳过并说明，绝不猜测。

use std::path::{Component, Path, PathBuf};

use crate::audit::AuditStore;

/// 单批上限。超过这个数就不是「铺个骨架」，而是一次误粘贴。
pub const MAX_ITEMS: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateKind {
    Dir,
    File,
}

impl CreateKind {
    pub fn as_str(self) -> &'static str {
        match self {
            CreateKind::Dir => "dir",
            CreateKind::File => "file",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            CreateKind::Dir => "新建文件夹",
            CreateKind::File => "新建文件",
        }
    }

    /// 从审计库里的字符串还原。未知值按文件处理——它更保守：
    /// 撤销时文件的校验（0 字节 + mtime）比目录（空目录）更难误伤。
    pub fn parse(s: &str) -> Self {
        match s {
            "dir" => CreateKind::Dir,
            _ => CreateKind::File,
        }
    }
}

/// 用户想建的一项。
#[derive(Debug, Clone)]
pub struct CreateSpec {
    pub path: PathBuf,
    pub kind: CreateKind,
}

impl CreateSpec {
    pub fn dir(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            kind: CreateKind::Dir,
        }
    }

    pub fn file(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            kind: CreateKind::File,
        }
    }
}

/// 把文本框里的一行行路径翻译成规格。
///
/// 语法只有一条：**行尾 `/` 是文件夹，否则是文件**。用户不用记第二种规则，
/// 预览区会把这条规则直接写成中文。
pub fn parse_lines(text: &str) -> Vec<CreateSpec> {
    let mut out = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.ends_with('/') {
            out.push(CreateSpec::dir(PathBuf::from(line)));
        } else {
            out.push(CreateSpec::file(PathBuf::from(line)));
        }
    }
    out
}

/// 预检结果。界面上的「将创建 / 跳过 / 拒绝」三个数字全部来自这里。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpecCheck {
    WillCreate,
    /// 已存在。**不覆盖**，只跳过并告诉用户。
    SkipExists,
    /// 命中硬禁区 / 越出根目录 / 含 `..` / 非法路径。
    Rejected(String),
}

#[derive(Debug, Clone)]
pub struct CreatePlan {
    pub root: PathBuf,
    pub items: Vec<(CreateSpec, SpecCheck)>,
}

impl CreatePlan {
    /// 纯只读：不碰文件系统写入，只做检查。预览与执行都用它的结果。
    ///
    /// `home` 由调用方传入而不是在这里读环境变量，是为了让判定能被单测精确覆盖。
    pub fn preflight(root: &Path, specs: &[CreateSpec], home: &str) -> Self {
        let root = expand(root, home);
        let items = specs
            .iter()
            .map(|s| {
                let spec = CreateSpec {
                    path: resolve(&root, &s.path, home),
                    kind: s.kind,
                };
                let check = check_one(&root, &spec, home);
                (spec, check)
            })
            .collect();
        Self { root, items }
    }

    pub fn will_create(&self) -> usize {
        self.items
            .iter()
            .filter(|(_, c)| *c == SpecCheck::WillCreate)
            .count()
    }

    pub fn skipped(&self) -> usize {
        self.items
            .iter()
            .filter(|(_, c)| *c == SpecCheck::SkipExists)
            .count()
    }

    pub fn rejected(&self) -> Vec<(PathBuf, String)> {
        self.items
            .iter()
            .filter_map(|(s, c)| match c {
                SpecCheck::Rejected(why) => Some((s.path.clone(), why.clone())),
                _ => None,
            })
            .collect()
    }

    /// 真正落盘。只做 `WillCreate` 的那些，顺序即用户写的顺序。
    ///
    /// `batch` 是一次创建的标识，撤销按它找回这一批。
    pub fn execute(&self, store: &AuditStore, batch: &str) -> CreateReport {
        let now = crate::now_iso8601();
        let mut report = CreateReport {
            batch: batch.to_string(),
            skipped: self.skipped(),
            rejected: self.rejected(),
            ..Default::default()
        };

        for (spec, check) in &self.items {
            if *check != SpecCheck::WillCreate {
                continue;
            }
            let made = match spec.kind {
                CreateKind::Dir => std::fs::create_dir_all(&spec.path),
                CreateKind::File => match spec.path.parent() {
                    Some(parent) => std::fs::create_dir_all(parent)
                        .and_then(|_| std::fs::write(&spec.path, b"")),
                    None => Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "路径没有上级目录",
                    )),
                },
            };
            if let Err(e) = made {
                report
                    .failed
                    .push((spec.path.clone(), format!("创建失败：{e}")));
                continue;
            }

            // 记下创建瞬间的 mtime：撤销时靠它区分「我建的那个」与「后来被改过的那个」。
            let mtime = mtime_nanos(&spec.path).unwrap_or(0);
            match store.record_create(&spec.path, spec.kind, batch, &now, mtime) {
                Ok(_) => report.created.push(spec.path.clone()),
                Err(e) => {
                    // 记录写不进去就把刚建的东西删掉——没有记录的创建不该存在，
                    // 否则用户之后无法撤销，而界面上还显示着「可撤销」。
                    let _ = remove_one(&spec.path);
                    report
                        .failed
                        .push((spec.path.clone(), format!("写审计失败，已回滚：{e}")));
                }
            }
        }
        report
    }
}

/// 一次创建的结果。
#[derive(Debug, Clone, Default)]
pub struct CreateReport {
    pub created: Vec<PathBuf>,
    pub skipped: usize,
    pub rejected: Vec<(PathBuf, String)>,
    pub failed: Vec<(PathBuf, String)>,
    pub batch: String,
}

impl CreateReport {
    pub fn summary(&self) -> String {
        let mut s = format!("已创建 {} 项", self.created.len());
        if self.skipped > 0 {
            s.push_str(&format!("，跳过 {} 项（已存在）", self.skipped));
        }
        if !self.rejected.is_empty() {
            s.push_str(&format!("，拒绝 {} 项", self.rejected.len()));
        }
        if !self.failed.is_empty() {
            s.push_str(&format!("，失败 {} 项", self.failed.len()));
        }
        s
    }

    pub fn has_problems(&self) -> bool {
        !self.rejected.is_empty() || !self.failed.is_empty()
    }
}

/// 撤销一次创建的结果。
#[derive(Debug, Clone, Default)]
pub struct UndoReport {
    pub removed: usize,
    /// 没有删、以及为什么（用户看得懂的那句话）
    pub skipped: Vec<(PathBuf, String)>,
}

impl UndoReport {
    pub fn summary(&self) -> String {
        let mut s = format!("已撤销 {} 项", self.removed);
        if !self.skipped.is_empty() {
            s.push_str(&format!("，保留 {} 项", self.skipped.len()));
        }
        s
    }
}

/// 撤销一批创建。**逆序**逐条处理：后建的先删，父目录才不会先被清空。
///
/// 三条校验任一不满足就跳过：目录必须仍为空、文件必须仍是 0 字节、
/// mtime 与记录一致。这三条合起来的意思是「它自建出来以后没人动过」。
pub fn undo_batch(batch: &str, store: &AuditStore, home: &str) -> Result<UndoReport, String> {
    let mut recs = store
        .pending_create_batch(batch)
        .map_err(|e| format!("读创建记录失败：{e}"))?;
    recs.sort_by_key(|r| std::cmp::Reverse(r.id));

    let mut report = UndoReport::default();
    for rec in recs {
        let kind = CreateKind::parse(&rec.kind);
        if let Some(why) = why_not_undoable(&rec.path, kind, rec.mtime) {
            report.skipped.push((rec.path.clone(), why));
            continue;
        }
        // 再走一次硬禁区：创建之后规则库可能变了，撤销不该绕开它。
        if let Some(hit) = molt_safety::deny::check(&rec.path.to_string_lossy(), home) {
            report
                .skipped
                .push((rec.path.clone(), format!("现在是受保护路径（{}）", hit.why)));
            continue;
        }
        match remove_one(&rec.path) {
            Ok(()) => {
                let _ = store.mark_create_undone(rec.id);
                report.removed += 1;
            }
            Err(e) => report
                .skipped
                .push((rec.path.clone(), format!("删除失败：{e}"))),
        }
    }
    Ok(report)
}

/// 一条记录还能不能删。返回 `Some(原因)` 表示不能。
fn why_not_undoable(path: &Path, kind: CreateKind, recorded_mtime: i64) -> Option<String> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        // 已经不在了：不是失败，用户可能自己删了。但要留一条说明。
        Err(_) => return Some("已经不在了".to_string()),
    };
    if meta.file_type().is_symlink() {
        return Some("它变成了符号链接，不删".to_string());
    }
    match kind {
        CreateKind::Dir => {
            if !meta.is_dir() {
                return Some("它已经不是一个文件夹了，不删".to_string());
            }
            match std::fs::read_dir(path) {
                Ok(mut rd) => {
                    if rd.next().is_some() {
                        return Some("这个文件夹里现在有东西了，不删".to_string());
                    }
                }
                Err(e) => return Some(format!("读不到这个文件夹：{e}")),
            }
        }
        CreateKind::File => {
            if !meta.is_file() {
                return Some("它已经不是一个文件了，不删".to_string());
            }
            if meta.len() != 0 {
                return Some("这个文件里已经有内容了，不删".to_string());
            }
        }
    }
    let now = mtime_nanos(path).unwrap_or(0);
    if now != recorded_mtime {
        return Some("它建好之后被改动过，不删".to_string());
    }
    None
}

fn remove_one(path: &Path) -> std::io::Result<()> {
    let meta = std::fs::symlink_metadata(path)?;
    if meta.is_dir() {
        std::fs::remove_dir(path)
    } else {
        std::fs::remove_file(path)
    }
}

fn mtime_nanos(path: &Path) -> Option<i64> {
    let t = std::fs::symlink_metadata(path).ok()?.modified().ok()?;
    let d = t.duration_since(std::time::UNIX_EPOCH).ok()?;
    Some(d.as_nanos() as i64)
}

/// 把 `~` 展开成家目录，并把相对路径接到根目录后面。
///
/// 相对路径是允许的：用户在「建在哪里」已经选了一个根目录，
/// 再逼他每行都写绝对路径就是多余的一步。
fn expand(p: &Path, home: &str) -> PathBuf {
    let s = p.to_string_lossy();
    let expanded = if s == "~" {
        PathBuf::from(home)
    } else if let Some(rest) = s.strip_prefix("~/") {
        Path::new(home).join(rest)
    } else {
        p.to_path_buf()
    };
    normalize(&expanded)
}

/// 一条待建路径的最终落点：`~` 展开、相对路径接到根目录下、消掉 `.`。
///
/// `..` **原样保留**：它交给闸 2 去拒绝，而不是在这里悄悄解释掉。
/// 解释掉就再也分不清「用户写的是根目录内的一条路径」和「用户试图跳出去」。
fn resolve(root: &Path, p: &Path, home: &str) -> PathBuf {
    let expanded = expand(p, home);
    if expanded.is_absolute() {
        expanded
    } else {
        normalize(&root.join(expanded))
    }
}

/// 消掉 `.` 与重复斜杠，**不解析 `..`**——`..` 会被闸 2 直接拒绝。
///
/// 不解析它是有意的：`root/a/../../etc/x` 规范化之后落在哪里，取决于
/// 符号链接与真实目录结构，靠字符串猜真实位置正是路径逃逸的来源。
fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// 七道闸，顺序即优先级。返回第一个不通过的。
fn check_one(root: &Path, spec: &CreateSpec, home: &str) -> SpecCheck {
    let path = &spec.path;

    // 闸 2：路径段里不得有 `..`。放在最前面——后面所有「落没落在根目录内」
    // 的判断都建立在路径是字面可读的前提上。
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return SpecCheck::Rejected("路径里不能有 `..`".to_string());
    }

    // 闸 1：必须落在用户选定的根目录之内。
    if !path.is_absolute() {
        return SpecCheck::Rejected("请写绝对路径，或选一个根目录后写相对路径".to_string());
    }
    if path == root {
        return SpecCheck::Rejected("这就是根目录本身".to_string());
    }
    if !path.starts_with(root) {
        return SpecCheck::Rejected("超出你选定的目录".to_string());
    }

    // 闸 3：硬禁区。复用安全层那一份表，不另写。
    if let Some(hit) = molt_safety::deny::check(&path.to_string_lossy(), home) {
        return SpecCheck::Rejected(format!("这是受保护路径（{}）", hit.why));
    }

    // 闸 5：父目录逐级都要过同样的检查。少一级就够从根目录跳出去。
    let mut cur = path.parent();
    while let Some(p) = cur {
        if p == root {
            break;
        }
        if !p.starts_with(root) {
            return SpecCheck::Rejected("上级目录超出你选定的目录".to_string());
        }
        if let Some(hit) = molt_safety::deny::check(&p.to_string_lossy(), home) {
            return SpecCheck::Rejected(format!("上级目录受保护（{}）", hit.why));
        }
        cur = p.parent();
    }

    // 闸 4：已存在只跳过，永不覆盖。
    if std::fs::symlink_metadata(path).is_ok() {
        return SpecCheck::SkipExists;
    }

    SpecCheck::WillCreate
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "molt-create-{}-{}-{}",
            std::process::id(),
            tag,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn trailing_slash_means_directory() {
        let specs = parse_lines("src/\ntests/\nREADME.md\n\n# 注释\n");
        assert_eq!(specs.len(), 3);
        assert_eq!(specs[0].kind, CreateKind::Dir);
        assert_eq!(specs[2].kind, CreateKind::File);
        assert_eq!(specs[2].path, PathBuf::from("README.md"));
    }

    #[test]
    fn preflight_creates_what_it_promised() {
        let base = tmp("plan");
        let root = base.join("proj");
        let specs = vec![
            CreateSpec::dir("src"),
            CreateSpec::dir("tests"),
            CreateSpec::file("README.md"),
        ];
        let plan =
            CreatePlan::preflight(&root, &specs, base.to_string_lossy().to_string().as_str());
        // 一行相对路径都能落成绝对路径
        assert_eq!(plan.items[0].0.path, root.join("src"));

        let store = AuditStore::in_memory().unwrap();
        let report = plan.execute(&store, "b1");
        assert_eq!(report.created.len(), 3);
        assert!(root.join("src").is_dir());
        assert!(root.join("README.md").is_file());
        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn an_existing_target_is_skipped_not_overwritten() {
        let base = tmp("exists");
        let root = base.join("proj");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("README.md"), b"keep me").unwrap();

        let specs = vec![CreateSpec::dir("src"), CreateSpec::file("README.md")];
        let plan = CreatePlan::preflight(&root, &specs, base.to_string_lossy().as_ref());
        assert_eq!(plan.will_create(), 0);
        assert_eq!(plan.skipped(), 2);

        let store = AuditStore::in_memory().unwrap();
        plan.execute(&store, "b");
        // 内容必须原封不动
        assert_eq!(fs::read(root.join("README.md")).unwrap(), b"keep me");
        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn parent_traversal_is_rejected() {
        let base = tmp("dotdot");
        let root = base.join("proj");
        let specs = vec![CreateSpec::file("../escape.txt")];
        let plan = CreatePlan::preflight(&root, &specs, base.to_string_lossy().as_ref());
        match &plan.items[0].1 {
            SpecCheck::Rejected(why) => assert!(why.contains("`..`"), "{why}"),
            other => panic!("应被拒绝，实际 {other:?}"),
        }
        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn paths_outside_the_root_are_rejected() {
        let base = tmp("outside");
        let root = base.join("proj");
        let specs = vec![CreateSpec::dir("/tmp/elsewhere")];
        let plan = CreatePlan::preflight(&root, &specs, base.to_string_lossy().as_ref());
        assert!(matches!(plan.items[0].1, SpecCheck::Rejected(_)));
        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn hard_denied_paths_are_rejected() {
        let base = tmp("deny");
        let home = base.to_string_lossy().to_string();
        // 建在受保护目录里：即使它真的落在根目录内（把根设成家目录）也不行
        let specs = vec![CreateSpec::file(".ssh/config")];
        let plan = CreatePlan::preflight(&base, &specs, &home);
        match &plan.items[0].1 {
            SpecCheck::Rejected(why) => assert!(why.contains("受保护"), "{why}"),
            other => panic!("应被拒绝，实际 {other:?}"),
        }
        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn undo_removes_only_untouched_items() {
        let base = tmp("undo");
        let root = base.join("proj");
        let specs = vec![
            CreateSpec::dir("src"),
            CreateSpec::dir("docs"),
            CreateSpec::file("README.md"),
        ];
        let plan = CreatePlan::preflight(&root, &specs, base.to_string_lossy().as_ref());
        let store = AuditStore::in_memory().unwrap();
        plan.execute(&store, "batch-x");

        // 往其中一个目录里放东西：它必须被保留
        fs::write(root.join("docs/note.md"), b"user").unwrap();

        let report = undo_batch("batch-x", &store, base.to_string_lossy().as_ref()).unwrap();
        assert_eq!(report.removed, 2, "空目录与空文件该被删掉");
        assert_eq!(report.skipped.len(), 1);
        assert!(root.join("docs/note.md").exists(), "有内容的不许删");
        assert!(!root.join("src").exists());
        assert!(!root.join("README.md").exists());
        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn undo_keeps_a_file_that_gained_content() {
        let base = tmp("undo-content");
        let root = base.join("proj");
        let specs = vec![CreateSpec::file("a.txt")];
        let plan = CreatePlan::preflight(&root, &specs, base.to_string_lossy().as_ref());
        let store = AuditStore::in_memory().unwrap();
        plan.execute(&store, "b2");

        fs::write(root.join("a.txt"), b"data").unwrap();
        let report = undo_batch("b2", &store, base.to_string_lossy().as_ref()).unwrap();
        assert_eq!(report.removed, 0);
        assert!(root.join("a.txt").exists());
        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn file_with_same_mtime_but_wrong_size_is_kept() {
        // 只校验 mtime 是不够的：写内容与建文件可能落在同一毫秒里。
        let base = tmp("undo-size");
        let root = base.join("proj");
        let specs = vec![CreateSpec::file("b.txt")];
        let plan = CreatePlan::preflight(&root, &specs, base.to_string_lossy().as_ref());
        let store = AuditStore::in_memory().unwrap();
        plan.execute(&store, "b3");

        // 直接写内容，mtime 可能就是同一纳秒级
        fs::write(root.join("b.txt"), b"x").unwrap();
        let report = undo_batch("b3", &store, base.to_string_lossy().as_ref()).unwrap();
        assert_eq!(report.removed, 0, "有内容的文件不能被撤销删掉");
        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn undo_marks_records_so_the_batch_cannot_be_undone_twice() {
        let base = tmp("undo-twice");
        let root = base.join("proj");
        let specs = vec![CreateSpec::dir("src")];
        let plan = CreatePlan::preflight(&root, &specs, base.to_string_lossy().as_ref());
        let store = AuditStore::in_memory().unwrap();
        plan.execute(&store, "b4");

        let first = undo_batch("b4", &store, base.to_string_lossy().as_ref()).unwrap();
        assert_eq!(first.removed, 1);
        let second = undo_batch("b4", &store, base.to_string_lossy().as_ref()).unwrap();
        assert_eq!(second.removed, 0, "同一批不该被撤销两次");
        fs::remove_dir_all(&base).ok();
    }
}
