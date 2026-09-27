//! Pages has no backend. Treat all Mini App data as untrusted until Siteverify.
use crate::{
    api::Services,
    engine::Engine,
    error::{Error, Result},
    model::{Job, NewJob, Session, VerificationAttempt},
    store::change,
};
use serde_json::{Value, json};
use verification_protocol::Submission;

impl<S: Services> Engine<S> {
    pub fn verification_keyboard(&self, session: &Session) -> Value {
        let mut url =
            reqwest::Url::parse(&self.config.verification_url).expect("validated config URL");
        url.set_path("/verify");
        url.query_pairs_mut()
            .append_pair("session", &session.claims.nonce)
            .append_pair("chat", &session.claims.chat)
            .append_pair("sitekey", &self.config.turnstile_site_key);
        json!({"keyboard":[[{"text":"人机验证 / Verify","web_app":{"url":url.as_str()}}]],"resize_keyboard":true,"one_time_keyboard":true})
    }

    pub fn receive_verification(&self, message: &Value) -> Result<()> {
        let outcome = (|| -> Result<()> {
            let user = message["from"]["id"]
                .as_u64()
                .filter(|v| *v > 0)
                .ok_or(Error::Config("invalid_sender"))?;
            let message_id = message["message_id"]
                .as_i64()
                .filter(|v| *v > 0)
                .ok_or(Error::Config("invalid_message_id"))?;
            if message["from"]["is_bot"] == true || message["chat"]["type"] != "private" {
                return Err(Error::Config("invalid_verification_context"));
            }
            let data = Submission::parse(message["web_app_data"]["data"].as_str().unwrap_or(""))?;
            let chat: i64 = data
                .chat
                .parse()
                .map_err(|_| Error::Config("invalid_chat"))?;
            if !self.config.chats.contains(&chat) {
                return Err(Error::Config("unmanaged_chat"));
            }
            let mut session = self
                .current_session(chat, &data.session)?
                .ok_or(Error::Config("session_replaced"))?;
            if session.claims.v != 2
                || session.claims.user != user
                || session.claims.chat != data.chat
            {
                return Err(Error::Config("session_not_owned"));
            }
            if matches!(session.state.as_str(), "approved" | "verified") {
                return Ok(());
            }
            if session.state != "pending"
                || self.now() >= session.claims.exp
                || !self.settings(chat)?.captcha
            {
                return Err(Error::Config("session_expired"));
            }
            if message_id <= session.last_submission {
                return Ok(());
            }
            let received_at = message["date"]
                .as_i64()
                .ok_or(Error::Config("invalid_message_date"))?;
            if received_at <= self.now() - 300 || received_at > self.now() + 30 {
                return self.queue(
                    chat,
                    "verification_rejected",
                    &format!("{user}:{message_id}"),
                    json!({"user":user,"id":data.session}),
                );
            }
            if self.now() >= session.window_start + 60 {
                session.window_start = self.now();
                session.submissions = 0;
            }
            if session.submissions >= 10 {
                return self.queue(
                    chat,
                    "verification_rejected",
                    &format!("rate:{user}:{}", session.window_start),
                    json!({"user":user,"id":data.session,"rate_limited":true}),
                );
            }
            session.submissions += 1;
            session.last_submission = message_id;
            let attempt = uuid::Uuid::new_v4().to_string();
            session.verification = Some(VerificationAttempt {
                id: attempt.clone(),
                token: data.token,
                received_at,
            });
            self.store.apply(
                vec![change(chat, format!("session:{}", data.session), &session)?],
                &[NewJob::new(
                    chat,
                    "verify_captcha",
                    &attempt,
                    json!({"id":data.session,"attempt":attempt}),
                    self.now(),
                )],
                self.now(),
            )
        })();
        match outcome {
            Ok(()) => Ok(()),
            Err(Error::Database(error)) => Err(Error::Database(error)),
            Err(_) => {
                if let Some(user) = message["from"]["id"].as_i64().filter(|v| *v > 0)
                    && message["from"]["is_bot"] != true
                    && message["chat"]["type"] == "private"
                {
                    self.queue(
                        0,
                        "verification_rejected",
                        &format!("{user}:{}", message["message_id"]),
                        json!({"user":user}),
                    )?;
                }
                Ok(())
            }
        }
    }

