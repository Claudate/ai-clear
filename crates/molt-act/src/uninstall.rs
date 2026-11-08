//! 卸载一个应用：本体 + 它在 `~/Library` 里留下的东西。
//!
//! 这一层**只做盘点**，不删也不移。真正的落地交给 [`crate::Executor`] 的
//! 隔离模式——这样卸载与清理走同一条写盘路径，带来的直接好处是：
//! 卸错了能在「临时空间」里还原，不需要为卸载再造一套回滚机制。
//!
//! 盘点有三条硬规定：
//!
//! - **残留必须属于这个应用**。按 bundle id 匹配，绝不按「名字看起来像」猜——
//!   `Caches/com.docker.docker` 与 `Caches/com.docker.helper` 是两个应用的东西。
//! - **共享的厂商目录只能进到产品那一层**。`~/Library/Application Support/Google/`
//!   下面同时住着 Chrome 与 GoogleUpdater，所以这一个目录本身不可动，只能收它
//!   下面**名字正好等于这个产品**的子目录。这条是实测出来的：Chrome 有 7.4 GB
//!   数据全在 `Google/Chrome` 里，早先为躲开共享目录把整个 `Google/` 都跳过，
//!   界面上就只剩一个 4 KB 的偏好设置 plist，用户会以为卸干净了。
//! - **受保护的一律不列**。钥匙串、iCloud、配置文件里的密钥由硬禁区兜底，
//!   但这里也要先挡一层：列出来又标成不可选，等于给用户看一堆废行。

use std::path::{Path, PathBuf};

use molt_core::{Candidate, ItemKind, Verdict};

/// 一个已安装的应用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppEntry {
    /// 展示名（取自 bundle 名或 Info.plist 的 CFBundleName）
    pub name: String,
    /// `.app` 本体的路径
    pub path: PathBuf,
    /// 反向域名标识。残留几乎全按它命名，缺了它就只能退化成按名字匹配。
    pub bundle_id: Option<String>,
    pub version: Option<String>,
    /// 本体体积（字节）
    pub bytes: u64,
    /// Info.plist 的 `CFBundleName`。它才是残留目录实际用的名字：
    /// `Visual Studio Code.app` 的 `CFBundleName` 是 `Code`，数据在 `Application Support/Code`。
    /// 少了它，4.4 GB 的数据一条都认不出来。
    pub bundle_name: Option<String>,
    /// `CFBundleExecutable`。有些应用只按可执行文件名放数据。
    pub executable: Option<String>,
    /// `CFBundleIconFile`。界面用它定位 `.icns`，缺了就得去资源目录里猜。
    pub icon_file: Option<String>,
}

impl AppEntry {
    /// 它是不是一个系统自带的应用。系统应用不列：卸掉 Safari 不会释放多少空间，
    /// 但会让机器变得奇怪，而且它们受 SIP 保护、根本移不动。
    pub fn is_system(&self) -> bool {
        let p = self.path.to_string_lossy();
        p.starts_with("/System/") || p.starts_with("/Applications/Utilities/")
    }
}

/// 一处残留。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Leftover {
    pub path: PathBuf,
    /// 「缓存」「偏好设置」这类中文类别，直接显示给用户
    pub category: &'static str,
    pub bytes: u64,
    /// 落在 `/Library` 下、普通用户改不动，要管理员权限。
    ///
    /// 不因此把它们藏起来：藏起来用户就不知道「还有东西没清掉」，
    /// 而他刚按下「卸载干净」。列出来、标清楚「需要管理员」，
    /// 由他自己决定要不要用 sudo 处理——这比假装清干净了诚实。
    pub needs_admin: bool,
    /// 到时间上限就收手了，`bytes` 只是下界。
    ///
    /// 必须和「确实只有这么大」分开：有些厂商容器目录会让 `read_dir` 一直不返回，
    /// 那时如实写「量不完」比给一个偏小的数字诚实。
    pub unmeasured: bool,
}

/// 一次卸载涉及的完整清单：本体 + 残留。
#[derive(Debug, Clone)]
pub struct UninstallPlan {
    pub app: AppEntry,
    pub leftovers: Vec<Leftover>,
    /// 移动本体是否需要管理员权限（`/Applications` 通常属 root）
    pub app_needs_admin: bool,
}

