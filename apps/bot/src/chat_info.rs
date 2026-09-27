//! Group display metadata only. Never cache membership or permission decisions here.
use crate::{api::Services, engine::Engine, error::Result};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Serialize, Deserialize)]
struct CachedTitle {
    title: String,
    fetched_at: i64,
}

pub async fn label<S: Services>(e: &Engine<S>, chat: i64) -> Result<String> {
    if !e.config.chats.contains(&chat) {
        return Ok(chat.to_string());
    }
    let cached = e.store.get::<CachedTitle>(chat, "chat_title")?;
    if let Some(cached) = &cached
        && (0..300).contains(&(e.now() - cached.fetched_at))
    {
        return Ok(format!("{} · {chat}", cached.title));
    }
    match e
        .services
        .telegram("getChat", json!({"chat_id":chat}))
        .await
    {
        Ok(info)
            if info["id"].as_i64() == Some(chat)
                && matches!(info["type"].as_str(), Some("group" | "supergroup")) =>
        {
            if let Some(title) = info["title"].as_str() {
                let title = title
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .chars()
                    .take(128)
                    .collect::<String>();
                if !title.is_empty() {
                    e.store.put(
                        chat,
                        "chat_title",
                        &CachedTitle {
                            title: title.clone(),
                            fetched_at: e.now(),
                        },
                    )?;
                    return Ok(format!("{title} · {chat}"));
                }
            }
        }
        Err(error) => e.logger.error(
            "admin.chat_title_error",
            json!({"chat_id":chat,"error":error.to_string()}),
        ),
        _ => (),
    }
    Ok(cached.map_or_else(
        || format!("群名称暂不可用 · {chat}"),
        |c| format!("{} · {chat}", c.title),
    ))
}
