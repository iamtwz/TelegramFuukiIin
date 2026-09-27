use serde::{Deserialize, Serialize};
use serde_json::Value;
// Local database binding; v1 records remain readable during migration.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Claims {
    pub v: u8,
    pub nonce: String,
    pub chat: String,
    pub user: u64,
    pub iat: i64,
    pub exp: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VerificationAttempt {
    pub id: String,
    pub token: String,
    pub received_at: i64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScreeningMode {
    #[default]
    NewMembers,
    FirstSeen,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Settings {
    pub spam: bool,
    pub captcha: bool,
    #[serde(default)]
    pub join_profile_review: bool,
    pub review: f64,
    pub ban: f64,
    #[serde(default)]
    pub screening: ScreeningMode,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            spam: true,
            captcha: true,
            join_profile_review: false,
            review: 0.6,
            ban: 0.8,
            screening: ScreeningMode::NewMembers,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Member {
    pub user: i64,
    pub cycle: String,
    pub joined_at: i64,
    pub active: bool,
    #[serde(default)]
    pub first_message: Option<i64>,
    #[serde(default)]
    pub provisional: bool,
}
/// A lifetime observation in one group, independent of membership and threads.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FirstMessage {
    pub user: i64,
    pub message: i64,
    pub created_at: i64,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaseKind {
    #[default]
    Message,
    Guest,
    JoinProfile,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GuestCaller {
    pub user: i64,
    pub cycle: Option<String>,
    pub message: Option<i64>,
    pub banned: bool,
    pub deleted: bool,
    pub protected: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Case {
    pub id: String,
    #[serde(default)]
    pub kind: CaseKind,
    #[serde(default)]
    pub guest_caller: Option<GuestCaller>,
    #[serde(default)]
    pub join_session: Option<String>,
    pub cycle: String,
    pub user: i64,
    pub message: i64,
    pub joined_at: i64,
    pub created_at: i64,
    pub evidence: Option<Value>,
    pub state: String,
    pub policy: String,
    #[serde(default)]
    pub screening: ScreeningMode,
    pub review_threshold: f64,
    pub ban_threshold: f64,
    #[serde(default)]
    pub probability: Option<f64>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub actor: Option<i64>,
    #[serde(default)]
    pub deleted: bool,
    #[serde(default)]
    pub banned: bool,
    #[serde(default)]
    pub reason: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Session {
    pub claims: Claims,
    #[serde(default)]
    pub verification: Option<VerificationAttempt>,
    #[serde(default)]
    pub last_submission: i64,
    #[serde(default)]
    pub window_start: i64,
    #[serde(default)]
    pub submissions: u32,
    pub update_id: i64,
    pub user_chat: i64,
    pub requested_at: i64,
    pub created_at: i64,
    pub state: String,
    #[serde(default)]
    pub profile_case: Option<String>,
    #[serde(default)]
    pub profile: Option<Value>,
}
#[derive(Clone, Debug)]
pub struct Job {
    pub id: String,
    pub chat: i64,
    pub kind: String,
    pub payload: Value,
    pub attempts: u32,
    pub created_at: i64,
}
#[derive(Clone, Debug)]
pub struct NewJob {
    pub id: String,
    pub chat: i64,
    pub kind: String,
    pub payload: Value,
    pub due: i64,
}
impl NewJob {
    pub fn new(chat: i64, kind: &str, id: &str, payload: Value, due: i64) -> Self {
        Self {
            id: format!("{chat}:{kind}:{id}"),
            chat,
            kind: kind.into(),
            payload,
            due,
        }
    }
}
