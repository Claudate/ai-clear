//! 审计日志：只增不改的操作记录。
//!
//! 为什么只增不改：用户要能回答「上周到底清了什么」。
//! 如果 `restore` 是改原行，那条记录就只剩「曾经被清过，后来还原了」，
//! 中间的时间线全丢——而这正是出错时唯一能用来追的东西。

use rusqlite::{params, Connection, OptionalExtension};
use std::path::{Path, PathBuf};

/// 一次操作。`Quarantine` 与 `Restore` / `Purge` 是同一条链上的不同事件，
/// 各自一行，靠 `absolute_path` 串起来。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionKind {
    /// 从原位置移到隔离区
    Quarantine,
    /// 从隔离区还原回原位置
    Restore,
    /// 隔离期满，真正删除
    Purge,
}

impl ActionKind {
    /// 目前只在调试与未来聚合统计时用得上；保留是为了避免各处再拼字符串。
    #[allow(dead_code)]
    fn as_str(&self) -> &'static str {
        match self {
            ActionKind::Quarantine => "quarantine",
            ActionKind::Restore => "restore",
            ActionKind::Purge => "purge",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OpKind {
    /// 原样移走
    #[default]
    Quarantine,
    /// 压缩成一个包再移走
    Compress,
}

impl OpKind {
    pub fn as_str(self) -> &'static str {
        match self {
            OpKind::Quarantine => "quarantine",
            OpKind::Compress => "compress",
        }
    }

    /// 未知值一律当作隔离：老库里没有这一列，它的语义就是隔离。
    fn from_str(s: &str) -> Self {
        match s {
            "compress" => OpKind::Compress,
            _ => OpKind::Quarantine,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionRecord {
    pub id: i64,
    pub absolute_path: PathBuf,
    pub quarantine_path: PathBuf,
    pub bytes: u64,
    pub sha256: Option<String>,
    pub action: ActionKind,
    /// 内容是怎么进隔离区的：原样移走，还是压缩后移走。
    /// 撤销时必须据此选还原方式——把包当成目录去 rename，只会把包放在原位置。
    pub op: OpKind,
    pub performed_at: String,
    pub purge_after: String,
    pub restored: bool,
    /// 这次操作是谁发起的。`auto` 意味着没有人在现场看过清单——
    /// 用户事后翻历史时必须能认出这一类。
    pub source: CleanSource,
}

/// 一次清理的发起方。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CleanSource {
    /// 有人在终端里看了清单、敲了 yes
    #[default]
    Manual,
    /// 定时任务或 `molt auto`，无人确认
    Auto,
}

impl CleanSource {
    pub fn as_str(self) -> &'static str {
        match self {
            CleanSource::Manual => "manual",
            CleanSource::Auto => "auto",
        }
    }

    fn from_str(s: &str) -> Self {
        match s {
            "auto" => CleanSource::Auto,
            _ => CleanSource::Manual,
        }
    }
}

/// 一条待写入的清理记录。把参数收成结构体，是因为位置参数一旦超过五六个，
/// 调用处就开始靠猜——而这几个字段里搞错一个就是审计追溯出错。
pub struct NewQuarantine<'a> {
    pub absolute: &'a Path,
    pub quarantine: &'a Path,
    pub bytes: u64,
    pub sha256: &'a str,
    pub performed_at: &'a str,
    pub purge_after: &'a str,
    pub source: CleanSource,
}

/// 审计库。持有一个 SQLite 连接，schema 在构造时确保存在。
pub struct AuditStore {
    conn: Connection,
}

impl AuditStore {
    /// 打开（或创建）审计库。`path` 通常是 `~/Library/Application Support/app.molt/audit.sqlite`。
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = Connection::open(path)?;
        Self::init(&conn)?;
        Ok(Self { conn })
    }

    /// 内存库，用于测试。不落盘，跑完即销毁。
    pub fn in_memory() -> rusqlite::Result<Self> {
        let conn = Connection::open_in_memory()?;
        Self::init(&conn)?;
        Ok(Self { conn })
    }

