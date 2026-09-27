use super::Store;
use crate::{
    error::{Error, Result},
    model::{Case, CaseKind, Session},
    statistics::{RETENTION_DAYS, Report, Usage, day},
};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;

pub(super) fn schema(db: &Connection) -> Result<()> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS statistics_coverage (chat INTEGER PRIMARY KEY, started_at INTEGER NOT NULL);
        CREATE TABLE IF NOT EXISTS statistics_joins (chat INTEGER NOT NULL, day INTEGER NOT NULL, user INTEGER NOT NULL, approved INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(chat,day,user));
        CREATE TABLE IF NOT EXISTS statistics_cases (chat INTEGER NOT NULL, id TEXT NOT NULL, day INTEGER NOT NULL, outcome TEXT NOT NULL, PRIMARY KEY(chat,id));
        CREATE INDEX IF NOT EXISTS statistics_cases_day ON statistics_cases(chat,day);
        CREATE TABLE IF NOT EXISTS statistics_usage (chat INTEGER NOT NULL, id TEXT NOT NULL, day INTEGER NOT NULL, input_tokens INTEGER, output_tokens INTEGER, cost_usd REAL, PRIMARY KEY(chat,id));
        CREATE INDEX IF NOT EXISTS statistics_usage_day ON statistics_usage(chat,day);")?;
    Ok(())
}
fn start(db: &Connection, chat: i64, now: i64) -> Result<()> {
    db.execute(
        "INSERT OR IGNORE INTO statistics_coverage VALUES (?1,?2)",
        params![chat, now],
    )?;
    Ok(())
}

/// Called within the same transaction as the case/session and follow-up jobs.
pub(super) fn capture(
    db: &Connection,
    chat: i64,
    key: &str,
    value: &Value,
    now: i64,
) -> Result<()> {
    if key.starts_with("case:") {
        let c: Case = serde_json::from_value(value.clone())?;
        let outcome = if let Some(p) = c.probability {
            if p >= c.ban_threshold {
                "spam"
            } else if p >= c.review_threshold {
                "review"
            } else {
                "clean"
            }
        } else if c.reason.is_some() {
            "failed"
        } else if c.state == "classifying" {
            "pending"
        } else {
            "skipped"
        };
        // Update existing facts even when a plain put (now=0) changes case state.
        let updated = db.execute(
            "UPDATE statistics_cases SET outcome=?1 WHERE chat=?2 AND id=?3",
            params![outcome, chat, c.id],
        )?;
        if updated == 0 && now > 0 {
            start(db, chat, now)?;
            let since: i64 = db.query_row(
                "SELECT started_at FROM statistics_coverage WHERE chat=?1",
                [chat],
                |r| r.get(0),
            )?;
            if c.created_at >= since {
                db.execute(
                    "INSERT OR IGNORE INTO statistics_cases(chat,id,day,outcome,kind) VALUES (?1,?2,?3,?4,?5)",
                    params![chat, c.id, day(c.created_at), outcome, if c.kind==CaseKind::JoinProfile {"join_profile"} else {"message"}],
                )?;
            }
        }
    } else if key.starts_with("session:") && value["state"] == "approved" {
        let s: Session = serde_json::from_value(value.clone())?;
        db.execute(
            "UPDATE statistics_joins SET approved=1 WHERE chat=?1 AND day=?2 AND user=?3",
            params![
                chat,
                day(s.requested_at),
                i64::try_from(s.claims.user).map_err(|_| Error::Config("invalid_join_user"))?
            ],
        )?;
    }
    Ok(())
}