impl UninstallPlan {
    /// 盘点一个应用。纯只读。
    pub fn build(app: AppEntry, home: &str) -> Self {
        let app_needs_admin = !parent_is_writable(&app.path);
        let leftovers = leftovers_for(&app, home);
        Self {
            app,
            leftovers,
            app_needs_admin,
        }
    }

    pub fn leftover_bytes(&self) -> u64 {
        self.leftovers.iter().map(|l| l.bytes).sum()
    }

    /// 全部勾掉时能腾出的量。
    pub fn total_bytes(&self) -> u64 {
        self.app.bytes + self.leftover_bytes()
    }

    /// 勾选的那些变成执行候选。本体在选中集合里时才进来。
    ///
    /// 判定写 `Costly` 而不是 `Reclaimable`：卸载掉的东西**不会自动重建**，
    /// 说成「可回收」就是在骗人。这个档位只影响展示与默认勾选，
    /// 不影响隔离与还原。
    pub fn to_candidates(&self, selected: &dyn Fn(&Path) -> bool) -> Vec<Candidate> {
        let mut out = Vec::new();
        if selected(&self.app.path) {
            out.push(
                Candidate::new(
                    &self.app.path,
                    ItemKind::Dir,
                    Verdict::Costly {
                        rebuild: molt_core::RebuildCost::new(
                            molt_core::CostKind::Reinstall,
                            "需要重新安装",
                        ),
                    },
                )
                .with_size(self.app.bytes, self.app.bytes, 0),
            );
        }
        for l in &self.leftovers {
            if !selected(&l.path) {
                continue;
            }
            let kind = if l.path.is_dir() {
                ItemKind::Dir
            } else {
                ItemKind::File
            };
            out.push(
                Candidate::new(
                    &l.path,
                    kind,
                    Verdict::Costly {
                        rebuild: molt_core::RebuildCost::new(
                            molt_core::CostKind::Reinstall,
                            "应用数据，重装后需要重新登录或配置",
                        ),
                    },
                )
                .with_size(l.bytes, l.bytes, 0),
            );
        }
        out
    }
}

/// 只读一个 `.app` 的 Info.plist，不测体积。
///
/// 拆出来给两处用：`scan_apps` 自己要它，界面出图也只要这几个键而不想付
/// 遍历整个 bundle 的代价（Chrome 有几十万文件）。
pub struct AppMeta {
    pub bundle_id: Option<String>,
    pub version: Option<String>,
    pub bundle_name: Option<String>,
    pub executable: Option<String>,
    pub icon_file: Option<String>,
}

pub fn read_app_meta(app: &Path) -> AppMeta {
    let dict = read_info(&app.join("Contents/Info.plist"));
    let d = dict.as_ref();
    let bundle_id = d
        .and_then(|d| dict_string(d, "CFBundleIdentifier"))
        .filter(|id| id.contains('.') && !id.contains(' '));
    AppMeta {
        bundle_id,
        version: d.and_then(|d| dict_string(d, "CFBundleShortVersionString")),
        bundle_name: d.and_then(|d| dict_string(d, "CFBundleName")),
        executable: d.and_then(|d| dict_string(d, "CFBundleExecutable")),
        icon_file: d.and_then(|d| dict_string(d, "CFBundleIconFile")),
    }
}

/// 扫一遍应用目录，**只读元信息、不量体积**。只认 `.app` 结尾的一层目录。
///
/// 为什么把「量体积」拆出去：量体积要遍历每个 bundle 里的几万个文件，
/// 一屏二十几个应用串行跑下来要好几秒。以前这两件事绑在一起，用户点进卸载页
/// 得盯着空白等全部量完；现在清单立刻出来（名字与图标都在），体积由调用方
/// 在后台并行补上，界面逐条填进去。
pub fn list_apps(dirs: &[PathBuf]) -> Vec<AppEntry> {
    let mut out = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for e in entries.flatten() {
            let path = e.path();
            if path.extension().map(|x| x != "app").unwrap_or(true) {
                continue;
            }
            if !path.is_dir() {
                continue;
            }
            let name = path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            if name.is_empty() {
                continue;
            }
            let meta = read_app_meta(&path);
            out.push(AppEntry {
                name,
                // 体积留给 `measure_app`；0 表示「还没量」，不是「空应用」
                bytes: 0,
                path,
                bundle_id: meta.bundle_id,
                version: meta.version,
                bundle_name: meta.bundle_name,
                executable: meta.executable,
                icon_file: meta.icon_file,
            });
        }
    }

    // 元信息阶段按名字排：体积还没量，按体积排会是随机的。
    // 量完体积之后调用方会再按体积排一次。
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// 量一个应用本体的体积。可以按应用并行调用。
pub fn measure_app(app: &AppEntry) -> u64 {
    measure_path(&app.path)
}

