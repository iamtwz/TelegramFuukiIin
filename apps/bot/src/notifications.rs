//! Administrative notifications must never fall back to a public group.
use crate::{
    api::{Services, is_admin},
    engine::Engine,
    error::Result,
    menus,
    model::{Case, Job},
};
use serde_json::{Value, json};

pub async fn enqueue<S: Services>(e: &Engine<S>, job: &Job) -> Result<()> {
    if job.kind == "notify" {
        let id = job.payload["id"].as_str().unwrap_or("");
        if e.store
            .get::<Case>(job.chat, &format!("case:{id}"))?
            .is_none_or(|c| c.state != "review")
        {
            return Ok(());
        }
    }
    let admins = menus::group_admins(e, job.chat).await;
    let mut recipients = e
        .config
        .super_admins
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    if let Ok(admins) = &admins {
        recipients.extend(admins);
    }
    for user in recipients {
        e.queue(
            job.chat,
            "admin_notice",
            &format!("{}:{user}", job.id),
            json!({"user":user,"kind":job.kind,"id":job.payload["id"]}),
        )?;
    }
    // Retry discovery without duplicating already queued recipients.
    admins.map(|_| ())
}

pub async fn deliver<S: Services>(e: &Engine<S>, job: &Job) -> Result<()> {
    let Some(user) = job.payload["user"].as_i64().filter(|id| *id > 0) else {
        return Ok(());
    };
    if !e.config.is_super_admin(user) && !is_admin(&e.member(job.chat, user).await?) {
        return Ok(());
    }
    let chat = job.chat;
    if job.payload["kind"] == "notify" {
        let id = job.payload["id"].as_str().unwrap_or("");
        let Some(case) = e.store.get::<Case>(chat, &format!("case:{id}"))? else {
            return Ok(());
        };
        if case.state != "review" {
            return Ok(());
        }
        let group = crate::chat_info::label(e, chat).await?;
        let probability = case.probability.map_or_else(
            || "需要人工审核".into(),
            |p| format!("垃圾消息概率： {:.1}%", p * 100.0),
        );
        let kind = if case.kind == crate::model::CaseKind::JoinProfile {
            "入群资料"
        } else {
            "消息与资料"
        };
        e.send(user, &format!("有一项{kind}需要审核\n群：{group}\n案件： {id}\n{probability}"), json!({"inline_keyboard":[[{"text":"查看待审案件","callback_data":format!("pending|{chat}|0")}]]})).await
    } else {
        let group = crate::chat_info::label(e, chat).await?;
        e.send(
            user,
            &format!(
                "群：{group}\n有任务失败，请检查：\n/health {chat}\n/audit {chat}\n/retry {chat}"
            ),
            Value::Null,
        )
        .await
    }
}
