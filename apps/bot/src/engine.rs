use crate::{
    api::{Services, is_admin, present},
    config::Config,
    error::{Error, Result},
    evidence,
    logging::{LogLevel, Logger},
    model::{
        Case, CaseKind, Claims, FirstMessage, Job, Member, NewJob, ScreeningMode, Session, Settings,
    },
    store::{Store, change},
};
use serde_json::{Value, json};
use std::{sync::Arc, time::Instant};

pub struct Engine<S: Services> {
    pub store: Arc<Store>,
    pub config: Arc<Config>,
    pub services: Arc<S>,
    pub clock: Arc<dyn Fn() -> i64 + Send + Sync>,
    pub logger: Arc<Logger>,
}
impl<S: Services> Engine<S> {
    pub fn now(&self) -> i64 {
        (self.clock)()
    }
    pub fn settings(&self, chat: i64) -> Result<Settings> {
        Ok(self.store.get(chat, "settings")?.unwrap_or_default())
    }
    pub fn audit(
        &self,
        chat: i64,
        action: &str,
        target: &str,
        detail: &str,
        actor: Option<i64>,
    ) -> Result<()> {
        self.store
            .audit(chat, self.now(), action, actor, target, detail)?;
        self.logger.event(
            LogLevel::Verbose,
            "audit",
            json!({"chat_id":chat,"action":action,"actor":actor,"target":target,"detail":detail}),
        );
        Ok(())
    }
    pub fn queue(&self, chat: i64, kind: &str, id: &str, payload: Value) -> Result<()> {
        self.store.enqueue(
            &NewJob::new(chat, kind, id, payload, self.now()),
            self.now(),
        )
    }
    pub async fn member(&self, chat: i64, user: i64) -> Result<Value> {
        self.services
            .telegram("getChatMember", json!({"chat_id":chat,"user_id":user}))
            .await
    }
    pub async fn send(&self, chat: i64, text: &str, markup: Value) -> Result<()> {
        self.send_markdown(
            chat,
            &crate::markdown::escape(&crate::markdown::truncate(text, 3900)),
            markup,
        )
        .await
    }
    /// Send complete MarkdownV2. Callers bound or split raw content before formatting.
    pub async fn send_markdown(&self, chat: i64, text: &str, markup: Value) -> Result<()> {
        let mut data = json!({"chat_id":chat,"text":text,"parse_mode":"MarkdownV2","link_preview_options":{"is_disabled":true}});
        if !markup.is_null() {
            data["reply_markup"] = markup;
        }
        self.services.telegram("sendMessage", data).await?;
        Ok(())
    }
    pub fn joined(&self, chat: i64, user: i64, date: i64, definite: bool) -> Result<()> {
        let key = format!("member:{user}");
        if let Some(mut old) = self.store.get::<Member>(chat, &key)? {
            if date < old.joined_at {
                return Ok(());
            }
            if old.active && old.provisional && definite {
                old.provisional = false;
                return self.store.put(chat, &key, &old);
            }
            if old.active && (date == old.joined_at || !definite) {
                return Ok(());
            }
        }
        self.store.put(
            chat,
            &key,
            &Member {
                user,
                cycle: uuid::Uuid::new_v4().simple().to_string(),
                joined_at: date,
                active: true,
                first_message: None,
                provisional: !definite,
            },
        )?;
        self.audit(chat, "member_joined", &user.to_string(), "", None)
    }
    fn left(&self, chat: i64, user: i64, date: i64) -> Result<()> {
        let key = format!("member:{user}");
        if let Some(mut old) = self.store.get::<Member>(chat, &key)?
            && date >= old.joined_at
        {
            old.active = false;
            self.store.put(chat, &key, &old)?;
        }
        Ok(())
    }
    pub fn ingest(&self, update: &Value) -> Result<()> {
        let uid = update["update_id"]
            .as_i64()
            .ok_or(Error::Config("invalid_update"))?;
        let kind = [
            "message",
            "edited_message",
            "chat_member",
            "chat_join_request",
            "callback_query",
        ]
        .into_iter()
        .find(|key| update[*key].is_object())
        .unwrap_or("other");
        let source = &update[kind];
        self.logger.io("telegram.update", json!({
            "update_id":uid,"kind":kind,
            "chat_id":source["chat"]["id"].as_i64().or_else(|| source["message"]["chat"]["id"].as_i64()),
            "user_id":source["from"]["id"].as_i64(),
        }), update);
        let message = &update["message"];
        if message["chat"]["type"] == "private" && message["web_app_data"].is_object() {
            return self.receive_verification(message);
        }
        if let Some(chat) = message["chat"]["id"].as_i64()
            && self.config.is_verbose(chat)
            && crate::verbose::is_command(message, &self.config.bot_username)
        {
            self.queue(
                chat,
                "manual_jev",
                &uid.to_string(),
                json!({"message":message}),
            )?;
            return Ok(());
        }
        if crate::commands::name(message, &self.config.bot_username).is_some() {
            if message["chat"]["type"] == "private" {
                self.queue(0, "command", &uid.to_string(), message.clone())?;
            } else if let Some(chat) = message["chat"]["id"].as_i64()
                && matches!(
                    message["chat"]["type"].as_str(),
                    Some("group" | "supergroup")
                )
            {
                // Discovery commands work before a group is configured. Only
                // managed groups consume first messages or trigger moderation.
                if self.config.chats.contains(&chat) {
                    self.message(chat, message)?;
                }
                self.queue(chat, "command", &uid.to_string(), message.clone())?;
            }
            return Ok(());
        }
        if update["callback_query"].is_object() || message["chat"]["type"] == "private" {
            let chat = crate::admin::target_chat(update, &self.config).unwrap_or(0);
            if chat == 0 || self.config.chats.contains(&chat) {
                self.queue(chat, "admin", &uid.to_string(), update.clone())?;
            }
            return Ok(());
        }
        if let Some(chat) = update["chat_join_request"]["chat"]["id"].as_i64() {
            if self.config.chats.contains(&chat) {
                self.join_request(chat, &update["chat_join_request"], uid)?;
            }
            return Ok(());
        }
        let changed = &update["chat_member"];
        if let (Some(chat), Some(user), Some(date)) = (
            changed["chat"]["id"].as_i64(),
            changed["new_chat_member"]["user"]["id"].as_i64(),
            changed["date"].as_i64(),
        ) && self.config.chats.contains(&chat)
            && self.recent_telegram_event(chat, "chat_member", date)
        {
            if is_admin(&changed["old_chat_member"]) != is_admin(&changed["new_chat_member"]) {
                self.queue(0, "menus", &uid.to_string(), json!({}))?;
            }
            if !present(&changed["old_chat_member"]) && present(&changed["new_chat_member"]) {
                self.joined(chat, user, date, true)?;
            } else if !present(&changed["new_chat_member"]) {
                self.left(chat, user, date)?;
            }
        }
        if let Some(chat) = message["chat"]["id"].as_i64()
            && self.config.chats.contains(&chat)
        {
            self.message(chat, message)?;
        }
        Ok(())
    }
    /// These dates came from authenticated Telegram updates, not Mini App input.
    /// Local clock lag must not silently acknowledge and discard a real message.
    fn recent_telegram_event(&self, chat: i64, kind: &str, date: i64) -> bool {
        let now = self.now();
        if date <= 0 || date <= now.saturating_sub(604800) {
            return false;
        }
        if date > now.saturating_add(30) {
            self.logger.error("telegram.clock_skew",json!({"chat_id":chat,"kind":kind,"telegram_date":date,"local_date":now,"ahead_seconds":date.saturating_sub(now)}));
        }
        true
    }
    pub(crate) fn screening_skip(&self, chat: i64, m: &Value, reason: &str) {
        self.logger.event(LogLevel::Verbose,"screening.skip",json!({"chat_id":chat,"message_id":m["message_id"].as_i64(),"user_id":m["from"]["id"].as_i64(),"reason":reason}));
    }
    pub fn message(&self, chat: i64, m: &Value) -> Result<()> {
        let date = m["date"].as_i64().unwrap_or(0);
        if !self.config.chats.contains(&chat) {
            return Ok(());
        }
        if !self.recent_telegram_event(chat, "message", date) {
            self.screening_skip(chat, m, "invalid_or_stale_date");
            return Ok(());
        }
        if let Some(users) = m["new_chat_members"].as_array() {
            for user in users {
                if let Some(id) = user["id"].as_i64() {
                    self.joined(chat, id, date, false)?;
                }
            }
            return Ok(());
        }
        if let Some(id) = m["left_chat_member"]["id"].as_i64() {
            return self.left(chat, id, date);
        }
        let Some(user) = m["from"]["id"].as_i64().filter(|id| *id > 0) else {
            self.screening_skip(chat, m, "missing_sender");
            return Ok(());
        };
        if user == self.config.bot_id()
            || !m["sender_chat"].is_null()
            || m["is_automatic_forward"] == true
            || !m["guest_query_id"].is_null()
            || !m["business_connection_id"].is_null()
            || !evidence::has_content(m)
        {
            self.screening_skip(chat, m, "unsupported_context_or_no_content");
            return Ok(());
        }
        if self.guest_message(chat, m)? {
            return Ok(());
        }
        let message = m["message_id"]
            .as_i64()
            .filter(|v| *v > 0)
            .ok_or(Error::Config("invalid_message_id"))?;
        let key = format!("member:{user}");
        let member = self.store.get::<Member>(chat, &key)?;
        let first_after_join = member.as_ref().is_some_and(|member| {
            member.active && member.first_message.is_none() && date >= member.joined_at
        });
        let observation_key = format!("first_message:{user}");
        let first_seen = self
            .store
            .get::<FirstMessage>(chat, &observation_key)?
            .is_none();
        let mut changes = vec![];
        if first_seen {
            changes.push(change(
                chat,
                observation_key,
                &FirstMessage {
                    user,
                    message,
                    created_at: self.now(),
                },
            )?);
        }
        // Maintain both markers even while screening is disabled or the other
        // mode is selected. Switching modes never replays an observed message.
        if first_after_join && let Some(mut updated) = member.clone() {
            updated.first_message = Some(message);
            changes.push(change(chat, key, &updated)?);
        }
        let settings = self.settings(chat)?;
        let should_screen = match settings.screening {
            ScreeningMode::NewMembers => first_after_join,
            ScreeningMode::FirstSeen => first_seen,
        };
        if !settings.spam || !should_screen {
            let reason = if !settings.spam {
                "spam_disabled"
            } else if settings.screening == ScreeningMode::NewMembers
                && !member.as_ref().is_some_and(|m| m.active)
            {
                "no_active_join_observed"
            } else if settings.screening == ScreeningMode::NewMembers
                && member.as_ref().is_some_and(|m| date < m.joined_at)
            {
                "before_observed_join"
            } else {
                "not_first_message"
            };
            self.screening_skip(chat, m, reason);
            let mut jobs = vec![];
            self.track_case_message(chat, user, message, date, &mut changes, &mut jobs)?;
            return if changes.is_empty() {
                Ok(())
            } else {
                self.store.apply(changes, &jobs, self.now())
            };
        }
        let id = uuid::Uuid::new_v4().simple().to_string()[..16].to_string();
        let mut message_evidence = evidence::extract(m);
        if let Some(latest) = self.store.get::<Value>(chat, &format!("latest:{user}"))?
            && let Some(nonce) = latest["nonce"].as_str()
            && let Some(session) = self.current_session(chat, nonce)?
            && session.created_at >= self.now() - 86400
            && let Some(profile) = session.profile
        {
            message_evidence["join_request_profile"] = profile;
        }
        let case = Case {
            id: id.clone(),
            kind: CaseKind::Message,
            guest_caller: None,
            join_session: None,
            // Membership cycles apply only to new-member screening. A first-
            // seen case belongs to the group/user's observation across joins.
            cycle: member
                .as_ref()
                .map_or_else(String::new, |m| m.cycle.clone()),
            user,
            message,
            joined_at: member.as_ref().map_or(0, |m| m.joined_at),
            created_at: self.now(),
            evidence: Some(message_evidence),
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
        changes.push(change(chat, format!("case:{id}"), &case)?);
        changes.push(change(
            chat,
            format!("message_case:{user}"),
            &crate::message_cleanup::MessageCase {
                id: id.clone(),
                date,
                created_at: case.created_at,
                banned_at: None,
            },
        )?);
        self.store.apply(
            changes,
            &[NewJob::new(
                chat,
                "classify",
                &id,
                json!({"id":id}),
                self.now(),
            )],
            self.now(),
        )
    }
    pub fn join_request(&self, chat: i64, r: &Value, update_id: i64) -> Result<()> {
        if !self.config.chats.contains(&chat) {
            return Ok(());
        }
        let user = r["from"]["id"]
            .as_u64()
            .filter(|id| *id > 0 && *id <= i64::MAX as u64)
            .ok_or(Error::Config("invalid_join_user"))?;
        let date = r["date"]
            .as_i64()
            .ok_or(Error::Config("invalid_join_date"))?;
        if !self.recent_telegram_event(chat, "chat_join_request", date) {
            return Ok(());
        }
        // Admission has time-limited credentials. Preserve its strict time gate,
        // but retain the update for retry instead of silently completing it.
        if date > self.now().saturating_add(30) {
            return Err(Error::external("join_request_clock_skew", false));
        }
        self.store
            .statistics_join(chat, user as i64, date, self.now())?;
        let settings = self.settings(chat)?;
        let review_profile = settings.spam && settings.join_profile_review;
        if (!settings.captcha && !review_profile) || date + 600 <= self.now() {
            return Ok(());
        }
        if !r["query_id"].is_null() {
            self.audit(
                chat,
                "guard_mode_unsupported",
                &user.to_string(),
                "Use ordinary admin join requests",
                None,
            )?;
            return self.queue(
                chat,
                "alert",
                &format!("guard:{}", self.now() / 3600),
                json!({}),
            );
        }
        let previous = self.store.get::<Value>(chat, &format!("latest:{user}"))?;
        let old = previous
            .as_ref()
            .and_then(|v| v["nonce"].as_str())
            .map(|n| self.store.get::<Session>(chat, &format!("session:{n}")))
            .transpose()?
            .flatten();
        if old.as_ref().is_some_and(|s| s.update_id >= update_id) {
            return Ok(());
        }
        let claims = Claims {
            v: 2,
            nonce: uuid::Uuid::new_v4().simple().to_string(),
            chat: chat.to_string(),
            user,
            iat: date,
            exp: date + 600,
        };
        let session = Session {
            profile_case: review_profile
                .then(|| uuid::Uuid::new_v4().simple().to_string()[..16].to_string()),
            profile: Some(crate::profiles::join_snapshot(r)),
            claims: claims.clone(),
            verification: None,
            last_submission: 0,
            window_start: 0,
            submissions: 0,
            update_id,
            user_chat: r["user_chat_id"]
                .as_i64()
                .ok_or(Error::Config("missing_user_chat_id"))?,
            requested_at: date,
            created_at: self.now(),
            state: if settings.captcha {
                "pending"
            } else {
                "manual"
            }
            .into(),
        };
        let mut changes = vec![
            change(chat, format!("session:{}", claims.nonce), &session)?,
            change(
                chat,
                format!("latest:{user}"),
                &json!({"nonce":claims.nonce,"created_at":self.now()}),
            )?,
        ];
        if let Some(mut old) = old
            && old.state != "approved"
        {
            old.state = "expired".into();
            old.verification = None;
            changes.push(change(chat, format!("session:{}", old.claims.nonce), &old)?);
        }
        let mut jobs = vec![];
        if settings.captcha {
            jobs.push(NewJob::new(
                chat,
                "invite",
                &claims.nonce,
                json!({"id":claims.nonce}),
                self.now(),
            ));
        }
        if session.profile_case.is_some() {
            let c = crate::join_screening::profile_case(&session, r, &settings, self.now());
            changes.push(change(chat, format!("case:{}", c.id), &c)?);
            jobs.push(NewJob::new(
                chat,
                "classify",
                &c.id,
                json!({"id":c.id}),
                self.now(),
            ));
        }
        self.store.apply(changes, &jobs, self.now())?;
        self.audit(chat, "join_requested", &user.to_string(), "", None)
    }
    pub fn current_session(&self, chat: i64, id: &str) -> Result<Option<Session>> {
        let Some(session) = self.store.get::<Session>(chat, &format!("session:{id}"))? else {
            return Ok(None);
        };
        let latest = self
            .store
            .get::<Value>(chat, &format!("latest:{}", session.claims.user))?;
        if latest.as_ref().and_then(|v| v["nonce"].as_str()) != Some(id) {
            return Ok(None);
        }
        Ok(Some(session))
    }
    async fn invite(&self, chat: i64, id: &str) -> Result<()> {
        let Some(s) = self.current_session(chat, id)? else {
            return Ok(());
        };
        if !self.config.chats.contains(&chat)
            || s.claims.v != 2
            || s.state != "pending"
            || self.now() >= s.claims.exp
            || !self.settings(chat)?.captcha
        {
            return Ok(());
        }
        if self.now() > s.requested_at + 290 {
            return Err(Error::external("join_dm_window_expired", true));
        }
        self.send(
            s.user_chat,
            "请点击下方按钮完成人机验证。\nTap below to verify you’re human.",
            self.verification_keyboard(&s),
        )
        .await
    }

    async fn approve(&self, chat: i64, id: &str) -> Result<()> {
        let Some(mut s) = self.current_session(chat, id)? else {
            return Ok(());
        };
        if !self.config.chats.contains(&chat) || s.claims.v != 2 || s.state != "verified" {
            return Ok(());
        }
        if self.now() >= s.claims.exp {
            s.state = "expired".into();
            self.store.put(chat, &format!("session:{id}"), &s)?;
            return Ok(());
        }
        if !self.settings(chat)?.captcha {
            return Err(Error::external("verification_disabled", true));
        }
        if let Some(case) = &s.profile_case {
            let allowed = self
                .store
                .get::<Case>(chat, &format!("case:{case}"))?
                .is_some_and(|c| {
                    c.kind == CaseKind::JoinProfile
                        && c.join_session.as_deref() == Some(id)
                        && c.user == s.claims.user as i64
                        && matches!(c.state.as_str(), "allowed" | "protected")
                });
            if !allowed {
                return Ok(());
            }
        }
        if let Err(e) = self
            .services
            .telegram(
                "approveChatJoinRequest",
                json!({"chat_id":chat,"user_id":s.claims.user}),
            )
            .await
            && !present(&self.member(chat, s.claims.user as i64).await?)
        {
            return Err(e);
        }
        s.state = "approved".into();
        let key = format!("member:{}", s.claims.user);
        // Preserve an already observed join, including a consumed first message.
        // Otherwise use request time so an approval whose response was lost cannot
        // make an already delivered first message look older than the membership.
        let member = self
            .store
            .get::<Member>(chat, &key)?
            .filter(|m| m.active && m.joined_at >= s.requested_at)
            .unwrap_or(Member {
                user: s.claims.user as i64,
                cycle: uuid::Uuid::new_v4().simple().to_string(),
                joined_at: s.requested_at,
                active: true,
                first_message: None,
                provisional: true,
            });
        self.store.apply(
            vec![
                change(chat, format!("session:{id}"), &s)?,
                change(chat, key, &member)?,
            ],
            &[NewJob::new(
                chat,
                "welcome",
                id,
                json!({"user":s.claims.user}),
                self.now(),
            )],
            self.now(),
        )?;
        self.audit(chat, "join_approved", &s.claims.user.to_string(), "", None)
    }
    async fn classify(&self, chat: i64, id: &str) -> Result<()> {
        let Some(mut c) = self.store.get::<Case>(chat, &format!("case:{id}"))? else {
            return Ok(());
        };
        if c.state != "classifying" {
            return Ok(());
        }
        if c.kind == CaseKind::JoinProfile && self.active_join_case(chat, &c)?.is_none() {
            c.state = "expired".into();
            return self.store.put(chat, &format!("case:{id}"), &c);
        }
        if is_admin(&self.member(chat, c.user).await?) {
            c.state = "protected".into();
            let jobs = crate::join_screening::approval_job(&c, chat, self.now(), "protected")
                .into_iter()
                .collect::<Vec<_>>();
            return self.store.apply(
                vec![change(chat, format!("case:{id}"), &c)?],
                &jobs,
                self.now(),
            );
        }
        let evidence = c
            .evidence
            .as_mut()
            .ok_or(Error::external("evidence_expired", true))?;
        crate::profiles::enrich(self, chat, evidence).await?;
        // Persist the profile snapshot and the model being called. Failures
        // before this point must not produce a group-visible model result.
        c.model = Some(self.config.jev_model.clone());
        self.store.put(chat, &format!("case:{id}"), &c)?;
        let evidence = c
            .evidence
            .as_ref()
            .ok_or(Error::external("evidence_expired", true))?;
        let attempt = self.store.start_usage(chat, self.now())?;
        let response = self.services.classify(evidence).await?;
        self.store.finish_usage(chat, &attempt, &response.usage)?;
        let (p, model) = response.decision?;
        c.state = evidence::decision(p, c.review_threshold, c.ban_threshold)?.into();
        c.probability = Some(p);
        c.model = Some(model.clone());
        c.policy = evidence::POLICY.into();
        let next = match c.state.as_str() {
            "enforcing" => Some("enforce"),
            "review" => Some("notify"),
            _ => None,
        };
        let mut jobs = next
            .map(|k| NewJob::new(chat, k, id, json!({"id":id}), self.now()))
            .into_iter()
            .collect::<Vec<_>>();
        if c.state == "allowed"
            && let Some(job) = crate::join_screening::approval_job(&c, chat, self.now(), "profile")
        {
            jobs.push(job);
        }
        if let Some(notice) = crate::verbose::case_notice(self, &c, chat) {
            jobs.push(notice);
        }
        self.store.apply(
            vec![change(chat, format!("case:{id}"), &c)?],
            &jobs,
            self.now(),
        )?;
        self.audit(
            chat,
            "classified",
            id,
            &format!("p={p};model={model};policy={}", c.policy),
            None,
        )
    }
    async fn enforce(&self, chat: i64, id: &str) -> Result<()> {
        let Some(mut c) = self.store.get::<Case>(chat, &format!("case:{id}"))? else {
            return Ok(());
        };
        if c.state != "enforcing" {
            return Ok(());
        }
        if c.kind == CaseKind::Guest {
            return self.enforce_guest(chat, c).await;
        }
        if c.kind == CaseKind::JoinProfile {
            return self.enforce_join_profile(chat, c).await;
        }
        if !self.current_message_case(chat, &c)? {
            c.state = "expired".into();
            return self.store.put(chat, &format!("case:{id}"), &c);
        }
        let target = self.member(chat, c.user).await?;
        if is_admin(&target) {
            c.state = "protected".into();
            self.audit(chat, "admin_protected", id, "", None)?;
            return self.store.put(chat, &format!("case:{id}"), &c);
        }
        if !c.banned && target["status"] == "kicked" {
            c.banned = true;
            self.store.put(chat, &format!("case:{id}"), &c)?;
            self.audit(chat, "ban_confirmed", id, "already_banned", None)?;
        }
        let mut failure = None;
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
                    self.store.put(chat, &format!("case:{id}"), &c)?;
                    self.audit(
                        chat,
                        "message_deleted",
                        id,
                        &format!("message_id={};scope=first", c.message),
                        None,
                    )?;
                }
                Err(e) => failure = Some(e),
            }
        }
        if !c.banned {
            match self
                .services
                .telegram(
                    "banChatMember",
                    json!({"chat_id":chat,"user_id":c.user,"revoke_messages":true}),
                )
                .await
            {
                Ok(_) => {
                    c.banned = true;
                    self.store.put(chat, &format!("case:{id}"), &c)?;
                    self.audit(chat, "user_banned", id, "", None)?;
                }
                Err(e) => failure = Some(e),
            }
        }
        if c.banned {
            self.queue_case_messages(chat, &c)?;
        }
        if let Some(e) = failure {
            return Err(e);
        }
        c.state = "banned".into();
        self.store.put(chat, &format!("case:{id}"), &c)
    }
    pub async fn execute(&self, job: &Job) -> Result<()> {
        // Removing a group (including clearing the allowlist) must also stop
        // moderation queued before the configuration change.
        if matches!(
            job.kind.as_str(),
            "classify"
                | "enforce"
                | "cleanup_message"
                | "notify"
                | "welcome"
                | "alert"
                | "admin_notice"
                | "manual_jev"
                | "verbose_notice"
        ) && !self.config.chats.contains(&job.chat)
        {
            if job.kind == "manual_jev" {
                self.discard_manual_jev(job)?;
            }
            return Ok(());
        }
        let id = job.payload["id"].as_str().unwrap_or("");
        match job.kind.as_str() {
            "update" => self.ingest(&job.payload),
            "admin" => crate::admin::handle(self, job.chat, &job.payload).await,
            "command" => crate::commands::handle(self, &job.payload).await,
            "invite" => self.invite(job.chat, id).await,
            "approve" => self.approve(job.chat, id).await,
            "classify" => self.classify(job.chat, id).await,
            "manual_jev" => self.manual_jev(job).await,
            "verbose_notice" => self.verbose_notice(job).await,
            "enforce" => self.enforce(job.chat, id).await,
            "cleanup_message" => self.cleanup_message(job).await,
            "welcome" => {
                self.send(
                    job.payload["user"].as_i64().unwrap_or(0),
                    "验证通过，入群申请已批准。\nVerified. Your join request has been approved.",
                    json!({"remove_keyboard":true}),
                )
                .await
            }
            "verify_captcha" => self.verify_captcha(job).await,
            "verification_rejected" => self.verification_rejected(job).await,
            "notify" | "alert" => crate::notifications::enqueue(self, job).await,
            "admin_notice" => crate::notifications::deliver(self, job).await,
            "menus" => crate::menus::refresh(self).await,
            _ => Err(Error::external("unknown_job", true)),
        }
    }
    pub async fn run(&self, job: &Job) -> Result<()> {
        let started = Instant::now();
        self.logger.event(
            LogLevel::Verbose,
            "job.start",
            json!({"job_id":job.id,"kind":job.kind,"chat_id":job.chat,"attempt":job.attempts}),
        );
        match self.execute(job).await {
            Ok(()) => {
                self.store.complete(&job.id)?;
                self.logger.event(
                    LogLevel::Verbose,
                    "job.complete",
                    json!({"job_id":job.id,"elapsed_ms":started.elapsed().as_millis()}),
                );
                Ok(())
            }
            Err(Error::Database(e)) => {
                self.logger.error(
                    "job.error",
                    json!({"job_id":job.id,"error":"database_error"}),
                );
                Err(Error::Database(e))
            }
            Err(error) => {
                let exhausted = error.permanent()
                    || job.attempts
                        >= if matches!(job.kind.as_str(), "classify" | "manual_jev") {
                            3
                        } else {
                            6
                        };
                let delay = (1u64 << job.attempts.min(8))
                    .min(300)
                    .max(error.retry_after());
                self.logger.error("job.error", json!({
                    "job_id":job.id,"kind":job.kind,"chat_id":job.chat,"attempt":job.attempts,
                    "error":error.to_string(),"exhausted":exhausted,"elapsed_ms":started.elapsed().as_millis(),
                    "retry_after_seconds":if exhausted { None } else { Some(delay) },
                }));
                if exhausted && job.kind == "manual_jev" {
                    self.manual_jev_failed(job)?;
                    return self.store.complete(&job.id);
                }
                if exhausted && job.kind == "classify" {
                    let id = job.payload["id"].as_str().unwrap_or("");
                    if let Some(mut c) = self.store.get::<Case>(job.chat, &format!("case:{id}"))?
                        && c.state == "classifying"
                    {
                        c.state = "review".into();
                        c.reason = Some(error.to_string());
                        let mut jobs = vec![NewJob::new(
                            job.chat,
                            "notify",
                            id,
                            json!({"id":id}),
                            self.now(),
                        )];
                        if let Some(notice) = crate::verbose::case_notice(self, &c, job.chat) {
                            jobs.push(notice);
                        }
                        self.store.apply(
                            vec![change(job.chat, format!("case:{id}"), &c)?],
                            &jobs,
                            self.now(),
                        )?;
                        self.audit(
                            job.chat,
                            "classification_failed_review",
                            id,
                            &error.to_string(),
                            None,
                        )?;
                    }
                    return self.store.complete(&job.id);
                }
                self.store.fail(
                    &job.id,
                    &error.to_string(),
                    self.now() + delay as i64,
                    exhausted,
                )?;
                if exhausted {
                    self.audit(job.chat, "job_failed", &job.id, &error.to_string(), None)?;
                    if job.chat != 0
                        && !matches!(
                            job.kind.as_str(),
                            "alert" | "admin_notice" | "menus" | "verbose_notice"
                        )
                    {
                        self.queue(
                            job.chat,
                            "alert",
                            &(self.now() / 3600).to_string(),
                            json!({}),
                        )?;
                    }
                }
                Ok(())
            }
        }
    }
}