    fn init(conn: &Connection) -> rusqlite::Result<()> {
        conn.execute_batch(
            r#"
            PRAGMA journal_mode = WAL;
            PRAGMA foreign_keys = ON;

            CREATE TABLE IF NOT EXISTS action_log (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                absolute_path   TEXT NOT NULL,
                quarantine_path TEXT NOT NULL,
                bytes           INTEGER NOT NULL,
                sha256          TEXT,
                action          TEXT NOT NULL CHECK (action IN ('quarantine','restore','purge')),
                performed_at    TEXT NOT NULL,
                purge_after     TEXT NOT NULL,
                restored        INTEGER NOT NULL DEFAULT 0,
                source          TEXT NOT NULL DEFAULT 'manual'
            );
            CREATE INDEX IF NOT EXISTS idx_action_path ON action_log(absolute_path);
            CREATE INDEX IF NOT EXISTS idx_action_pending
                ON action_log(performed_at) WHERE restored = 0;
            "#,
        )?;

        // 走过本轮之前的库没有 source 列。CREATE TABLE IF NOT EXISTS 不会给已存在的表补列，
        // 所以旧库必须显式升级——否则老用户升级后第一条时间线就写不进去。
        let has_source = {
            let mut stmt = conn.prepare("PRAGMA table_info(action_log)")?;
            let mut rows = stmt.query_map([], |r| r.get::<_, String>(1))?;
            rows.any(|c| matches!(c, Ok(name) if name == "source"))
        };
        if !has_source {
            conn.execute(
                "ALTER TABLE action_log ADD COLUMN source TEXT NOT NULL DEFAULT 'manual'",
                [],
            )?;
        }

        // `op` 同理。用「补列」而不是改 CHECK 约束：
        // 改约束要重建整张表，而这张表存着用户唯一的一份还原凭据。
        let has_op = {
            let mut stmt = conn.prepare("PRAGMA table_info(action_log)")?;
            let mut rows = stmt.query_map([], |r| r.get::<_, String>(1))?;
            rows.any(|c| matches!(c, Ok(name) if name == "op"))
        };
        if !has_op {
            conn.execute(
                "ALTER TABLE action_log ADD COLUMN op TEXT NOT NULL DEFAULT 'quarantine'",
                [],
            )?;
        }
        Ok(())
    }

