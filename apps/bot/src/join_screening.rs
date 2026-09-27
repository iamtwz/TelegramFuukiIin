//! Admission requires both a successful captcha and an allowed profile case.
use crate::{
    api::{Services, is_admin},
    engine::Engine,
    error::Result,
    evidence,
    model::{Case, CaseKind, NewJob, Session, Settings},
    store::change,
};
use serde_json::{Value, json};

pub fn profile_case(s: &Session, request: &Value, settings: &Settings, now: i64) -> Case {
    let mut data =
        json!({"context":"join_profile","user":evidence::user(&request["from"]),"message":{}});
    data["join_request_profile"] = crate::profiles::join_snapshot(request);
    Case {
        id: s.profile_case.clone().expect("profile case assigned"),
        kind: CaseKind::JoinProfile,
        guest_caller: None,
        join_session: Some(s.claims.nonce.clone()),
        cycle: String::new(),
        user: s.claims.user as i64,
        message: 0,
        joined_at: s.requested_at,
        created_at: now,
        evidence: Some(data),
        state: "classifying".into(),
        policy: evidence::POLICY.into(),
        screening: settings.screening,
        review_threshold: settings.review,
        ban_threshold: settings.ban,
        probability: None,
        model: None,
        actor: None,
        deleted: true,
        banned: false,
        reason: None,
    }
}

pub fn approval_job(c: &Case, chat: i64, now: i64, source: &str) -> Option<NewJob> {
    c.join_session.as_ref().map(|id| {
        NewJob::new(
            chat,
            "approve",
            &format!("{source}:{}", c.id),
            json!({"id":id}),
            now,
        )
    })
}

impl<S: Services> Engine<S> {
    pub(crate) fn active_join_case(&self, chat: i64, c: &Case) -> Result<Option<Session>> {
        let Some(id) = &c.join_session else {
            return Ok(None);
        };
        Ok(self.current_session(chat, id)?.filter(|s| {
            s.profile_case.as_deref() == Some(&c.id)
                && s.claims.user as i64 == c.user
                && self.now() < s.claims.exp
                && matches!(
                    s.state.as_str(),
                    "pending" | "verified" | "manual" | "rejected"
                )
        }))
    }
    pub(crate) async fn enforce_join_profile(&self, chat: i64, mut c: Case) -> Result<()> {
        let Some(mut s) = self.active_join_case(chat, &c)? else {
            c.state = "expired".into();
            return self.store.put(chat, &format!("case:{}", c.id), &c);
        };
        let member = self.member(chat, c.user).await?;
        if self.active_join_case(chat, &c)?.is_none() {
            c.state = "expired".into();
            return self.store.put(chat, &format!("case:{}", c.id), &c);
        }
        if is_admin(&member) {
            c.state = "protected".into();
            let jobs = approval_job(&c, chat, self.now(), "protected")
                .into_iter()
                .collect::<Vec<_>>();
            self.store.apply(
                vec![change(chat, format!("case:{}", c.id), &c)?],
                &jobs,
                self.now(),
            )?;
            return self.audit(chat, "admin_protected", &c.id, "join_profile", None);
        }
        // Close the local approval gate before making an external moderation call.
        s.state = "rejected".into();
        s.verification = None;
        self.store
            .put(chat, &format!("session:{}", s.claims.nonce), &s)?;
        if !c.banned {
            if member["status"] != "kicked" {
                self.services
                    .telegram(
                        "banChatMember",
                        json!({"chat_id":chat,"user_id":c.user,"revoke_messages":true}),
                    )
                    .await?;
            }
            c.banned = true;
            self.store.put(chat, &format!("case:{}", c.id), &c)?;
            self.audit(chat, "user_banned", &c.id, "join_profile", None)?;
        }
        c.state = "banned".into();
        self.store.put(chat, &format!("case:{}", c.id), &c)
    }
}
