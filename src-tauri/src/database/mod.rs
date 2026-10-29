use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const DATABASE_FILE: &str = "clipboard-history.sqlite3";

#[derive(Debug, Clone, Serialize)]
pub struct ClipboardRecord {
    pub id: i64,
    pub content: String,
    pub content_type: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub is_pinned: bool,
    pub hash: String,
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    pub search: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    pub max_history: u32,
    pub cleanup_days: u32,
    #[serde(default = "default_language")]
    pub language: String,
}

pub const DEFAULT_LANGUAGE: &str = "zh-CN";
const SUPPORTED_LANGUAGES: [&str; 2] = ["zh-CN", "en"];

fn default_language() -> String {
    DEFAULT_LANGUAGE.to_string()
}

pub struct Database {
    connection: Mutex<Connection>,
}

impl Database {
    pub fn open(app_data_dir: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(app_data_dir)
            .map_err(|error| format!("无法创建数据库目录: {error}"))?;

        let database_path = app_data_dir.join(DATABASE_FILE);
        let connection = Connection::open(&database_path)
            .map_err(|error| format!("无法打开 SQLite 数据库: {error}"))?;

        connection
            .busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|error| format!("无法设置 SQLite busy timeout: {error}"))?;
        connection
            .execute_batch(
                "PRAGMA journal_mode = WAL;
                 PRAGMA foreign_keys = ON;",
            )
            .map_err(|error| format!("无法配置 SQLite: {error}"))?;