    /// 记一次「移入隔离区」。返回新记录的 id，它是后续 restore 的凭据。
    pub fn record_quarantine(&self, rec: &NewQuarantine<'_>) -> rusqlite::Result<i64> {
        self.conn.execute(
            "INSERT INTO action_log
             (absolute_path, quarantine_path, bytes, sha256, action, performed_at, purge_after, source)
             VALUES (?1, ?2, ?3, ?4, 'quarantine', ?5, ?6, ?7)",
            params![
                rec.absolute.to_string_lossy(),
                rec.quarantine.to_string_lossy(),
                rec.bytes as i64,
                rec.sha256,
                rec.performed_at,
                rec.purge_after,
                rec.source.as_str()
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// 记一次「压缩归档」。
    ///
    /// `action` 仍然是 `quarantine`：它同样是「待还原」的一条，撤销时按 `op` 选还原方式。
    /// 这样既不用重建老库的表结构，又能让时间线区分「移走」与「压缩」。
    pub fn record_archive(&self, rec: &NewQuarantine<'_>) -> rusqlite::Result<i64> {
        self.conn.execute(
            "INSERT INTO action_log
             (absolute_path, quarantine_path, bytes, sha256, action, performed_at, purge_after, source, op)
             VALUES (?1, ?2, ?3, ?4, 'quarantine', ?5, ?6, ?7, 'compress')",
            params![
                rec.absolute.to_string_lossy(),
                rec.quarantine.to_string_lossy(),
                rec.bytes as i64,
                rec.sha256,
                rec.performed_at,
                rec.purge_after,
                rec.source.as_str()
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// 记一次还原，并把原记录标记为已还原。
    ///
    /// 这里同时做两件事：新增一行 restore 事件（保留时间线），
    /// 把 quarantine 那行的 `restored` 置 1（让它不再出现在「待清理」里）。
    pub fn record_restore(&self, id: i64, performed_at: &str) -> rusqlite::Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        let row: Option<(String, String, i64, Option<String>, String, String)> = tx
            .query_row(
                "SELECT absolute_path, quarantine_path, bytes, sha256, purge_after, op
                 FROM action_log WHERE id = ?1 AND action = 'quarantine'",
                params![id],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                    ))
                },
            )
            .optional()?;

        let Some((absolute, quarantine, bytes, sha256, purge_after, op)) = row else {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        };

        tx.execute(
            "INSERT INTO action_log
             (absolute_path, quarantine_path, bytes, sha256, action, performed_at, purge_after, restored, source, op)
             VALUES (?1, ?2, ?3, ?4, 'restore', ?5, ?6, 1, 'manual', ?7)",
            params![absolute, quarantine, bytes, sha256, performed_at, purge_after, op],
        )?;
        tx.execute(
            "UPDATE action_log SET restored = 1 WHERE id = ?1",
            params![id],
        )?;
        tx.commit()
    }

    /// 找到一条仍可还原的记录。
    pub fn find_pending(&self, id: i64) -> rusqlite::Result<Option<ActionRecord>> {
        self.conn
            .query_row(
                "SELECT id, absolute_path, quarantine_path, bytes, sha256, action,
                        performed_at, purge_after, restored, source, op
                 FROM action_log WHERE id = ?1 AND action = 'quarantine'",
                params![id],
                map_row,
            )
            .optional()
    }

    /// 列出仍可还原的记录，最近的在前面。
    pub fn list_pending(&self, limit: usize) -> rusqlite::Result<Vec<ActionRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, absolute_path, quarantine_path, bytes, sha256, action,
                    performed_at, purge_after, restored, source, op
             FROM action_log
             WHERE action = 'quarantine' AND restored = 0
             ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], map_row)?;
        rows.collect()
    }

    /// 全部历史（含 restore 事件），用于 `molt history`。
    pub fn list_all(&self, limit: usize) -> rusqlite::Result<Vec<ActionRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, absolute_path, quarantine_path, bytes, sha256, action,
                    performed_at, purge_after, restored, source, op
             FROM action_log ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], map_row)?;
        rows.collect()
    }

    /// 到期该清理的隔离记录。用「当前时刻早于 purge_after」判断，
    /// 所以时间一律按 ISO8601 字符串比较——同格式下字典序就是时间序。
    pub fn due_for_purge(&self, now: &str, limit: usize) -> rusqlite::Result<Vec<ActionRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, absolute_path, quarantine_path, bytes, sha256, action,
                    performed_at, purge_after, restored, source, op
             FROM action_log
             WHERE action = 'quarantine' AND restored = 0 AND purge_after <= ?1
             ORDER BY purge_after ASC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![now, limit as i64], map_row)?;
        rows.collect()
    }

    /// 记一次「直接删除」。
    ///
    /// 与 `record_purge` 的区别：这一条从未进过隔离区，所以没有隔离路径可记。
    /// `restored` 直接置 1，因为它不存在「还能还原」的状态——把它留在待还原
    /// 列表里会让用户以为东西还在。
    ///
    /// 审计里 `action` 仍是 `purge`：从时间线上看，这两者都是「内容永久消失」，
    /// 区别只在有没有经过隔离期，而 `quarantine_path` 是否为空已经说明了这一点。
    pub fn record_direct_delete(
        &self,
        path: &Path,
        bytes: u64,
        performed_at: &str,
        source: CleanSource,
    ) -> rusqlite::Result<i64> {
        self.conn.execute(
            "INSERT INTO action_log
             (absolute_path, quarantine_path, bytes, sha256, action, performed_at,
              purge_after, restored, source)
             VALUES (?1, '', ?2, NULL, 'purge', ?3, ?3, 1, ?4)",
            params![
                path.to_string_lossy(),
                bytes as i64,
                performed_at,
                source.as_str()
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// 记一次清除（隔离区里的内容被真正删除）。
    ///
    /// 同时把原始的 quarantine 行标记为 `restored = 1`——它的意思在这里是
    /// 「这一条不再待还原」。不这么做的话，隔离区列表会继续列出已经不存在的
    /// 内容，用户点「还原」才发现东西早就没了；而 `purge_due` 也会反复
    /// 尝试清除同一条记录。
    pub fn record_purge(&self, id: i64, performed_at: &str) -> rusqlite::Result<()> {
        let record = self.find_pending(id)?;
        let Some(rec) = record else {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        };
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO action_log
             (absolute_path, quarantine_path, bytes, sha256, action, performed_at, purge_after, restored, source)
             VALUES (?1, ?2, ?3, ?4, 'purge', ?5, ?6, 1, ?7)",
            params![
                rec.absolute_path.to_string_lossy(),
                rec.quarantine_path.to_string_lossy(),
                rec.bytes as i64,
                rec.sha256,
                performed_at,
                rec.purge_after,
                rec.source.as_str()
            ],
        )?;
        tx.execute(
            "UPDATE action_log SET restored = 1 WHERE id = ?1",
            params![id],
        )?;
        tx.commit()
    }
}

fn map_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ActionRecord> {
    let action: String = r.get(5)?;
    Ok(ActionRecord {
        id: r.get(0)?,
        absolute_path: PathBuf::from(r.get::<_, String>(1)?),
        quarantine_path: PathBuf::from(r.get::<_, String>(2)?),
        bytes: r.get::<_, i64>(3)? as u64,
        sha256: r.get(4)?,
        action: match action.as_str() {
            "quarantine" => ActionKind::Quarantine,
            "restore" => ActionKind::Restore,
            _ => ActionKind::Purge,
        },
        op: OpKind::from_str(&r.get::<_, String>(10)?),
        performed_at: r.get(6)?,
        purge_after: r.get(7)?,
        restored: r.get::<_, i64>(8)? != 0,
        source: CleanSource::from_str(&r.get::<_, String>(9)?),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> AuditStore {
        AuditStore::in_memory().unwrap()
    }

    #[test]
    fn quarantine_then_find() {
        let s = store();
        let id = s
            .record_quarantine(&NewQuarantine {
                absolute: Path::new("/x/cache"),
                quarantine: Path::new("/q/1/cache"),
                bytes: 1234,
                sha256: "abc",
                performed_at: "2026-10-20T10:00:00Z",
                purge_after: "2026-10-27T10:00:00Z",
                source: CleanSource::Manual,
            })
            .unwrap();
        let rec = s.find_pending(id).unwrap().unwrap();
        assert_eq!(rec.bytes, 1234);
        assert_eq!(rec.action, ActionKind::Quarantine);
        assert!(!rec.restored);
    }

    #[test]
    fn restore_adds_a_row_and_marks_original() {
        let s = store();
        let id = s
            .record_quarantine(&NewQuarantine {
                absolute: Path::new("/x/cache"),
                quarantine: Path::new("/q/cache"),
                bytes: 100,
                sha256: "h",
                performed_at: "2026-10-20T10:00:00Z",
                purge_after: "2026-10-27T10:00:00Z",
                source: CleanSource::Manual,
            })
            .unwrap();

        s.record_restore(id, "2026-10-21T10:00:00Z").unwrap();

        // 原始行仍在，只是标记为已还原
        let original = s.find_pending(id).unwrap().unwrap();
        assert!(original.restored);

        // 历史里多出一条 restore 事件，时间线完整
        let all = s.list_all(10).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].action, ActionKind::Restore);
        assert_eq!(all[1].action, ActionKind::Quarantine);

        // 待还原列表里不再包含它
        assert!(s.list_pending(10).unwrap().is_empty());
    }

    #[test]
    fn restoring_unknown_id_is_an_error() {
        let s = store();
        assert!(s.record_restore(999, "2026-10-21T00:00:00Z").is_err());
    }

    #[test]
    fn due_for_purge_respects_the_deadline() {
        let s = store();
        s.record_quarantine(&NewQuarantine {
            absolute: Path::new("/a"),
            quarantine: Path::new("/q/a"),
            bytes: 1,
            sha256: "h1",
            performed_at: "2026-10-01T00:00:00Z",
            purge_after: "2026-10-08T00:00:00Z",
            source: CleanSource::Manual,
        })
        .unwrap();
        s.record_quarantine(&NewQuarantine {
            absolute: Path::new("/b"),
            quarantine: Path::new("/q/b"),
            bytes: 2,
            sha256: "h2",
            performed_at: "2026-10-19T00:00:00Z",
            purge_after: "2026-10-26T00:00:00Z",
            source: CleanSource::Manual,
        })
        .unwrap();

        let due = s.due_for_purge("2026-10-20T00:00:00Z", 10).unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].absolute_path, PathBuf::from("/a"));
    }

    #[test]
    fn restored_items_are_not_purged() {
        let s = store();
        let id = s
            .record_quarantine(&NewQuarantine {
                absolute: Path::new("/a"),
                quarantine: Path::new("/q/a"),
                bytes: 1,
                sha256: "h",
                performed_at: "2026-10-01T00:00:00Z",
                purge_after: "2026-10-08T00:00:00Z",
                source: CleanSource::Manual,
            })
            .unwrap();
        s.record_restore(id, "2026-10-02T00:00:00Z").unwrap();
        assert!(s
            .due_for_purge("2026-10-20T00:00:00Z", 10)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn list_all_is_newest_first() {
        let s = store();
        for (i, name) in ["/a", "/b", "/c"].iter().enumerate() {
            s.record_quarantine(&NewQuarantine {
                absolute: Path::new(name),
                quarantine: Path::new(&format!("/q/{i}")),
                bytes: 1,
                sha256: "h",
                performed_at: "2026-10-20T10:00:00Z",
                purge_after: "2026-10-27T10:00:00Z",
                source: CleanSource::Manual,
            })
            .unwrap();
        }
        let all = s.list_all(10).unwrap();
        assert_eq!(all[0].absolute_path, PathBuf::from("/c"));
    }

    #[test]
    fn persists_across_reopen() {
        let dir = std::env::temp_dir().join(format!("molt-audit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("audit.sqlite");

        {
            let s = AuditStore::open(&db).unwrap();
            s.record_quarantine(&NewQuarantine {
                absolute: Path::new("/x"),
                quarantine: Path::new("/q/x"),
                bytes: 42,
                sha256: "h",
                performed_at: "2026-10-20T10:00:00Z",
                purge_after: "2026-10-27T10:00:00Z",
                source: CleanSource::Manual,
            })
            .unwrap();
        }
        {
            let s = AuditStore::open(&db).unwrap();
            let pending = s.list_pending(10).unwrap();
            assert_eq!(pending.len(), 1);
            assert_eq!(pending[0].bytes, 42);
        }

        std::fs::remove_dir_all(&dir).ok();
    }
}
