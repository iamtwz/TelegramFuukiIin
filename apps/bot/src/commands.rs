use crate::{
    api::Services,
    engine::Engine,
    error::Result,
    markdown::{bold, code, escape},
};
use serde_json::{Value, json};

pub const MENU: [(&str, &str); 2] = [
    ("ping", "检查在线状态 / Check availability"),
    ("whoami", "查看身份和聊天 ID / Show identity and chat ID"),
];

pub fn name(message: &Value, bot: &str) -> Option<String> {
    let (name, _) = crate::admin::command(message["text"].as_str()?, bot)?;
    MENU.iter().any(|(cmd, _)| *cmd == name).then_some(name)
}

// Keep user-controlled names on one line; escape them when adding formatting.
fn label(value: &Value) -> String {
    value
        .as_str()
        .unwrap_or("")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(128)
        .collect()
}

pub async fn handle<S: Services>(e: &Engine<S>, message: &Value) -> Result<()> {
    let Some(command) = name(message, &e.config.bot_username) else {
        return Ok(());
    };
    let Some(chat) = message["chat"]["id"].as_i64() else {
        return Ok(());
    };
    let chat_type = match message["chat"]["type"].as_str() {
        Some("private") => "私聊 / Private",
        Some("group") => "群组 / Group",
        Some("supergroup") => "超级群 / Supergroup",
        _ => return Ok(()),
    };
    let sender_chat = &message["sender_chat"];
    let sender = &message["from"];
    // sender_chat takes precedence: Telegram may provide a fake bot sender for
    // anonymous administrators. Never present that bot ID as a person's ID.
    if sender_chat.is_null()
        && (sender["is_bot"] == true || sender["id"].as_i64().is_none_or(|id| id <= 0))
    {
        return Ok(());
    }
    let text = if command == "ping" {
        escape("Pong! 🏓")
    } else {
        let identity = if let Some(id) = sender_chat["id"].as_i64() {
            format!(
                "{}\n{}\n{}：{}\n{}：{}",
                escape("当前以群组或频道身份发言，无法获取个人身份。"),
                escape("Posting as a group or channel; personal identity is unavailable."),
                bold("发送身份 / Posting as"),
                escape(&label(&sender_chat["title"])),
                bold("身份 ID / Identity ID"),
                code(&id.to_string())
            )
        } else {
            let nickname = [label(&sender["first_name"]), label(&sender["last_name"])]
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            let username = label(&sender["username"]);
            let username = if username.is_empty() {
                escape("未设置 / Not set")
            } else {
                code(&format!("@{username}"))
            };
            format!(
                "{}：{}\n{}：{}\n{}：{username}",
                bold("用户 ID / User ID"),
                code(&sender["id"].to_string()),
                bold("昵称 / Name"),
                escape(&nickname),
                bold("用户名 / Username")
            )
        };
        format!(
            "{identity}\n{}：{}\n{}：{}",
            bold("聊天 ID / Chat ID"),
            code(&chat.to_string()),
            bold("聊天类型 / Chat type"),
            escape(chat_type)
        )
    };
    let mut reply = json!({
        "chat_id": chat,
        "text": text,
        "parse_mode": "MarkdownV2",
        "link_preview_options": {"is_disabled": true},
    });
    if let Some(id) = message["message_id"].as_i64().filter(|id| *id > 0) {
        reply["reply_parameters"] = json!({"message_id":id,"allow_sending_without_reply":true});
    }
    if let Some(thread) = message["message_thread_id"].as_i64().filter(|id| *id > 0) {
        reply["message_thread_id"] = json!(thread);
    }
    e.services.telegram("sendMessage", reply).await?;
    Ok(())
}
