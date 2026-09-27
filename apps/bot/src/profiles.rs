//! Profile evidence is a per-case snapshot, never a source of punishment IDs.
use crate::{api::Services, engine::Engine, error::Result, evidence};
use serde_json::{Value, json};

pub fn bio(value: &str) -> Value {
    let text = value.chars().take(4096).collect::<String>();
    let mut links = vec![];
    let mut mentions = vec![];
    for (i, c) in text.char_indices() {
        let previous = text[..i].chars().next_back();
        let boundary = previous.is_none_or(|p| !p.is_ascii_alphanumeric() && !"_.@/-".contains(p));
        if !boundary {
            continue;
        }
        let rest = &text[i..];
        if c == '@' {
            let name: String = rest[1..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if (1..=32).contains(&name.len()) && mentions.len() < 32 {
                let name = format!("@{name}");
                if !mentions.contains(&name) {
                    mentions.push(name);
                }
            }
        }
        let lower = rest.to_ascii_lowercase();
        if [
            "https://t.me/",
            "http://t.me/",
            "t.me/",
            "https://telegram.me/",
            "http://telegram.me/",
            "telegram.me/",
            "tg://resolve?",
        ]
        .iter()
        .any(|prefix| lower.starts_with(prefix))
            && links.len() < 32
        {
            let link: String = rest
                .chars()
                .take_while(|c| c.is_ascii_graphic() && !"<>\"'`()[]{}".contains(*c))
                .collect();
            let link = link.trim_end_matches(['.', ',', ';', '!', ':']).to_string();
            if !links.contains(&link) {
                links.push(link);
            }
        }
    }
    json!({"text":text,"telegram_links":links,"mentions":mentions,"truncated":value.chars().count()>4096})
}

pub fn join_snapshot(request: &Value) -> Value {
    let mut profile = json!({"identity":evidence::user(&request["from"]),"observed_at":request["date"],"source":"ChatJoinRequest"});
    if let Some(text) = request["bio"].as_str() {
        profile["bio"] = bio(text);
    }
    profile
}

/// Only `getChat` with numeric, Telegram-supplied IDs; never resolve/fetch BIO links.
/// Missing profile data must not prevent screening the evidence we do have.
pub async fn enrich<S: Services>(e: &Engine<S>, chat: i64, data: &mut Value) -> Result<()> {
    if data["profiles"].is_object() {
        return Ok(());
    }
    let mut profiles = serde_json::Map::new();
    for role in ["user", "via_bot", "guest_bot_caller_user"] {
        let identity = &data[role];
        let Some(id) = identity["id"].as_i64().filter(|id| *id > 0) else {
            continue;
        };
        let mut profile = json!({"identity":evidence::user(identity),"observed_at":e.now(),"lookup":"unavailable"});
        match e.services.telegram("getChat", json!({"chat_id":id})).await {
            Ok(full) if full["id"].as_i64() == Some(id) && full["type"] == "private" => {
                profile["lookup"] = json!("available");
                for key in ["username", "first_name", "last_name"] {
                    if let Some(value) = full[key].as_str() {
                        profile["identity"][key] =
                            json!(value.chars().take(256).collect::<String>());
                    }
                }
                if let Some(text) = full["bio"].as_str() {
                    profile["bio"] = bio(text);
                    profile["bio_source"] = json!("getChat");
                }
            }
            _ => (),
        }
        if role == "user" && data["join_request_profile"].is_object() {
            // Preserve the original application snapshot even if the current BIO changed.
            profile["join_request"] = data["join_request_profile"].clone();
        } else if let Some(latest) = e.store.get::<Value>(chat, &format!("latest:{id}"))?
            && let Some(nonce) = latest["nonce"].as_str()
            && let Some(session) = e.current_session(chat, nonce)?
            && session.claims.user as i64 == id
            && session.created_at >= e.now() - 86400
            && let Some(snapshot) = session.profile
        {
            profile["join_request"] = snapshot;
        }
        profiles.insert(role.into(), profile);
    }
    data["profiles"] = Value::Object(profiles);
    Ok(())
}
