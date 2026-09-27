//! Group-visible Jev results and independent, non-enforcing manual assessments.
use crate::{
    api::{Services, present},
    engine::Engine,
    error::{Error, Result},
    evidence,
    markdown::{code, escape},
    model::{Case, CaseKind, Job, NewJob},
    store::change,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub fn is_command(message: &Value, bot: &str) -> bool {
    crate::admin::command(message["text"].as_str().unwrap_or(""), bot)
        .is_some_and(|(name, _)| name == "spamcheck")
}

enum Input {
    Identity(i64),
    Text(String),
}
fn parse_input(text: &str) -> Option<Input> {
    let (_, arguments) = text.trim_start().split_once(char::is_whitespace)?;
    let (kind, value) = arguments.trim_start().split_once(char::is_whitespace)?;
    match kind {
        "id" => {
            let value = value.trim();
            if !value.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            value
                .parse()
                .ok()
                .filter(|id| (1..=9_007_199_254_740_991).contains(id))
                .map(Input::Identity)
        }
        "text" if !value.trim().is_empty() && value.len() <= 16_384 => {
            Some(Input::Text(value.into()))
        }
        _ => None,
    }
}

fn assessment(model: &str, probability: Option<f64>, review: f64, ban: f64) -> String {
    let result = match probability {
        Some(p) if p >= ban => "高风险 / High risk",
        Some(p) if p >= review => "人工复核 / Manual review",
        Some(_) => "低风险 / Low risk",
        None => "模型未返回有效判断 / No valid model decision",
    };
    let probability = probability.map_or_else(
        || escape("未知 / Unknown"),
        |p| code(&format!("{p} ({:.2}%)", p * 100.0)),
    );
    // Bound untrusted model names before adding MarkdownV2 markup.
    let model: String = model.chars().take(128).collect();
    format!(
        "*Jev 判断 / Jev assessment*\n*模型 / Model*：{}\n*Spam 概率 / Spam probability*：{probability}\n*结果 / Result*：{}\n*人工复核阈值 / Review threshold*：{}\n*高风险阈值 / High risk threshold*：{}",
        code(&model),
        escape(result),
        code(&review.to_string()),
        code(&ban.to_string()),
    )
}

pub fn case_notice<S: Services>(e: &Engine<S>, c: &Case, chat: i64) -> Option<NewJob> {
    if !e.config.is_verbose(chat) || (c.probability.is_none() && c.model.is_none()) {
        return None;
    }
    let kind = match c.kind {
        CaseKind::Message => "首条消息 / First message",
        CaseKind::Guest => "Guest Bot 消息 / Guest Bot message",
        CaseKind::JoinProfile => "入群资料 / Join profile",
    };
    let action = if c.probability.is_none() {
        "模型调用失败，已转人工复核。 / Model call failed; queued for manual review."
    } else {
        match c.state.as_str() {
            "enforcing" => "已进入自动处理流程。 / Automatic enforcement queued.",
            "review" => "已转人工复核。 / Queued for manual review.",
            _ => "已放行。 / Allowed.",
        }
    };
    let text = format!(
        "{}\n*对象 ID / Target ID*：{}\n*类型 / Type*：{}\n*案件 / Case*：{}\n{}",
        assessment(
            c.model.as_deref().unwrap_or(&e.config.jev_model),
            c.probability,
            c.review_threshold,
            c.ban_threshold
        ),
        code(&c.user.to_string()),
        escape(kind),
        code(&c.id),
        escape(action),
    );
    Some(NewJob::new(
        chat,
        "verbose_notice",
        &format!("case:{}", c.id),
        json!({"text":text}),
        e.now(),
    ))
}

#[derive(Serialize, Deserialize)]
struct Manual {
    created_at: i64,
    user: i64,
    review: f64,
    ban: f64,
    evidence: Option<Value>,
    state: String,
}
#[derive(Serialize, Deserialize)]
struct Limit {
    created_at: i64,
    count: u32,
}

impl<S: Services> Engine<S> {
    fn manual_notice(&self, job: &Job, text: String) -> NewJob {
        let message = &job.payload["message"];
        NewJob::new(
            job.chat,
            "verbose_notice",
            &format!("manual:{}", job.id),
            json!({
                "text":text,"reply":message["message_id"],"thread":message["message_thread_id"],
            }),
            self.now(),
        )
    }
    fn manual_done(&self, job: &Job, mut manual: Manual, text: String) -> Result<()> {
        manual.state = "done".into();
        manual.evidence = None;
        self.store.apply(
            vec![change(job.chat, format!("manual_jev:{}", job.id), &manual)?],
            &[self.manual_notice(job, text)],
            self.now(),
        )
    }
    async fn manual_identity(&self, chat: i64, id: i64) -> Result<Option<Value>> {
        let full = self
            .services
            .telegram("getChat", json!({"chat_id":id}))
            .await;
        let (full, full_error) = match full {
            Ok(full) if full["id"].as_i64() == Some(id) && full["type"] == "private" => {
                (Some(full), None)
            }
            Err(error) => (None, Some(error)),
            _ => (None, None),
        };
        let membership = self.member(chat, id).await;
        let identity = match &membership {
            Ok(member) if member["user"]["id"].as_i64() == Some(id) => {
                Some(evidence::user(&member["user"]))
            }
            _ => full.as_ref().map(evidence::user),
        };
        let Some(identity) = identity else {
            if let Err(error) = membership
                && !error.permanent()
            {
                return Err(error);
            }
            if let Some(error) = full_error
                && !error.permanent()
            {
                return Err(error);
            }
            return Ok(None);
        };
        let mut profile =
            json!({"identity":identity,"observed_at":self.now(),"lookup":"unavailable"});
        if let Some(full) = full {
            profile["lookup"] = json!("available");
            for key in ["username", "first_name", "last_name"] {
                if let Some(value) = full[key].as_str() {
                    profile["identity"][key] = json!(value.chars().take(256).collect::<String>());
                }
            }
            if let Some(bio) = full["bio"].as_str() {
                profile["bio"] = crate::profiles::bio(bio);
                profile["bio_source"] = json!("getChat");
            }
        }
        Ok(Some(
            json!({"context":"join_profile","user":identity,"message":{},"profiles":{"user":profile}}),
        ))
    }
    pub(crate) fn discard_manual_jev(&self, job: &Job) -> Result<()> {
        let key = format!("manual_jev:{}", job.id);
        if let Some(mut manual) = self.store.get::<Manual>(job.chat, &key)? {
            manual.state = "done".into();
            manual.evidence = None;
            self.store.put(job.chat, &key, &manual)?;
        }
        Ok(())
    }
    pub(crate) async fn manual_jev(&self, job: &Job) -> Result<()> {
        let message = &job.payload["message"];
        let Some(user) = message["from"]["id"]
            .as_i64()
            .filter(|id| (1..=9_007_199_254_740_991).contains(id))
        else {
            return self.discard_manual_jev(job);
        };
        if !self.config.is_verbose(job.chat)
            || message["chat"]["id"].as_i64() != Some(job.chat)
            || !matches!(
                message["chat"]["type"].as_str(),
                Some("group" | "supergroup")
            )
            || message["from"]["is_bot"] == true
            || !message["sender_chat"].is_null()
            || !message["business_connection_id"].is_null()
            || !message["guest_query_id"].is_null()
            || !is_command(message, &self.config.bot_username)
            || !present(&self.member(job.chat, user).await?)
        {
            return self.discard_manual_jev(job);
        }
        let key = format!("manual_jev:{}", job.id);
        let old = self.store.get::<Manual>(job.chat, &key)?;
        if old.as_ref().is_some_and(|m| m.state == "done") {
            return Ok(());
        }
        if self.now() - job.created_at > 300 {
            let text =
                escape("请求已过期，请重新发送命令。\nRequest expired; send the command again.");
            return if let Some(manual) = old {
                self.manual_done(job, manual, text)
            } else {
                self.store
                    .enqueue(&self.manual_notice(job, text), self.now())
            };
        }
        let Some(input) = parse_input(message["text"].as_str().unwrap_or("")) else {
            return self.store.enqueue(
                &self.manual_notice(
                    job,
                    format!(
                        "*Jev 手动判断 / Manual Jev assessment*\n{}\n{}\n{}",
                        escape("用法 / Usage:"),
                        code("/spamcheck id 123456"),
                        code("/spamcheck text 待判断内容 / text to assess"),
                    ),
                ),
                self.now(),
            );
        };
        let mut manual = if let Some(old) = old {
            old
        } else {
            let user_key = format!("jev_limit:user:{user}");
            let user_limit = self.store.get::<Limit>(job.chat, &user_key)?;
            let mut group_limit = self
                .store
                .get::<Limit>(job.chat, "jev_limit:group")?
                .filter(|l| self.now() - l.created_at < 60)
                .unwrap_or(Limit {
                    created_at: self.now(),
                    count: 0,
                });
            if user_limit.is_some_and(|l| self.now() - l.created_at < 10) || group_limit.count >= 10
            {
                return self.store.enqueue(&self.manual_notice(job, escape("请求过于频繁：每人间隔 10 秒，全群每分钟最多 10 次。\nToo many requests: wait 10 seconds between requests; up to 10 requests per group per minute.")), self.now());
            }
            group_limit.count += 1;
            let settings = self.settings(job.chat)?;
            let manual = Manual {
                created_at: self.now(),
                user,
                review: settings.review,
                ban: settings.ban,
                evidence: None,
                state: "pending".into(),
            };
            self.store.apply(
                vec![
                    change(job.chat, &key, &manual)?,
                    change(
                        job.chat,
                        user_key,
                        &Limit {
                            created_at: self.now(),
                            count: 1,
                        },
                    )?,
                    change(job.chat, "jev_limit:group", &group_limit)?,
                ],
                &[],
                self.now(),
            )?;
            manual
        };
        if manual.evidence.is_none() {
            manual.evidence = match input {
                Input::Text(text) => Some(
                    json!({"context":"group_message","user":null,"via_bot":null,"message":{"text":text}}),
                ),
                Input::Identity(id) => self.manual_identity(job.chat, id).await?,
            };
            if manual.evidence.is_none() {
                return self.manual_done(job, manual, format!(
                    "*Jev 手动判断 / Manual Jev assessment*\n{}",
                    escape("Bot 无法读取该 ID 的用户或 Bot 资料，未调用模型。\nThe bot cannot read this user's or bot's profile; no model call was made."),
                ));
            }
            self.store.put(job.chat, &key, &manual)?;
        }
        let attempt = self.store.start_usage(job.chat, self.now())?;
        let response = self
            .services
            .classify(
                manual
                    .evidence
                    .as_ref()
                    .ok_or(Error::external("manual_evidence_missing", true))?,
            )
            .await?;
        self.store
            .finish_usage(job.chat, &attempt, &response.usage)?;
        let (p, model) = response.decision?;
        // Keep the same probability validation as automatic moderation.
        evidence::decision(p, manual.review, manual.ban)?;
        let target = match parse_input(message["text"].as_str().unwrap_or("")) {
            Some(Input::Identity(id)) => {
                format!("\n*对象 ID / Target ID*：{}", code(&id.to_string()))
            }
            _ => String::new(),
        };
        let text = format!(
            "{}{target}\n*请求者 ID / Requested by*：{}\n{}",
            assessment(&model, Some(p), manual.review, manual.ban),
            code(&manual.user.to_string()),
            escape("仅供参考，不执行处罚。 / Assessment only; no moderation action."),
        );
        self.manual_done(job, manual, text)
    }
    pub(crate) fn manual_jev_failed(&self, job: &Job) -> Result<()> {
        let manual = self
            .store
            .get::<Manual>(job.chat, &format!("manual_jev:{}", job.id))?;
        if !self.config.is_verbose(job.chat) {
            return Ok(());
        }
        let text = format!(
            "{}\n{}",
            assessment(
                &self.config.jev_model,
                None,
                manual.as_ref().map_or(0.6, |m| m.review),
                manual.as_ref().map_or(0.8, |m| m.ban)
            ),
            escape(
                "请求失败，请稍后重试；未执行处罚。\nRequest failed; try again later. No moderation action."
            )
        );
        if let Some(manual) = manual {
            if manual.state != "done" {
                self.manual_done(job, manual, text)?;
            }
        } else {
            self.store
                .enqueue(&self.manual_notice(job, text), self.now())?;
        }
        Ok(())
    }
    pub(crate) async fn verbose_notice(&self, job: &Job) -> Result<()> {
        if !self.config.is_verbose(job.chat) {
            return Ok(());
        }
        let mut body = json!({"chat_id":job.chat,"text":job.payload["text"],"parse_mode":"MarkdownV2","link_preview_options":{"is_disabled":true}});
        if let Some(id) = job.payload["reply"].as_i64().filter(|id| *id > 0) {
            body["reply_parameters"] = json!({"message_id":id,"allow_sending_without_reply":true});
        }
        if let Some(thread) = job.payload["thread"].as_i64().filter(|id| *id > 0) {
            body["message_thread_id"] = json!(thread);
        }
        self.services.telegram("sendMessage", body).await?;
        Ok(())
    }
}