/// 从一个 `.app` 路径重新读出 [`AppEntry`]。
///
/// 界面要按需盘点某个应用的残留，而它手里只有「路径 + bundle id」这些展示字段
/// （清单是后台扫出来、复制进界面的）。残留匹配要用到 `CFBundleName` 与
/// `CFBundleExecutable`，所以这里把 Info.plist 再读一遍——只是一次文件读，
/// 比在界面状态里额外养一份完整 `AppEntry` 划算。
pub fn entry_at(path: &Path) -> Option<AppEntry> {
    if path.extension().map(|x| x != "app").unwrap_or(true) {
        return None;
    }
    let name = path.file_stem()?.to_string_lossy().to_string();
    if name.is_empty() {
        return None;
    }
    let meta = read_app_meta(path);
    Some(AppEntry {
        name,
        path: path.to_path_buf(),
        bundle_id: meta.bundle_id,
        version: meta.version,
        bundle_name: meta.bundle_name,
        executable: meta.executable,
        icon_file: meta.icon_file,
        bytes: 0,
    })
}

/// 量一个目录的体积。并行线程只有路径，所以单独给它一个入口。
///
/// **必须带上限，而且上限要由外部线程来收。** 遍历在个别目录上会卡在
/// `read_dir` 这个系统调用里，既不返回也不报错——内部的计时检查根本执行不到，
/// 于是在那里的那一个应用永远量不完，它的格子停在「正在量…」，
/// 整次列举也跟着永远不结束。所以这里沿用 [`measure_with_budget`] 的做法：
/// 量在别的线程里跑，这边按超时收手，超时就当「量不出来」，给它 0 让界面
/// 继续往下走，而不是把整页拖死。
pub fn measure_path(path: &Path) -> u64 {
    // `.app` 本体可能是符号链接：`/Applications/Safari.app` 就指向
    // `/System/Cryptexes/App/System/Applications/Safari.app`（系统卷里）。
    // 遍历默认不跟随链接，那是为了不把**清理范围**扩到链接目标；但这里问的是
    // 「这个应用占多大」，链接目标就是它的全部内容，不跟随只会量出一个 0，
    // 界面上就成了一个骗人的「0 B」。所以在入口这一步解析一次。
    let real = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    measure_with_budget(&real, APP_MEASURE_BUDGET).0
}

/// 量一个应用的体积最多等多久。
///
/// 20 秒对真实的应用包远远够用（本机最大的 Chrome 约 1 GB，不到 1 秒），
/// 而卡死的那类目录是一秒都读不动——超过这个数就不是「大」而是「读不了」。
const APP_MEASURE_BUDGET: std::time::Duration = std::time::Duration::from_secs(20);

/// 扫一遍应用目录并顺带量好体积。串行版本，供命令行与测试使用。
pub fn scan_apps(dirs: &[PathBuf]) -> Vec<AppEntry> {
    let mut out = list_apps(dirs);
    for a in out.iter_mut() {
        a.bytes = measure_app(a);
    }
    sort_by_bytes(&mut out);
    out
}

/// 按体积降序：用户想卸的通常是那个占了好几 G 的。
pub fn sort_by_bytes(apps: &mut [AppEntry]) {
    apps.sort_by_key(|a| std::cmp::Reverse(a.bytes));
}