    pub async fn verify_captcha(&self, job: &Job) -> Result<()> {
        let id = job.payload["id"].as_str().unwrap_or("");
        let Some(mut session) = self.current_session(job.chat, id)? else {
            return Ok(());
        };
        if !self.config.chats.contains(&job.chat) {
            session.verification = None;
            return self.store.put(job.chat, &format!("session:{id}"), &session);
        }
        if session.claims.v != 2 || session.state != "pending" {
            return Ok(());
        }
        let Some(attempt) = session
            .verification
            .clone()
            .filter(|a| Some(a.id.as_str()) == job.payload["attempt"].as_str())
        else {
            return Ok(());
        };
        if self.now() >= session.claims.exp
            || self.now() >= attempt.received_at + 300
            || !self.settings(job.chat)?.captcha
        {
            return self.captcha_failed(job, &mut session, "verification_expired");
        }
        let data = match self.services.siteverify(&attempt.token, &attempt.id).await {
            Ok(data) => data,
            Err(error)
                if !error.permanent()
                    && job.attempts < 3
                    && self.now() < attempt.received_at + 290
                    && self.now() < session.claims.exp =>
            {
                return Err(error);
            }
            Err(error) => return self.captcha_failed(job, &mut session, &error.to_string()),
        };
        // internal-error is the documented retryable Siteverify verdict.
        if data["success"] != true
            && data["error-codes"]
                .as_array()
                .is_some_and(|codes| codes.iter().any(|c| c == "internal-error"))
            && job.attempts < 3
        {
            return Err(Error::external("turnstile_internal_error", false));
        }
        let hostname =
            reqwest::Url::parse(&self.config.verification_url).expect("validated config URL");
        if !valid_verdict(&data, hostname.host_str().unwrap_or(""), id) {
            return self.captcha_failed(job, &mut session, "turnstile_rejected");
        }
        // State and expiry must still match after waiting on Cloudflare.
        let Some(current) = self.current_session(job.chat, id)? else {
            return Ok(());
        };
        if current.state != "pending"
            || current.verification.as_ref().map(|a| &a.id) != Some(&attempt.id)
        {
            return Ok(());
        }
        session = current;
        if self.now() >= session.claims.exp
            || self.now() >= attempt.received_at + 300
            || !self.settings(job.chat)?.captcha
        {
            return self.captcha_failed(job, &mut session, "verification_expired");
        }
        session.state = "verified".into();
        session.verification = None;
        self.store.apply(
            vec![change(job.chat, format!("session:{id}"), &session)?],
            &[NewJob::new(
                job.chat,
                "approve",
                id,
                json!({"id":id}),
                self.now(),
            )],
            self.now(),
        )?;
        self.audit(
            job.chat,
            "captcha_verified",
            &session.claims.user.to_string(),
            "",
            None,
        )
    }

    fn captcha_failed(&self, job: &Job, session: &mut Session, reason: &str) -> Result<()> {
        session.verification = None;
        if self.now() >= session.claims.exp {
            session.state = "expired".into();
        }
        self.store.apply(
            vec![change(
                job.chat,
                format!("session:{}", session.claims.nonce),
                session,
            )?],
            &[NewJob::new(
                job.chat,
                "verification_rejected",
                &job.id,
                json!({"user":session.claims.user,"id":session.claims.nonce}),
                self.now(),
            )],
            self.now(),
        )?;
        self.audit(
            job.chat,
            "captcha_rejected",
            &session.claims.user.to_string(),
            reason,
            None,
        )
    }

    pub async fn verification_rejected(&self, job: &Job) -> Result<()> {
        if job.chat != 0 && !self.config.chats.contains(&job.chat) {
            return Ok(());
        }
        let user = job.payload["user"].as_i64().unwrap_or(0);
        if let Some(id) = job.payload["id"].as_str()
            && let Some(session) = self.current_session(job.chat, id)?
            && session.claims.v == 2
            && session.claims.user == user as u64
        {
            if matches!(session.state.as_str(), "approved" | "verified") {
                return Ok(());
            }
            if session.state == "pending"
                && self.now() < session.claims.exp
                && self.settings(job.chat)?.captcha
            {
                let text = if job.payload["rate_limited"] == true {
                    "提交过于频繁，请稍等一分钟后重新验证。\nToo many attempts. Please wait one minute and try again."
                } else {
                    "验证未通过或已过期，请点击下方按钮重试。\nVerification failed or expired. Tap below to try again."
                };
                return self
                    .send(user, text, self.verification_keyboard(&session))
                    .await;
            }
        }
        self.send(
            user,
            "验证链接无效或已过期，请使用最新按钮，或重新申请加入。\nThis link is invalid or expired. Use the latest button or submit a new join request.",
            Value::Null,
        )
        .await
    }
}

pub fn valid_verdict(data: &Value, hostname: &str, session: &str) -> bool {
    data["success"] == true
        && data["hostname"] == hostname
        && data["action"] == "join"
        && data["cdata"] == session
}
