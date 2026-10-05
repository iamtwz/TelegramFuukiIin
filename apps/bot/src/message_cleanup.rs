//! Follow-up posts belong to the pending first-message case, across topics.
use crate::{
    api::{Services, is_admin},
    engine::Engine,
    error::Result,
    model::{Case, CaseKind, FirstMessage, Job, Member, NewJob, ScreeningMode},
    store::{Change, change},
};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Serialize, Deserialize)]
pub(crate) struct MessageCase {
    pub id: String,
    pub date: i64,
    pub created_at: i64,
    #[serde(default)]
    pub banned_at: Option<i64>,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct CaseMessage {
    pub message: i64,
    pub date: i64,
    pub created_at: i64,
    pub deleted: bool,
}

impl<S: Services> Engine<S> {
    pub(crate) fn current_message_case(&self, chat: i64, c: &Case) -> Result<bool> {
        Ok(match c.screening {
            ScreeningMode::NewMembers => self
                .store
                .get::<Member>(chat, &format!("member:{}", c.user))?
                .is_some_and(|m| m.cycle == c.cycle),
            ScreeningMode::FirstSeen => self
                .store
                .get::<FirstMessage>(chat, &format!("first_message:{}", c.user))?
                .is_some_and(|m| m.message == c.message),
        })
    }

    pub(crate) fn track_case_message(
        &self,
        chat: i64,
        user: i64,
        message: i64,
        date: i64,
        changes: &mut Vec<Change>,
        jobs: &mut Vec<NewJob>,
    ) -> Result<()> {
        let Some(index) = self
            .store
            .get::<MessageCase>(chat, &format!("message_case:{user}"))?
        else {
            return Ok(());
        };
        let Some(c) = self
            .store
            .get::<Case>(chat, &format!("case:{}", index.id))?
        else {
            return Ok(());
        };
        if c.kind != CaseKind::Message
            || c.user != user
            || message <= c.message
            || date < index.date
            || c.created_at < self.now() - 2_592_000
            || !(matches!(c.state.as_str(), "classifying" | "review" | "enforcing")
                || c.state == "banned" && c.banned && index.banned_at.is_some())
            || index.banned_at.is_some_and(|at| date > at)
            || !self.current_message_case(chat, &c)?
        {
            return Ok(());
        }
        let key = format!("case_message:{}:{message}", c.id);
        if self.store.get::<CaseMessage>(chat, &key)?.is_none() {
            changes.push(change(
                chat,
                key,
                &CaseMessage {
                    message,
                    date,
                    // Retain metadata for the same period as its parent case.
                    created_at: c.created_at,
                    deleted: false,
                },
            )?);
            if c.banned {
                jobs.push(cleanup_job(chat, &c.id, message, self.now()));
            }
        }
        Ok(())
    }

    pub(crate) fn queue_case_messages(&self, chat: i64, c: &Case) -> Result<()> {
        let index_key = format!("message_case:{}", c.user);
        if let Some(mut index) = self.store.get::<MessageCase>(chat, &index_key)?
            && index.id == c.id
            && index.banned_at.is_none()
        {
            // Preserve the observed Telegram/local clock offset when the local
            // clock lags. The cutoff is fixed across retries and restarts.
            index.banned_at = Some(
                self.now()
                    .saturating_add(index.date.saturating_sub(index.created_at).max(0)),
            );
            self.store.put(chat, &index_key, &index)?;
        }
        let prefix = format!("case_message:{}:", c.id);
        let mut offset = 0;
        loop {
            let page = self
                .store
                .list::<CaseMessage>(chat, &prefix, None, offset, 100)?;
            let count = page.len();
            let jobs = page
                .into_iter()
                .filter(|m| !m.deleted)
                .map(|m| cleanup_job(chat, &c.id, m.message, self.now()))
                .collect::<Vec<_>>();
            // Each deletion has its own retry budget; a failed post cannot
            // repeat the ban or block cleanup of other posts.
            self.store.apply(vec![], &jobs, self.now())?;
            if count < 100 {
                break;
            }
            offset += 100;
        }
        Ok(())
    }

    pub(crate) async fn cleanup_message(&self, job: &Job) -> Result<()> {
        let id = job.payload["id"].as_str().unwrap_or("");
        let Some(message_id) = job.payload["message"].as_i64().filter(|id| *id > 0) else {
            return Ok(());
        };
        let Some(c) = self.store.get::<Case>(job.chat, &format!("case:{id}"))? else {
            return Ok(());
        };
        if c.kind != CaseKind::Message
            || !c.banned
            || !matches!(c.state.as_str(), "enforcing" | "banned")
            || !self.current_message_case(job.chat, &c)?
        {
            return Ok(());
        }
        let key = format!("case_message:{id}:{message_id}");
        let Some(mut message) = self.store.get::<CaseMessage>(job.chat, &key)? else {
            return Ok(());
        };
        if message.deleted {
            return Ok(());
        }
        let target = self.member(job.chat, c.user).await?;
        if is_admin(&target) || target["status"] != "kicked" {
            // Respect promotions and external unbans while cleanup is queued.
            return self.audit(
                job.chat,
                "message_cleanup_skipped",
                id,
                &format!(
                    "message_id={};reason={}",
                    message.message,
                    if is_admin(&target) {
                        "admin_protected"
                    } else {
                        "target_not_banned"
                    }
                ),
                None,
            );
        }
        self.services
            .telegram(
                "deleteMessage",
                json!({"chat_id":job.chat,"message_id":message.message}),
            )
            .await?;
        message.deleted = true;
        self.store.put(job.chat, &key, &message)?;
        self.audit(
            job.chat,
            "message_deleted",
            id,
            &format!("message_id={};scope=followup", message.message),
            None,
        )
    }
}

fn cleanup_job(chat: i64, id: &str, message: i64, now: i64) -> NewJob {
    NewJob::new(
        chat,
        "cleanup_message",
        &format!("{id}:{message}"),
        json!({"id":id,"message":message}),
        now,
    )
}