/// `~/Library` 里与该应用相关的残留。
///
/// `home` 显式传入，测试才能拿假家目录完整跑一遍。
pub fn leftovers_for(app: &AppEntry, home: &str) -> Vec<Leftover> {
    let home = Path::new(home);
    let lib = home.join("Library");
    let mut out = Vec::new();

    // 应用的数据目录不一定用 `.app` 的文件名。真机上：
    //   `Visual Studio Code.app` → 数据在 `Application Support/Code`（CFBundleName）
    //   `Google Chrome.app`      → 数据在 `Caches/Google/Chrome`（厂商/产品两层）
    // 所以每个候选名字都要试，而「等于」永远是完整相等，不做包含匹配——
    // `Code` 包含在 `Code - Insiders` 里，`Chrome` 包含在 `Chrome for Testing` 里。
    let names: Vec<String> = [
        Some(app.name.clone()),
        app.bundle_name.clone(),
        app.executable.clone(),
    ]
    .into_iter()
    .flatten()
    .filter(|s| !s.is_empty())
    .collect();

    let ids: Vec<String> = app.bundle_id.iter().cloned().collect();

    let mut candidates: Vec<(PathBuf, &'static str)> = Vec::new();

    for id in &ids {
        candidates.push((lib.join("Application Support").join(id), "应用数据"));
        candidates.push((lib.join("Caches").join(id), "缓存"));
        candidates.push((lib.join("Logs").join(id), "日志"));
        candidates.push((lib.join("HTTPStorages").join(id), "网络缓存"));
        candidates.push((lib.join("WebKit").join(id), "网页数据"));
        candidates.push((
            lib.join("Cookies").join(format!("{id}.binarycookies")),
            "登录状态",
        ));
        candidates.push((lib.join("Containers").join(id), "应用容器"));
        candidates.push((lib.join("Group Containers").join(id), "共享容器"));
        candidates.push((lib.join("Application Scripts").join(id), "脚本"));
        candidates.push((
            lib.join("Saved Application State")
                .join(format!("{id}.savedState")),
            "窗口状态",
        ));
        candidates.push((
            lib.join("Preferences").join(format!("{id}.plist")),
            "偏好设置",
        ));
        candidates.push((
            lib.join("LaunchAgents").join(format!("{id}.plist")),
            "开机启动",
        ));
        candidates.push((lib.join("CrashReporter").join(id), "崩溃日志"));
        // 系统级区域：所有用户共享，只有完整等于 bundle id 才收。
        // 普通用户删不动，界面上要标出来，见 `Leftover::needs_admin`。
        candidates.push((
            PathBuf::from("/Library/Application Support").join(id),
            "系统级数据",
        ));
        candidates.push((PathBuf::from("/Library/Caches").join(id), "系统级缓存"));
    }

    // 崩溃报告按「应用名_日期」命名，不是按 bundle id，只能按前缀扫。
    // 只收这个目录下以 `<name>_` 开头的：那一段前缀正是崩溃报告的命名规则，
    // 拿不到更多结构可以判等，所以宁可只认这一种形状。
    for name in &names {
        let dir = lib.join("Logs/DiagnosticReports");
        if let Ok(entries) = std::fs::read_dir(&dir) {
            let prefix = format!("{name}_");
            for e in entries.flatten() {
                let file_name = e.file_name().to_string_lossy().to_string();
                if file_name.starts_with(&prefix) {
                    candidates.push((e.path(), "崩溃报告"));
                }
            }
        }
    }

    // 厂商分层目录：`<库目录>/<厂商>/<产品>`。见 [`vendor_children`] 里的判等规则。
    let vendors: Vec<String> = app
        .bundle_id
        .as_deref()
        .map(bundle_id_segments)
        .unwrap_or_default();
    for name in &names {
        candidates.extend(vendor_children(&lib, &vendors, name));
    }

    // 按名字命名的那些。同一个东西可能已经被 bundle id 那条收进去了，
    // 所以去重放在最后统一做，而不是边加边查——边加边查会让
    // 每加一条都扫一遍前面全部。厂商分层那部分已经在上面每个名字都试过了。
    for name in &names {
        candidates.push((lib.join("Application Support").join(name), "应用数据"));
        candidates.push((lib.join("Caches").join(name), "缓存"));
        candidates.push((lib.join("Logs").join(name), "日志"));
    }

    let mut seen: Vec<PathBuf> = Vec::new();
    for (path, category) in candidates {
        if seen.contains(&path) || !path.exists() {
            continue;
        }
        if molt_safety::deny::check(&path.to_string_lossy(), &home.to_string_lossy()).is_some() {
            continue;
        }
        seen.push(path.clone());
        let m = measure_with_budget(&path, LEFTOVER_BUDGET);
        out.push(Leftover {
            bytes: m.0,
            unmeasured: m.1,
            needs_admin: needs_admin(&path),
            path,
            category,
        });
    }

    out.sort_by_key(|l| std::cmp::Reverse(l.bytes));
    out
}

/// bundle id 的全部段，转小写：`com.google.Chrome` → `[com, google, chrome]`。
///
/// 厂商目录名几乎总是其中的一段（`Google`、`Microsoft`、`obsidian`），
/// 但**哪一段没有定式**：`com.google.Chrome` 是第二段，`md.obsidian` 是最后一段，
/// `io.github.x.y` 可能一个都不是。所以不猜，全部拿出来逐个试。
/// 猜错的代价是把别人的数据列成「可卸载」，比多扫几个不存在的目录严重得多。
fn bundle_id_segments(bundle_id: &str) -> Vec<String> {
    bundle_id
        .split('.')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_ascii_lowercase())
        .collect()
}

