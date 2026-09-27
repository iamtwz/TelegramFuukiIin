//! Guest replies are screened per message; only Telegram's caller field grants attribution.
use crate::{
    api::{Services, is_admin},
    engine::Engine,
    error::Result,
    evidence,
    model::{Case, CaseKind, GuestCaller, Member, NewJob},
    store::change,
};
use serde_json::{Value, json};

impl<S: Services> Engine<S> {
    pub(crate) fn guest_message(&self, chat: i64, m: &Value) -> Result<bool> {
        if m["from"]["is_bot"] != true
            || !(m["guest_bot_caller_user"].is_object() || m["guest_bot_caller_chat"].is_object())
        {
            return Ok(false);
        }
        let Some(message) = m["message_id"].as_i64().filter(|id| *id > 0) else {
            return Ok(true);
        };
        let id = format!("g{message}");
        let settings = self.settings(chat)?;
        if !settings.spam {
            self.screening_skip(chat, m, "spam_disabled");
            return Ok(true);
        }
        if self
            .store
            .get::<Case>(chat, &format!("case:{id}"))?
            .is_some()
        {
            self.screening_skip(chat, m, "guest_message_already_screened");
            return Ok(true);
        }
        let user = m["from"]["id"].as_i64().unwrap_or(0);
        let caller = &m["guest_bot_caller_user"];
        let caller_id = caller["id"].as_i64().filter(|id| {
            *id > 0
                && *id != user
                && *id != self.config.bot_id()
                && caller["is_bot"] == false
                && m["guest_bot_caller_chat"].is_null()
        });
        let guest_caller = if let Some(caller) = caller_id {
            let reply = &m["reply_to_message"];
            let message = if reply["chat"]["id"].as_i64() == Some(chat)
                && reply["from"]["id"].as_i64() == Some(caller)
                && reply["sender_chat"].is_null()
                && reply["guest_query_id"].is_null()
                && reply["business_connection_id"].is_null()
            {
                reply["message_id"].as_i64().filter(|id| *id > 0)
            } else {
                None
            };
            Some(GuestCaller {
                user: caller,
                cycle: self
                    .store
                    .get::<Member>(chat, &format!("member:{caller}"))?
                    .map(|m| m.cycle),
                message,
                banned: false,
                deleted: message.is_none(),
                protected: false,
            })
        } else {
            None
        };
        let case = Case {
            id: id.clone(),
            kind: CaseKind::Guest,
            guest_caller,
            join_session: None,
            cycle: String::new(),
            user,
            message,
            joined_at: 0,
            created_at: self.now(),
            evidence: Some(evidence::extract(m)),
            state: "classifying".into(),
            policy: evidence::POLICY.into(),
            screening: settings.screening,
            review_threshold: settings.review,
            ban_threshold: settings.ban,
            probability: None,
            model: None,
            actor: None,
            deleted: false,
            banned: false,
            reason: None,
        };
        self.store.apply(
            vec![change(chat, format!("case:{id}"), &case)?],
            &[NewJob::new(
                chat,
                "classify",
                &id,
                json!({"id":id}),
                self.now(),
            )],
            self.now(),
        )?;
        Ok(true)
    }

    pub(crate) async fn enforce_guest(&self, chat: i64, mut c: Case) -> Result<()> {
        let key = format!("case:{}", c.id);
        let mut failure = None;
        // Independent steps: a failure for one target must not skip the other.
        match self.member(chat, c.user).await {
            Ok(member) if is_admin(&member) => {
                self.audit(
                    chat,
                    "admin_protected",
                    &c.id,
                    &format!("guest_bot={}", c.user),
                    None,
                )?;
                c.state = "protected".into();
                return self.store.put(chat, &key, &c);
            }
            Ok(member) => {
                if !c.deleted {
                    match self
                        .services
                        .telegram(
                            "deleteMessage",
                            json!({"chat_id":chat,"message_id":c.message}),
                        )
                        .await
                    {
                        Ok(_) => {
                            c.deleted = true;
                            self.store.put(chat, &key, &c)?;
                            self.audit(
                                chat,
                                "message_deleted",
                                &c.id,
                                &format!("guest_bot={}", c.user),
                                None,
                            )?;
                        }
                        Err(err) => failure = Some(err),
                    }
                }
                if !c.banned {
                    let result = if member["status"] == "kicked" {
                        Ok(Value::Bool(true))
                    } else {
                        self.services
                            .telegram(
                                "banChatMember",
                                json!({"chat_id":chat,"user_id":c.user,"revoke_messages":true}),
                            )
                            .await
                    };
                    match result {
                        Ok(_) => {
                            c.banned = true;
                            self.store.put(chat, &key, &c)?;
                            self.audit(
                                chat,
                                "user_banned",
                                &c.id,
                                &format!("guest_bot={}", c.user),
                                None,
                            )?;
                        }
                        Err(err) => failure = Some(err),
                    }
                }
            }
            Err(err) => failure = Some(err),
        }
        if let Some(mut caller) = c.guest_caller.clone()
            && !caller.protected
        {
            let current = self
                .store
                .get::<Member>(chat, &format!("member:{}", caller.user))?;
            let changed_cycle = current.is_some_and(|m| match &caller.cycle {
                Some(cycle) => *cycle != m.cycle,
                None => m.joined_at >= c.created_at,
            });
            match self.member(chat, caller.user).await {
                Ok(member) if is_admin(&member) || changed_cycle => {
                    caller.protected = true;
                    self.audit(
                        chat,
                        "guest_caller_skipped",
                        &c.id,
                        &format!("user={};admin_or_new_membership", caller.user),
                        None,
                    )?;
                }
                Ok(member) => {
                    if !caller.deleted
                        && let Some(message) = caller.message
                    {
                        match self
                            .services
                            .telegram(
                                "deleteMessage",
                                json!({"chat_id":chat,"message_id":message}),
                            )
                            .await
                        {
                            Ok(_) => {
                                caller.deleted = true;
                                c.guest_caller = Some(caller.clone());
                                self.store.put(chat, &key, &c)?;
                                self.audit(
                                    chat,
                                    "message_deleted",
                                    &c.id,
                                    &format!("guest_caller={};message={message}", caller.user),
                                    None,
                                )?;
                            }
                            Err(err) => failure = Some(err),
                        }
                    }
                    if !caller.banned {
                        let result = if member["status"] == "kicked" {
                            Ok(Value::Bool(true))
                        } else {
                            self.services.telegram("banChatMember",json!({"chat_id":chat,"user_id":caller.user,"revoke_messages":true})).await
                        };
                        match result {
                            Ok(_) => {
                                caller.banned = true;
                                c.guest_caller = Some(caller.clone());
                                self.store.put(chat, &key, &c)?;
                                self.audit(
                                    chat,
                                    "user_banned",
                                    &c.id,
                                    &format!("guest_caller={}", caller.user),
                                    None,
                                )?;
                            }
                            Err(err) => failure = Some(err),
                        }
                    }
                }
                Err(err) => failure = Some(err),
            }
            c.guest_caller = Some(caller);
            self.store.put(chat, &key, &c)?;
        }
        if let Some(err) = failure {
            return Err(err);
        }
        c.state = "banned".into();
        self.store.put(chat, &key, &c)
    }
}
