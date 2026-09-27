//! Untrusted Mini App submission format. Authorization belongs to the local bot.
use serde::{Deserialize, Serialize};
#[derive(Debug, thiserror::Error)]
#[error("invalid_verification_submission")]
pub struct ProtocolError;
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Submission {
    pub v: u8,
    pub session: String,
    pub chat: String,
    pub token: String,
}
impl Submission {
    pub fn parse(raw: &str) -> Result<Self, ProtocolError> {
        if raw.len() > 4096 {
            return Err(ProtocolError);
        }
        let data: Self = serde_json::from_str(raw).map_err(|_| ProtocolError)?;
        let chat = data.chat.parse::<i64>().map_err(|_| ProtocolError)?;
        if data.v != 2
            || data.session.len() != 32
            || !data
                .session
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || !(-9_007_199_254_740_991..0).contains(&chat)
            || chat.to_string() != data.chat
            || data.token.is_empty()
            || data.token.len() > 2048
            || !data.token.bytes().all(|b| (0x21..=0x7e).contains(&b))
        {
            return Err(ProtocolError);
        }
        Ok(data)
    }
}