/// 在各库目录下找 `<厂商>/<产品>` 形式的目录。
///
/// 两道判等同时成立才算命中：父目录名等于厂商、末段名等于应用名。
/// 只有一道不够——只判产品名会把 `Caches/chrome_crashpad_handler` 之外所有叫
/// `Chrome` 的东西都算进来，只判厂商会把同厂的其他应用也一起收走。
/// 所以厂商目录本身**永远不单独进候选**，只定位到产品那一层为止。
fn vendor_children(lib: &Path, vendors: &[String], name: &str) -> Vec<(PathBuf, &'static str)> {
    let mut out = Vec::new();
    if vendors.is_empty() || name.is_empty() {
        return out;
    }
    for (dir, category) in [
        ("Application Support", "应用数据"),
        ("Caches", "缓存"),
        ("Logs", "日志"),
        ("HTTPStorages", "网络缓存"),
    ] {
        let Some(parents) = std::fs::read_dir(lib.join(dir)).ok() else {
            continue;
        };
        for p in parents.flatten() {
            // 大小写不敏感地判等：真机上 `Google` 是大写，bundle id 里是小写。
            let parent = p.file_name().to_string_lossy().to_ascii_lowercase();
            if !vendors.iter().any(|v| v == &parent) {
                continue;
            }
            let child = p.path().join(name);
            if child.exists() {
                out.push((child, category));
            }
        }
    }
    out
}

/// 这台机器上该扫哪些应用目录。
pub fn app_dirs(home: &str) -> Vec<PathBuf> {
    vec![
        PathBuf::from("/Applications"),
        Path::new(home).join("Applications"),
    ]
}

/// 从 Info.plist 里取一串键值。
///
/// 用 `plist` crate 而不是手写找 `<key>`：`/Applications` 里大量应用的 Info.plist
/// 是**二进制**格式，手写解析只能认 XML，二进制那批就得退回 `defaults` 子进程。
/// 顺带一次读出四个键，省掉每个应用多次打开同一文件。
fn read_info(info: &Path) -> Option<plist::Dictionary> {
    let value = plist::Value::from_file(info).ok()?;
    match value {
        plist::Value::Dictionary(d) => Some(d),
        _ => None,
    }
}

/// 从字典里取字符串，空串当没有。
fn dict_string(d: &plist::Dictionary, key: &str) -> Option<String> {
    let v = d.get(key)?.as_string()?.trim();
    if v.is_empty() {
        None
    } else {
        Some(v.to_string())
    }
}

/// 这条路径是不是只有管理员（或属主是别人）才能动。
///
/// 判的是「父目录不在家目录里」：`/Library/...` 一定在别人的权限范围里，
/// 而 `~/Library/...` 不管内容属主是谁，至少能试着移走。真的移不动时
/// 执行阶段会逐项报错，这里只用来给用户一个提前的提示。
fn needs_admin(path: &Path) -> bool {
    let s = path.to_string_lossy();
    s.starts_with("/Library/")
        || s.starts_with("/Applications/")
        || s.starts_with("/private/")
        || s.starts_with("/usr/")
}