impl Store {
    pub fn start_statistics(&self, chat: i64, now: i64) -> Result<()> {
        let db = self.db()?;
        start(&db, chat, now)
    }
    pub fn statistics_join(&self, chat: i64, user: i64, at: i64, now: i64) -> Result<()> {
        let mut db = self.db()?;
        let tx = db.transaction()?;
        start(&tx, chat, now)?;
        let since: i64 = tx.query_row(
            "SELECT started_at FROM statistics_coverage WHERE chat=?1",
            [chat],
            |r| r.get(0),
        )?;
        if at >= since {
            tx.execute(
                "INSERT OR IGNORE INTO statistics_joins(chat,day,user) VALUES (?1,?2,?3)",
                params![chat, day(at), user],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
    /// Persist intent before a network call. A crash/timeout leaves unknown usage,
    /// not zero. Every real retry gets a new ID instead of overwriting its cost.
    pub fn start_usage(&self, chat: i64, now: i64) -> Result<String> {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let mut db = self.db()?;
        let tx = db.transaction()?;
        start(&tx, chat, now)?;
        tx.execute(
            "INSERT INTO statistics_usage(chat,id,day) VALUES (?1,?2,?3)",
            params![chat, id, day(now)],
        )?;
        tx.commit()?;
        Ok(id)
    }
    pub fn finish_usage(&self, chat: i64, id: &str, usage: &Usage) -> Result<()> {
        self.db()?.execute("UPDATE statistics_usage SET input_tokens=?1,output_tokens=?2,cost_usd=?3 WHERE chat=?4 AND id=?5", params![usage.input_tokens,usage.output_tokens,usage.cost_usd,chat,id])?;
        Ok(())
    }
    pub fn statistics_date(&self, raw: &str) -> Result<Option<i64>> {
        if raw.len() != 10 || raw.as_bytes()[4] != b'-' || raw.as_bytes()[7] != b'-' {
            return Ok(None);
        }
        Ok(self
            .db()?
            .query_row("SELECT unixepoch(?1)/86400 WHERE date(?1)=?1", [raw], |r| {
                r.get(0)
            })
            .optional()?)
    }
    pub fn statistics_report(&self, chat: i64, first: i64, last: i64, now: i64) -> Result<Report> {
        let mut db = self.db()?;
        let tx = db.transaction()?;
        start(&tx, chat, now)?;
        let (since, since_label): (i64, String) = tx.query_row("SELECT started_at,datetime(started_at,'unixepoch','+8 hours') FROM statistics_coverage WHERE chat=?1", [chat], |r| Ok((r.get(0)?,r.get(1)?)))?;
        let (start, end) = tx.query_row(
            "SELECT date(?1*86400,'unixepoch'),date(?2*86400,'unixepoch')",
            params![first, last],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let mut report = Report {
            start,
            end,
            since: since_label,
            partial: first * 86400 - 28800 < since,
            unavailable: (last + 1) * 86400 - 28800 <= since,
            ..Report::default()
        };
        (report.applicants, report.approved) = tx.query_row("SELECT COUNT(*),COALESCE(SUM(approved),0) FROM statistics_joins WHERE chat=?1 AND day BETWEEN ?2 AND ?3", params![chat,first,last], |r| Ok((r.get(0)?,r.get(1)?)))?;
        {
            let mut stmt = tx.prepare("SELECT outcome,COUNT(*) FROM statistics_cases WHERE chat=?1 AND day BETWEEN ?2 AND ?3 AND kind='message' GROUP BY outcome")?;
            for row in stmt.query_map(params![chat, first, last], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
            })? {
                let (outcome, count) = row?;
                report.messages += count;
                match outcome.as_str() {
                    "spam" => {
                        report.spam = count;
                        report.classified += count;
                    }
                    "review" => {
                        report.review = count;
                        report.classified += count;
                    }
                    "clean" => report.classified += count,
                    "failed" => report.failed = count,
                    "pending" => report.pending = count,
                    _ => report.skipped += count,
                }
            }
        }
        (report.profiles,report.profile_spam,report.profile_review,report.profile_failed) = tx.query_row("SELECT COUNT(*),COALESCE(SUM(outcome='spam'),0),COALESCE(SUM(outcome='review'),0),COALESCE(SUM(outcome='failed'),0) FROM statistics_cases WHERE chat=?1 AND day BETWEEN ?2 AND ?3 AND kind='join_profile'",params![chat,first,last],|r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?;
        (report.attempts,report.input,report.output,report.input_known,report.output_known,report.cost,report.cost_known) = tx.query_row("SELECT COUNT(*),COALESCE(SUM(input_tokens),0),COALESCE(SUM(output_tokens),0),COUNT(input_tokens),COUNT(output_tokens),COALESCE(SUM(cost_usd),0),COUNT(cost_usd) FROM statistics_usage WHERE chat=?1 AND day BETWEEN ?2 AND ?3", params![chat,first,last], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?)))?;
        tx.commit()?;
        Ok(report)
    }
}

pub(super) fn prune(db: &Connection, now: i64) -> Result<()> {
    let cutoff = day(now) - RETENTION_DAYS + 1;
    for table in ["statistics_joins", "statistics_cases", "statistics_usage"] {
        db.execute(&format!("DELETE FROM {table} WHERE day<?1"), [cutoff])?;
    }
    Ok(())
}
