mod statistics;
use crate::{
    audit::{Cursor, Entry, PAGE_SIZE, Page},
    error::{Error, Result},
    model::{Job, NewJob},
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{
    path::Path,
    sync::{Mutex, MutexGuard},
    time::Duration,
};

pub type Change = (i64, String, Value);
pub fn change<T: Serialize>(chat: i64, key: impl Into<String>, value: &T) -> Result<Change> {
    Ok((chat, key.into(), serde_json::to_value(value)?))
}
pub struct Store {
    db: Mutex<Connection>,
}
impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            let new = !parent.exists();
            std::fs::create_dir_all(parent)?;
            #[cfg(unix)]
            if new {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
            }
            #[cfg(not(unix))]
            let _ = new;
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        Self::from_connection(Connection::open(path)?)
    }
    pub fn memory() -> Result<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }
    fn from_connection(db: Connection) -> Result<Self> {
        db.busy_timeout(Duration::from_secs(5))?;
        let version: i64 = db.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if version > 3 {
            return Err(Error::Config("database_schema_newer_than_binary"));
        }
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
            CREATE TABLE IF NOT EXISTS records (chat INTEGER NOT NULL, key TEXT NOT NULL, value TEXT NOT NULL, PRIMARY KEY(chat,key));
            CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS jobs (id TEXT PRIMARY KEY, chat INTEGER NOT NULL, kind TEXT NOT NULL, payload TEXT NOT NULL, status TEXT NOT NULL DEFAULT 'pending', attempts INTEGER NOT NULL DEFAULT 0, next_at INTEGER NOT NULL, created_at INTEGER NOT NULL, error TEXT);
            CREATE INDEX IF NOT EXISTS jobs_due ON jobs(status,next_at);
            CREATE TABLE IF NOT EXISTS audit (id INTEGER PRIMARY KEY, chat INTEGER NOT NULL, at INTEGER NOT NULL, action TEXT NOT NULL, actor INTEGER, target TEXT NOT NULL, detail TEXT NOT NULL);
            CREATE INDEX IF NOT EXISTS audit_chat_at ON audit(chat,at);
            CREATE INDEX IF NOT EXISTS audit_chat_id ON audit(chat,id);
            INSERT INTO meta VALUES ('audit_sequence',(SELECT COALESCE(MAX(id),0) FROM audit))
                ON CONFLICT(key) DO UPDATE SET value=MAX(meta.value,excluded.value);
            INSERT OR IGNORE INTO records(chat,key,value)
                SELECT chat,'first_message:' || json_extract(value,'$.user'),
                    json_object('user',json_extract(value,'$.user'),
                                'message',json_extract(value,'$.first_message'),
                                'created_at',json_extract(value,'$.joined_at'))
                FROM records WHERE key LIKE 'member:%'
                    AND json_type(value,'$.first_message')='integer'
                    AND json_extract(value,'$.first_message')>0;")?;
        statistics::schema(&db)?;
        if version < 3 {
            db.execute_batch("BEGIN IMMEDIATE; ALTER TABLE statistics_cases ADD COLUMN kind TEXT NOT NULL DEFAULT 'message'; PRAGMA user_version=3; COMMIT;")?;
        }
        Ok(Self { db: Mutex::new(db) })
    }
    fn db(&self) -> Result<MutexGuard<'_, Connection>> {
        self.db
            .lock()
            .map_err(|_| Error::Config("database_lock_poisoned"))
    }
    pub fn get<T: DeserializeOwned>(&self, chat: i64, key: &str) -> Result<Option<T>> {
        let raw: Option<String> = self
            .db()?
            .query_row(
                "SELECT value FROM records WHERE chat=?1 AND key=?2",
                params![chat, key],
                |r| r.get(0),
            )
            .optional()?;
        raw.map(|s| serde_json::from_str(&s).map_err(Error::from))
            .transpose()
    }
    pub fn put<T: Serialize>(&self, chat: i64, key: &str, value: &T) -> Result<()> {
        self.apply(vec![change(chat, key, value)?], &[], 0)
    }
    pub fn apply(&self, changes: Vec<Change>, jobs: &[NewJob], now: i64) -> Result<()> {
        let mut db = self.db()?;
        let tx = db.transaction()?;
        for (chat, key, value) in changes {
            statistics::capture(&tx, chat, &key, &value, now)?;
            tx.execute("INSERT INTO records VALUES (?1,?2,?3) ON CONFLICT(chat,key) DO UPDATE SET value=excluded.value", params![chat,key,value.to_string()])?;
        }
        for job in jobs {
            insert_job(&tx, job, now)?;
        }
        tx.commit()?;
        Ok(())
    }
    pub fn enqueue(&self, job: &NewJob, now: i64) -> Result<()> {
        self.apply(vec![], std::slice::from_ref(job), now)
    }
    pub fn persist_updates(&self, updates: &[Value], now: i64) -> Result<()> {
        let mut db = self.db()?;
        let tx = db.transaction()?;
        let mut next: Option<i64> = None;
        for update in updates {
            let id = update["update_id"]
                .as_i64()
                .filter(|id| *id >= 0)
                .ok_or(Error::Config("invalid_update_id"))?;
            insert_job(
                &tx,
                &NewJob::new(0, "update", &id.to_string(), update.clone(), now),
                now,
            )?;
            next = Some(next.map_or(id + 1, |n| n.max(id + 1)));
        }
        if let Some(next) = next {
            tx.execute("INSERT INTO meta VALUES ('offset',?1) ON CONFLICT(key) DO UPDATE SET value=MAX(meta.value,excluded.value)", [next])?;
        }
        tx.commit()?;
        Ok(())
    }
    pub fn offset(&self) -> Result<i64> {
        Ok(self
            .db()?
            .query_row("SELECT value FROM meta WHERE key='offset'", [], |r| {
                r.get(0)
            })
            .optional()?
            .unwrap_or(0))
    }
    pub fn recover(&self, now: i64) -> Result<()> {
        self.db()?.execute(
            "UPDATE jobs SET status='pending',next_at=?1 WHERE status='running'",
            [now],
        )?;
        Ok(())
    }
    pub fn claim(&self, now: i64) -> Result<Option<Job>> {
        let mut db = self.db()?;
        let tx = db.transaction()?;
        tx.execute(
            "UPDATE jobs SET status='pending' WHERE status='running' AND next_at<=?1",
            [now],
        )?;
        let row = tx.query_row("SELECT id,chat,kind,payload,attempts,created_at FROM jobs WHERE status='pending' AND next_at<=?1 ORDER BY CASE kind WHEN 'update' THEN 0 WHEN 'invite' THEN 1 WHEN 'verify_captcha' THEN 1 WHEN 'approve' THEN 1 WHEN 'admin' THEN 2 WHEN 'enforce' THEN 3 ELSE 4 END, next_at,rowid LIMIT 1",[now],|r| Ok((r.get::<_,String>(0)?,r.get(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,u32>(4)?,r.get(5)?))).optional()?;
        let job = if let Some((id, chat, kind, payload, attempts, created_at)) = row {
            tx.execute(
                "UPDATE jobs SET status='running',attempts=attempts+1,next_at=?1 WHERE id=?2",
                params![now + 90, id],
            )?;
            Some(Job {
                id,
                chat,
                kind,
                payload: serde_json::from_str(&payload)?,
                attempts: attempts + 1,
                created_at,
            })
        } else {
            None
        };
        tx.commit()?;
        Ok(job)
    }
    pub fn complete(&self, id: &str) -> Result<()> {
        self.db()?.execute(
            "UPDATE jobs SET status='done',payload='{}',error=NULL WHERE id=?1",
            [id],
        )?;
        Ok(())
    }
    pub fn fail(&self, id: &str, error: &str, due: i64, dead: bool) -> Result<()> {
        self.db()?.execute(
            "UPDATE jobs SET status=?1,error=?2,next_at=?3 WHERE id=?4",
            params![if dead { "dead" } else { "pending" }, error, due, id],
        )?;
        Ok(())
    }
    pub fn retry(&self, chat: i64, now: i64) -> Result<usize> {
        Ok(self.db()?.execute("UPDATE jobs SET status='pending',attempts=0,next_at=?1 WHERE chat=?2 AND status='dead' AND created_at>?3",params![now,chat,now-604800])?)
    }
    pub fn list<T: DeserializeOwned>(
        &self,
        chat: i64,
        prefix: &str,
        state: Option<&str>,
        offset: i64,
        limit: i64,
    ) -> Result<Vec<T>> {
        let db = self.db()?;
        let mut stmt=db.prepare("SELECT value FROM records WHERE chat=?1 AND key LIKE ?2 AND (?3 IS NULL OR json_extract(value,'$.state')=?3) ORDER BY json_extract(value,'$.created_at') DESC,key LIMIT ?4 OFFSET ?5")?;
        let raw = stmt
            .query_map(
                params![chat, format!("{prefix}%"), state, limit, offset],
                |r| r.get::<_, String>(0),
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        raw.into_iter()
            .map(|s| serde_json::from_str(&s).map_err(Error::from))
            .collect()
    }
    pub fn stats(&self, chat: i64) -> Result<Value> {
        let db = self.db()?;
        let mut stmt =
            db.prepare("SELECT status,COUNT(*) FROM jobs WHERE chat=?1 GROUP BY status")?;
        let rows = stmt.query_map([chat], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })?;
        let mut map = serde_json::Map::new();
        for row in rows {
            let (k, v) = row?;
            map.insert(k, json!(v));
        }
        Ok(Value::Object(map))
    }
    pub fn audit(
        &self,
        chat: i64,
        now: i64,
        action: &str,
        actor: Option<i64>,
        target: &str,
        detail: &str,
    ) -> Result<()> {
        let mut db = self.db()?;
        let tx = db.transaction()?;
        // Keep IDs monotonic after retention deletes every row, so an old button
        // can never resolve to a different event. Also handles pre-upgrade rows.
        tx.execute(
            "INSERT INTO audit(id,chat,at,action,actor,target,detail)
             SELECT MAX(COALESCE((SELECT MAX(id) FROM audit),0),COALESCE((SELECT value FROM meta WHERE key='audit_sequence'),0))+1,?1,?2,?3,?4,?5,?6",
            params![chat, now, action, actor, target, detail],
        )?;
        tx.execute("INSERT INTO meta VALUES ('audit_sequence',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [tx.last_insert_rowid()])?;
        tx.commit()?;
        Ok(())
    }
    pub fn audits(&self, chat: i64) -> Result<Vec<Value>> {
        let db = self.db()?;
        let mut stmt=db.prepare("SELECT at,action,actor,target,detail FROM audit WHERE chat=?1 ORDER BY id DESC LIMIT 15")?;
        Ok(stmt.query_map([chat],|r| Ok(json!({"at":r.get::<_,i64>(0)?,"action":r.get::<_,String>(1)?,"actor":r.get::<_,Option<i64>>(2)?,"target":r.get::<_,String>(3)?,"detail":r.get::<_,String>(4)?})))?.collect::<std::result::Result<Vec<_>,_>>()?)
    }
    pub fn audit_page(&self, chat: i64, cursor: Cursor) -> Result<Page> {
        let db = self.db()?;
        let latest: i64 = db.query_row(
            "SELECT COALESCE(MAX(id),0) FROM audit WHERE chat=?1",
            [chat],
            |r| r.get(0),
        )?;
        let snapshot = cursor.snapshot.unwrap_or(latest).clamp(0, latest);
        let actions = serde_json::to_string(cursor.filter.actions())?;
        let all = cursor.filter.actions().is_empty();
        let total: i64 = db.query_row("SELECT COUNT(*) FROM audit WHERE chat=?1 AND id<=?2 AND (?3 OR action IN (SELECT value FROM json_each(?4)))", params![chat,snapshot,all,actions], |r| r.get(0))?;
        let pages = ((total + PAGE_SIZE - 1) / PAGE_SIZE).clamp(1, 1_000_001) as u32;
        let page = cursor.page.min(pages - 1);
        let mut stmt = db.prepare("SELECT id,strftime('%Y-%m-%d %H:%M:%S',at,'unixepoch'),action,actor,target,detail FROM audit WHERE chat=?1 AND id<=?2 AND (?3 OR action IN (SELECT value FROM json_each(?4))) ORDER BY id DESC LIMIT ?5 OFFSET ?6")?;
        let entries = stmt
            .query_map(
                params![
                    chat,
                    snapshot,
                    all,
                    actions,
                    PAGE_SIZE,
                    i64::from(page) * PAGE_SIZE
                ],
                audit_entry,
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(Page {
            cursor: Cursor {
                snapshot: Some(snapshot),
                page,
                ..cursor
            },
            total,
            pages,
            entries,
        })
    }
    pub fn audit_entry(&self, chat: i64, id: i64) -> Result<Option<Entry>> {
        Ok(self.db()?.query_row("SELECT id,strftime('%Y-%m-%d %H:%M:%S',at,'unixepoch'),action,actor,target,detail FROM audit WHERE chat=?1 AND id=?2", params![chat,id], audit_entry).optional()?)
    }
    pub fn prune(&self, now: i64) -> Result<()> {
        let mut db = self.db()?;
        let tx = db.transaction()?;
        statistics::prune(&tx, now)?;
        tx.execute("DELETE FROM audit WHERE at<?1", [now - 2_592_000])?;
        tx.execute("DELETE FROM records WHERE key LIKE 'manual_jev:%' AND json_extract(value,'$.created_at')<?1", [now-604800])?;
        tx.execute("DELETE FROM records WHERE key LIKE 'jev_limit:%' AND json_extract(value,'$.created_at')<?1", [now-86400])?;
        tx.execute(
            "DELETE FROM jobs WHERE status IN ('done','dead') AND created_at<?1",
            [now - 604800],
        )?;
        tx.execute("UPDATE records SET value=json_set(value,'$.verification.token','') WHERE key LIKE 'session:%' AND json_type(value,'$.verification')='object' AND (json_extract(value,'$.claims.exp')<=?1 OR json_extract(value,'$.verification.received_at')<=?2)", params![now,now-300])?;
        tx.execute("DELETE FROM records WHERE (key LIKE 'session:%' OR key LIKE 'latest:%') AND json_extract(value,'$.created_at')<?1",[now-86400])?;
        tx.execute("UPDATE records SET value=json_set(value,'$.evidence',json('null')) WHERE key LIKE 'case:%' AND json_extract(value,'$.state') NOT IN ('review','classifying','enforcing') AND json_extract(value,'$.created_at')<?1",[now-604800])?;
        tx.execute("UPDATE records SET value=json_set(value,'$.evidence',json('null'),'$.state','expired') WHERE key LIKE 'case:%' AND json_extract(value,'$.state') IN ('review','classifying','enforcing') AND json_extract(value,'$.created_at')<?1",[now-2_592_000])?;
        tx.execute(
            "DELETE FROM records WHERE key LIKE 'case:%' AND json_extract(value,'$.created_at')<?1",
            [now - 3_196_800],
        )?;
        tx.commit()?;
        Ok(())
    }
}
fn audit_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<Entry> {
    Ok(Entry {
        id: row.get(0)?,
        time: row.get(1)?,
        action: row.get(2)?,
        actor: row.get(3)?,
        target: row.get(4)?,
        detail: row.get(5)?,
    })
}
fn insert_job(db: &Connection, job: &NewJob, now: i64) -> Result<()> {
    db.execute("INSERT OR IGNORE INTO jobs(id,chat,kind,payload,next_at,created_at) VALUES (?1,?2,?3,?4,?5,?6)",params![job.id,job.chat,job.kind,job.payload.to_string(),job.due,now])?;
    Ok(())
}