/// 量残留时，一条候选目录最多花多久。
///
/// 5 秒对真实目录远远够用（本机最大的 Chrome 数据 5 GB 约 1 秒），
/// 而挂住的那类容器目录会一秒都读不动——超过这个数就不是「大」，
/// 是「读不了」，再等下去只会让用户对着「正在找残留…」空等。
/// 实测：本机 Clash Verge 的共享容器正是这种目录，30 秒预算时整次盘点要 32 秒，
/// 5 秒预算时只剩那一条等待，其余五条照常量出。
const LEFTOVER_BUDGET: std::time::Duration = std::time::Duration::from_secs(5);

/// 限时量一个目录，返回 `(字节数, 是否没量完)`。
///
/// **为什么要在别的线程里量。** 挂住的那种目录卡在 `read_dir` 这个系统调用里，
/// 它既不返回也不报错——遍历内部再多的计时检查都执行不到，因为根本回不到循环。
/// 唯一能挣脱的办法就是不等它：这边按超时收手，那边那个线程让它留着，
/// 等内核哪天放它出来自己结束。代价是一个卡死的线程，换来的是界面不会跟着卡死。
fn measure_with_budget(path: &Path, budget: std::time::Duration) -> (u64, bool) {
    let (tx, rx) = std::sync::mpsc::channel::<(u64, bool)>();
    let p = path.to_path_buf();
    std::thread::spawn(move || {
        let opts = molt_scan::WalkOptions {
            budget: Some(budget),
            ..Default::default()
        };
        let r = molt_scan::measure(&p, &opts);
        // 接收端已经按超时走了也不打紧，发送失败就丢掉
        let _ = tx.send((r.measured.alloc_bytes, r.truncated));
    });
    // 连超时都没等到，说明这条目录读不了：字节数只知道「不止于此」
    rx.recv_timeout(budget + std::time::Duration::from_secs(2))
        .unwrap_or((0, true))
}