        migrate(&connection)?;

        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    pub fn list(&self, query: ListQuery) -> Result<Vec<ClipboardRecord>, String> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| "数据库锁已失效".to_string())?;
        // 未显式指定条数时，跟随用户设置里的 max_history，
        // 避免前端写死 1000 把 5000/10000 的设置压掉。
        let limit = query
            .limit
            .unwrap_or_else(|| read_setting_u32(&connection, "max_history", 1000))
            .clamp(1, 10_000) as i64;
        let search = query.search.unwrap_or_default().trim().to_string();

        let mut statement = connection
            .prepare(
                "SELECT id, content, content_type, created_at, updated_at, is_pinned, hash
                 FROM clipboard_items
                 WHERE (?1 = '' OR instr(lower(content), lower(?1)) > 0)
                 ORDER BY is_pinned DESC, updated_at DESC
                 LIMIT ?2",
            )
            .map_err(|error| format!("准备历史记录查询失败: {error}"))?;

        let rows = statement
            .query_map(params![search, limit], map_record)
            .map_err(|error| format!("查询历史记录失败: {error}"))?;

        rows.map(|row| row.map_err(|error| format!("读取历史记录失败: {error}")))
            .collect()
    }

    pub fn upsert_text(&self, content: &str) -> Result<ClipboardRecord, String> {
        if content.trim().is_empty() {
            return Err("不能保存空白剪贴板内容".to_string());
        }

        let hash = content_hash(content);
        let now = unix_timestamp()?;
        let connection = self
            .connection
            .lock()
            .map_err(|_| "数据库锁已失效".to_string())?;

        let existing_id: Option<i64> = connection
            .query_row(
                "SELECT id FROM clipboard_items WHERE hash = ?1",
                params![hash],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| format!("检查重复记录失败: {error}"))?;

        let id = if let Some(id) = existing_id {
            connection
                .execute(
                    "UPDATE clipboard_items SET content = ?1, updated_at = ?2 WHERE id = ?3",
                    params![content, now, id],
                )
                .map_err(|error| format!("更新剪贴板记录失败: {error}"))?;
            id
        } else {
            connection
                .execute(
                    "INSERT INTO clipboard_items
                     (content, content_type, created_at, updated_at, is_pinned, hash)
                     VALUES (?1, 'text/plain', ?2, ?2, 0, ?3)",
                    params![content, now, hash],
                )
                .map_err(|error| format!("新增剪贴板记录失败: {error}"))?;
            connection.last_insert_rowid()
        };

        fetch_by_id(&connection, id)?.ok_or_else(|| "保存后无法读取剪贴板记录".to_string())
    }

    pub fn delete(&self, id: i64) -> Result<bool, String> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| "数据库锁已失效".to_string())?;
        let affected = connection
            .execute("DELETE FROM clipboard_items WHERE id = ?1", params![id])
            .map_err(|error| format!("删除剪贴板记录失败: {error}"))?;
        Ok(affected > 0)
    }

    pub fn set_pinned(&self, id: i64, pinned: bool) -> Result<bool, String> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| "数据库锁已失效".to_string())?;
        let affected = connection
            .execute(
                "UPDATE clipboard_items SET is_pinned = ?1 WHERE id = ?2",
                params![pinned as i64, id],
            )
            .map_err(|error| format!("更新固定状态失败: {error}"))?;
        Ok(affected > 0)
    }

    pub fn clear(&self) -> Result<u64, String> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| "数据库锁已失效".to_string())?;
        let affected = connection
            .execute("DELETE FROM clipboard_items", [])
            .map_err(|error| format!("清理历史记录失败: {error}"))?;
        Ok(affected as u64)
    }

    pub fn settings(&self) -> Result<AppSettings, String> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| "数据库锁已失效".to_string())?;
        Ok(AppSettings {
            max_history: read_setting_u32(&connection, "max_history", 1000),
            cleanup_days: read_setting_u32(&connection, "cleanup_days", 30),
            language: read_setting_text(&connection, "language")
                .filter(|value| SUPPORTED_LANGUAGES.contains(&value.as_str()))
                .unwrap_or_else(default_language),
        })
    }

    pub fn update_settings(&self, settings: AppSettings) -> Result<AppSettings, String> {
        if ![100, 500, 1000, 5000, 10000].contains(&settings.max_history)
            || ![0, 1, 7, 30].contains(&settings.cleanup_days)
            || !SUPPORTED_LANGUAGES.contains(&settings.language.as_str())
        {
            return Err("设置值不在允许范围内".to_string());
        }
        let connection = self
            .connection
            .lock()
            .map_err(|_| "数据库锁已失效".to_string())?;
        for (key, value) in [
            ("max_history", settings.max_history.to_string()),
            ("cleanup_days", settings.cleanup_days.to_string()),
            ("language", settings.language.clone()),
        ] {
            write_setting(&connection, key, &value)?;
        }
        Ok(settings)
    }

    pub fn cleanup(&self, settings: &AppSettings) -> Result<u64, String> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| "数据库锁已失效".to_string())?;
        let mut removed = 0u64;
        if settings.cleanup_days > 0 {
            let cutoff = unix_timestamp()? - i64::from(settings.cleanup_days) * 86_400;
            removed += connection
                .execute(
                    "DELETE FROM clipboard_items WHERE is_pinned = 0 AND updated_at < ?1",
                    params![cutoff],
                )
                .map_err(|error| format!("按时间清理历史失败: {error}"))?
                as u64;
        }
        let overflow = connection
            .execute(
                "DELETE FROM clipboard_items
                 WHERE is_pinned = 0 AND id IN (
                   SELECT id FROM clipboard_items WHERE is_pinned = 0
                   ORDER BY updated_at DESC LIMIT -1 OFFSET ?1
                 )",
                params![settings.max_history],
            )
            .map_err(|error| format!("按数量清理历史失败: {error}"))?;
        removed += overflow as u64;
        Ok(removed)
    }
}