/// 父目录能不能直接写。`/Applications` 属 root，普通用户移不动里面的东西。
///
/// 用 owner 写位判断而不是真的试写：试写会在用户的应用程序目录里留下垃圾。
fn parent_is_writable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let Some(parent) = path.parent() else {
        return false;
    };
    let Ok(meta) = std::fs::metadata(parent) else {
        return false;
    };
    meta.permissions().mode() & 0o200 != 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "molt-uninst-{}-{}-{}",
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

    fn make_app(base: &Path, name: &str, bundle: &str) -> PathBuf {
        let app = base.join(format!("{name}.app"));
        fs::create_dir_all(app.join("Contents")).unwrap();
        fs::write(
            app.join("Contents/Info.plist"),
            format!(
                "<?xml version=\"1.0\"?><plist><dict>\
                 <key>CFBundleIdentifier</key><string>{bundle}</string>\
                 <key>CFBundleShortVersionString</key><string>1.2.3</string>\
                 </dict></plist>"
            ),
        )
        .unwrap();
        fs::write(app.join("Contents/binary"), vec![0u8; 4096]).unwrap();
        app
    }

    /// `.app` 本体是符号链接时，体积必须来自链接目标。
    ///
    /// `/Applications/Safari.app` 就是这种：它指向系统卷里的真实 bundle。
    /// 遍历默认不跟随链接（免得把清理范围拓到链接目标），但「这个应用占多大」
    /// 问的就是目标本身——不解析就量出 0，界面上写着「0 B」，是个骗人的数字。
    #[test]
    fn a_symlinked_app_reports_the_size_of_its_target() {
        let base = tmp("symlink");
        let real = make_app(&base.join("real"), "Linked", "com.example.linked");
        fs::write(real.join("Contents/payload"), vec![1u8; 300 * 1024]).unwrap();
        let apps_dir = base.join("Applications");
        fs::create_dir_all(&apps_dir).unwrap();
        std::os::unix::fs::symlink(&real, apps_dir.join("Linked.app")).unwrap();

        let apps = scan_apps(std::slice::from_ref(&apps_dir));
        let app = apps
            .iter()
            .find(|a| a.name == "Linked")
            .expect("要列出链接");
        assert!(
            app.bytes >= 300 * 1024,
            "链接指向的 bundle 有 300 KiB，量到 {} —— 为 0 说明没跟随到目标",
            app.bytes
        );
        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn scan_reads_name_bundle_id_and_version() {
        let base = tmp("scan");
        make_app(&base, "Demo", "com.example.demo");
        let apps = scan_apps(std::slice::from_ref(&base));
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].name, "Demo");
        assert_eq!(apps[0].bundle_id.as_deref(), Some("com.example.demo"));
        assert_eq!(apps[0].version.as_deref(), Some("1.2.3"));
        assert!(apps[0].bytes > 0);
        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn leftovers_are_matched_by_bundle_id_only() {
        let base = tmp("leftover");
        let home = base.join("home");
        let apps = base.join("Applications");
        fs::create_dir_all(home.join("Library/Caches/com.example.demo")).unwrap();
        fs::create_dir_all(home.join("Library/Caches/com.example.demo.helper")).unwrap();
        fs::create_dir_all(home.join("Library/Application Support/Demo")).unwrap();
        fs::create_dir_all(home.join("Library/Preferences")).unwrap();
        fs::write(
            home.join("Library/Preferences/com.example.demo.plist"),
            b"x",
        )
        .unwrap();
        let app = make_app(&apps, "Demo", "com.example.demo");
        let entry = scan_apps(&[apps])
            .into_iter()
            .find(|a| a.path == app)
            .unwrap();

        let left = leftovers_for(&entry, &home.to_string_lossy());
        let paths: Vec<String> = left.iter().map(|l| l.path.display().to_string()).collect();

        assert!(paths.iter().any(|p| p.ends_with("Caches/com.example.demo")));
        assert!(
            !paths
                .iter()
                .any(|p| p.ends_with("Caches/com.example.demo.helper")),
            "另一个应用的缓存不能算进来：{paths:?}"
        );
        assert!(paths
            .iter()
            .any(|p| p.ends_with("Application Support/Demo")));
        assert!(paths
            .iter()
            .any(|p| p.ends_with("Preferences/com.example.demo.plist")));
        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn app_without_leftovers_reports_only_itself() {
        let base = tmp("clean-app");
        let home = base.join("home");
        fs::create_dir_all(&home).unwrap();
        let apps = base.join("Applications");
        make_app(&apps, "Solo", "com.example.solo");
        let entry = scan_apps(&[apps]).into_iter().next().unwrap();

        let plan = UninstallPlan::build(entry, &home.to_string_lossy());
        assert!(plan.leftovers.is_empty());
        assert_eq!(plan.leftover_bytes(), 0);
        assert!(plan.app.bytes > 0);
        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn deny_listed_leftovers_are_never_offered() {
        // 假如某个应用的数据真的落在受保护目录里，它不该出现在清单上。
        let base = tmp("deny-leftover");
        let home = base.join("home");
        let apps = base.join("Applications");
        fs::create_dir_all(home.join("Library/Application Support/.secrets")).unwrap();
        make_app(&apps, "Weird", "com.example.weird");
        let entry = scan_apps(&[apps]).into_iter().next().unwrap();

        let left = leftovers_for(&entry, &home.to_string_lossy());
        assert!(
            left.iter()
                .all(|l| !l.path.to_string_lossy().contains(".secrets")),
            "受保护路径不该被列出来"
        );
        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn vendor_scoped_leftovers_are_found_by_product_not_by_vendor() {
        // 真机上 Chrome 的数据全在 `Caches/Google/Chrome`，按 bundle id 直接查
        // 什么都查不到。这条守住「进到产品那一层」，同时守住「厂商目录不动」。
        let base = tmp("vendor");
        let home = base.join("home");
        let apps = base.join("Applications");
        let lib = home.join("Library");
        fs::create_dir_all(lib.join("Caches/Google/Chrome")).unwrap();
        fs::create_dir_all(lib.join("Caches/Google/Chrome for Testing")).unwrap();
        fs::create_dir_all(lib.join("Application Support/Google/GoogleUpdater")).unwrap();
        make_app(&apps, "Google Chrome", "com.google.Chrome");
        // 真机上 Chrome 的 Info.plist 里 CFBundleName 是 `Chrome`（不带 Google），
        // 数据目录用的正是它。夹具必须照实写，否则这条测的不是真机情形。
        fs::write(
            apps.join("Google Chrome.app/Contents/Info.plist"),
            "<?xml version=\"1.0\"?><plist><dict>\
             <key>CFBundleIdentifier</key><string>com.google.Chrome</string>\
             <key>CFBundleName</key><string>Chrome</string>\
             <key>CFBundleExecutable</key><string>Google Chrome</string>\
             </dict></plist>",
        )
        .unwrap();
        let entry = scan_apps(&[apps]).into_iter().next().unwrap();

        let left = leftovers_for(&entry, &home.to_string_lossy());
        let paths: Vec<String> = left.iter().map(|l| l.path.display().to_string()).collect();

        assert!(
            paths.iter().any(|p| p.ends_with("Caches/Google/Chrome")),
            "厂商/产品两层的缓存必须认出来：{paths:?}"
        );
        // 完整相等：`Chrome for Testing` 是另一个产品
        assert!(
            !paths.iter().any(|p| p.ends_with("Chrome for Testing")),
            "名字相近的另一个产品不能算进来：{paths:?}"
        );
        // 厂商目录本身不进清单，否则同厂其他应用会被一起删
        assert!(
            !paths.iter().any(|p| p.ends_with("Google")),
            "厂商目录本身不能作为一项：{paths:?}"
        );
        assert!(
            !paths.iter().any(|p| p.contains("GoogleUpdater")),
            "同厂的其他组件不能算进来：{paths:?}"
        );
        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn data_dir_uses_bundle_name_when_it_differs_from_app_name() {
        // `Visual Studio Code.app` 的数据在 `Application Support/Code`。
        // 只按 `.app` 文件名找会漏掉这份 4 GB 的数据。
        let base = tmp("bundle-name");
        let home = base.join("home");
        let apps = base.join("Applications");
        fs::create_dir_all(home.join("Library/Application Support/Code")).unwrap();
        make_app(&apps, "Visual Studio Code", "com.microsoft.VSCode");
        // make_app 的 Info.plist 没写 CFBundleName，这里补上——真机就是这样
        fs::write(
            apps.join("Visual Studio Code.app/Contents/Info.plist"),
            "<?xml version=\"1.0\"?><plist><dict>\
             <key>CFBundleIdentifier</key><string>com.microsoft.VSCode</string>\
             <key>CFBundleName</key><string>Code</string>\
             <key>CFBundleExecutable</key><string>Code</string>\
             </dict></plist>",
        )
        .unwrap();
        let entry = scan_apps(&[apps]).into_iter().next().unwrap();
        assert_eq!(entry.bundle_name.as_deref(), Some("Code"));

        let left = leftovers_for(&entry, &home.to_string_lossy());
        assert!(
            left.iter()
                .any(|l| l.path.ends_with("Application Support/Code")),
            "CFBundleName 指向的数据目录必须认出来"
        );
        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn bundle_id_segments_keep_every_part() {
        // 每一段都要留：厂商目录用哪一段没有定式。
        assert_eq!(
            bundle_id_segments("com.google.Chrome"),
            vec!["com", "google", "chrome"]
        );
        assert_eq!(bundle_id_segments("md.obsidian"), vec!["md", "obsidian"]);
        assert!(bundle_id_segments("").is_empty());
    }

    #[test]
    fn candidates_only_include_what_was_selected() {
        let base = tmp("cand");
        let home = base.join("home");
        let apps = base.join("Applications");
        fs::create_dir_all(home.join("Library/Caches/com.example.demo")).unwrap();
        make_app(&apps, "Demo", "com.example.demo");
        let entry = scan_apps(&[apps]).into_iter().next().unwrap();
        let plan = UninstallPlan::build(entry, &home.to_string_lossy());

        let only_cache = |p: &Path| p.to_string_lossy().contains("Caches");
        let picked = plan.to_candidates(&only_cache);
        assert_eq!(picked.len(), 1);
        assert!(picked[0].path.to_string_lossy().contains("Caches"));

        // 全不选就是空——不能退化成「默认全选」
        assert!(plan.to_candidates(&|_| false).is_empty());
        fs::remove_dir_all(&base).ok();
    }
}