fn migrate(connection: &Connection) -> Result<(), String> {
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                 version INTEGER PRIMARY KEY,
                 applied_at INTEGER NOT NULL
             );",
        )
        .map_err(|error| format!("初始化迁移表失败: {error}"))?;

    let applied: Option<i64> = connection
        .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
            row.get(0)
        })
        .map_err(|error| format!("读取数据库版本失败: {error}"))?;

    if applied.unwrap_or(0) < 1 {
        connection
            .execute_batch(
                "CREATE TABLE clipboard_items (
                     id INTEGER PRIMARY KEY AUTOINCREMENT,
                     content TEXT NOT NULL,
                     content_type TEXT NOT NULL DEFAULT 'text/plain',
                     created_at INTEGER NOT NULL,
                     updated_at INTEGER NOT NULL,
                     is_pinned INTEGER NOT NULL DEFAULT 0 CHECK (is_pinned IN (0, 1)),
                     hash TEXT NOT NULL UNIQUE
                 );
                 CREATE INDEX idx_clipboard_items_order
                     ON clipboard_items (is_pinned DESC, updated_at DESC);
                 CREATE INDEX idx_clipboard_items_hash
                     ON clipboard_items (hash);
                 INSERT INTO schema_migrations (version, applied_at)
                     VALUES (1, strftime('%s', 'now'));",
            )
            .map_err(|error| format!("执行数据库迁移失败: {error}"))?;
    }

    if applied.unwrap_or(0) < 2 {
        connection
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS app_settings (
                     key TEXT PRIMARY KEY,
                     value TEXT NOT NULL
                 );
                 INSERT INTO app_settings (key, value) VALUES ('max_history', '1000')
                     ON CONFLICT(key) DO NOTHING;
                 INSERT INTO app_settings (key, value) VALUES ('cleanup_days', '30')
                     ON CONFLICT(key) DO NOTHING;
                 INSERT INTO schema_migrations (version, applied_at)
                     VALUES (2, strftime('%s', 'now'));",
            )
            .map_err(|error| format!("执行数据库迁移 2 失败: {error}"))?;
    }

    if applied.unwrap_or(0) < 3 {
        connection
            .execute_batch(
                "INSERT INTO app_settings (key, value) VALUES ('language', 'zh-CN')
                     ON CONFLICT(key) DO NOTHING;
                 INSERT INTO schema_migrations (version, applied_at)
                     VALUES (3, strftime('%s', 'now'));",
            )
            .map_err(|error| format!("执行数据库迁移 3 失败: {error}"))?;
    }

    Ok(())
}

fn read_setting_text(connection: &Connection, key: &str) -> Option<String> {
    connection
        .query_row(
            "SELECT value FROM app_settings WHERE key = ?1",
            params![key],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .ok()
        .flatten()
}

fn read_setting_u32(connection: &Connection, key: &str, fallback: u32) -> u32 {
    read_setting_text(connection, key)
        .and_then(|item| item.parse::<u32>().ok())
        .unwrap_or(fallback)
}

fn write_setting(connection: &Connection, key: &str, value: &str) -> Result<(), String> {
    connection
        .execute(
            "INSERT INTO app_settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )
        .map(|_| ())
        .map_err(|error| format!("保存设置失败: {error}"))
}

fn map_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<ClipboardRecord> {
    Ok(ClipboardRecord {
        id: row.get(0)?,
        content: row.get(1)?,
        content_type: row.get(2)?,
        created_at: row.get(3)?,
        updated_at: row.get(4)?,
        is_pinned: row.get::<_, i64>(5)? != 0,
        hash: row.get(6)?,
    })
}

fn fetch_by_id(connection: &Connection, id: i64) -> Result<Option<ClipboardRecord>, String> {
    connection
        .query_row(
            "SELECT id, content, content_type, created_at, updated_at, is_pinned, hash
             FROM clipboard_items WHERE id = ?1",
            params![id],
            map_record,
        )
        .optional()
        .map_err(|error| format!("读取剪贴板记录失败: {error}"))
}

pub fn content_hash(content: &str) -> String {
    let digest = Sha256::digest(content.as_bytes());
    format!("{digest:x}")
}

fn unix_timestamp() -> Result<i64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .map_err(|error| format!("获取系统时间失败: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_database() -> Database {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be valid")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "clipboard-history-test-{}-{nonce}",
            std::process::id()
        ));
        Database::open(&path).expect("database should open")
    }

    #[test]
    fn duplicate_text_updates_existing_record() {
        let database = test_database();
        let first = database.upsert_text("hello").expect("first insert");
        let second = database.upsert_text("hello").expect("duplicate update");
        assert_eq!(first.id, second.id);
        assert_eq!(
            database
                .list(ListQuery {
                    search: None,
                    limit: None
                })
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn whitespace_only_text_is_rejected() {
        let database = test_database();
        assert!(database.upsert_text(" \n\t").is_err());
    }

    #[test]
    fn pinned_records_are_listed_first() {
        let database = test_database();
        let first = database.upsert_text("first").expect("first insert");
        database.upsert_text("second").expect("second insert");
        database.set_pinned(first.id, true).expect("pin record");
        let records = database
            .list(ListQuery {
                search: None,
                limit: None,
            })
            .unwrap();
        assert_eq!(records[0].id, first.id);
    }
}
