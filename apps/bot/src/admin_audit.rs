use crate::{
    admin::button,
    api::Services,
    audit::{Cursor, Filter},
    chat_info,
    engine::Engine,
    error::Result,
    model::{Case, CaseKind},
};
use serde_json::{Value, json};

fn excerpt(text: &str, limit: usize) -> String {
    let mut chars = text.chars();
    let out: String = chars.by_ref().take(limit).collect();
    if chars.next().is_some() {
        format!("{out}…")
    } else {
        out
    }
}

pub async fn show<S: Services>(e: &Engine<S>, chat: i64, user: i64, raw: &str) -> Result<()> {
    let Some(cursor) = Cursor::parse(raw) else {
        return e
            .send(user, "分页参数无效，请重新打开审计记录。", Value::Null)
            .await;
    };
    let page = e.store.audit_page(chat, cursor)?;
    let group = chat_info::label(e, chat).await?;
    let mut text = format!(
        "审计记录\n{group}\n类型：{} · 第 {}/{} 页 · 共 {} 条\n时间：UTC",
        cursor.filter.label(),
        page.cursor.page + 1,
        page.pages,
        page.total
    );
    let mut rows = vec![];
    for entry in page.entries {
        text.push_str(&format!(
            "\n\n#{} {} · {}\n目标：{}\n操作人：{}",
            entry.id,
            entry.time,
            excerpt(entry.label(), 60),
            excerpt(&entry.target, 60),
            entry
                .actor
                .map_or_else(|| "系统".into(), |id| id.to_string())
        ));
        if !entry.detail.is_empty() {
            text.push_str(&format!("\n{}", excerpt(&entry.detail, 120)));
        }
        let label = if entry.case_id().is_some() {
            "查看原消息"
        } else {
            "查看详情"
        };
        rows.push(json!([button(
            chat,
            &format!("#{} · {label}", entry.id),
            "auditentry",
            &entry.id.to_string()
        )]));
    }
    if page.total == 0 {
        text.push_str("\n\n暂无此类型的审计记录。");
    }
    let mut navigation = vec![];
    if page.cursor.page > 0 {
        navigation.push(button(
            chat,
            "上一页",
            "audit",
            &Cursor {
                page: page.cursor.page - 1,
                ..page.cursor
            }
            .encode(),
        ));
    }
    if page.cursor.page + 1 < page.pages {
        navigation.push(button(
            chat,
            "下一页",
            "audit",
            &Cursor {
                page: page.cursor.page + 1,
                ..page.cursor
            }
            .encode(),
        ));
    }
    if !navigation.is_empty() {
        rows.push(json!(navigation));
    }
    for filters in Filter::ALL.chunks(3) {
        rows.push(json!(
            filters
                .iter()
                .map(|filter| {
                    let label = format!(
                        "{}{}",
                        if *filter == cursor.filter { "✓ " } else { "" },
                        filter.label()
                    );
                    button(
                        chat,
                        &label,
                        "audit",
                        &Cursor {
                            filter: *filter,
                            snapshot: page.cursor.snapshot,
                            page: 0,
                        }
                        .encode(),
                    )
                })
                .collect::<Vec<_>>()
        ));
    }
    rows.push(json!([
        button(
            chat,
            "刷新",
            "audit",
            &Cursor {
                filter: cursor.filter,
                ..Cursor::default()
            }
            .encode()
        ),
        button(chat, "管理面板", "panel", "")
    ]));
    e.send(user, &text, json!({"inline_keyboard":rows})).await
}

/// Split without dropping evidence or exceeding Telegram's UTF-16 text limit.
async fn send_parts<S: Services>(
    e: &Engine<S>,
    user: i64,
    heading: &str,
    text: &str,
) -> Result<()> {
    let chars = text.chars().collect::<Vec<_>>();
    let parts = chars.chunks(1200);
    let total = parts.len();
    for (i, part) in parts.enumerate() {
        e.send(
            user,
            &format!(
                "{heading}\n（{}/{total}）\n{}",
                i + 1,
                part.iter().collect::<String>()
            ),
            Value::Null,
        )
        .await?;
    }
    Ok(())
}

fn identity(value: &Value) -> String {
    let name = [value["first_name"].as_str(), value["last_name"].as_str()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "{name} · @{} · ID {}",
        value["username"].as_str().unwrap_or("—"),
        value["id"]
    )
}
fn snapshot(evidence: &Value) -> Result<String> {
    let mut out = format!("发送者：{}", identity(&evidence["user"]));
    if evidence["via_bot"].is_object() {
        out.push_str(&format!("\n通过机器人：{}", identity(&evidence["via_bot"])));
    }
    if evidence["guest_bot_caller_user"].is_object() {
        out.push_str(&format!(
            "\nGuest Bot 召唤者：{}",
            identity(&evidence["guest_bot_caller_user"])
        ));
    }
    for (key, label) in [
        ("guest_bot_caller_chat", "召唤频道/群身份"),
        ("join_request_profile", "入群申请时资料"),
        ("profiles", "送审资料与 BIO 快照（含链接和 @用户名）"),
    ] {
        if evidence[key].is_object() {
            out.push_str(&format!(
                "\n\n{label}：\n{}",
                serde_json::to_string_pretty(&evidence[key])?
            ));
        }
    }
    for (key, label) in [("text", "正文"), ("caption", "附件说明")] {
        if let Some(text) = evidence["message"][key].as_str() {
            out.push_str(&format!("\n\n{label}：\n{text}"));
        }
    }
    if let Some(rows) = evidence["message"]["reply_markup"]["inline_keyboard"].as_array() {
        out.push_str("\n\n消息按钮（仅展示内容）：");
        for row in rows {
            if let Some(buttons) = row.as_array() {
                for button in buttons {
                    out.push_str(&format!("\n{}", serde_json::to_string(button)?));
                }
            }
        }
    }
    // Retain all other captured fields, including hidden text links and media metadata.
    let mut extra = evidence["message"].clone();
    if let Some(extra) = extra.as_object_mut() {
        extra.remove("text");
        extra.remove("caption");
        if let Some(markup) = extra.get_mut("reply_markup").and_then(Value::as_object_mut) {
            markup.remove("inline_keyboard");
            if markup.is_empty() {
                extra.remove("reply_markup");
            }
        }
        if !extra.is_empty() {
            out.push_str(&format!(
                "\n\n其他已保存信息：\n{}",
                serde_json::to_string_pretty(extra)?
            ));
        }
    }
    Ok(out)
}

pub async fn detail<S: Services>(e: &Engine<S>, chat: i64, user: i64, raw: &str) -> Result<()> {
    let entry = match raw.parse::<i64>().ok().filter(|id| *id > 0) {
        Some(id) => e.store.audit_entry(chat, id)?,
        None => None,
    };
    let Some(entry) = entry else {
        return e
            .send(user, "记录不存在或已过保留期限。", Value::Null)
            .await;
    };
    let group = chat_info::label(e, chat).await?;
    let heading = format!(
        "审计 #{} · {}\n{group}",
        entry.id,
        excerpt(entry.label(), 60)
    );
    let text = format!(
        "时间：{} UTC\n目标：{}\n操作人：{}\n详情：{}",
        entry.time,
        entry.target,
        entry
            .actor
            .map_or_else(|| "系统".into(), |id| id.to_string()),
        if entry.detail.is_empty() {
            "—"
        } else {
            &entry.detail
        }
    );
    send_parts(e, user, &heading, &text).await?;
    let Some(id) = entry.case_id() else {
        return Ok(());
    };
    let Some(case) = e.store.get::<Case>(chat, &format!("case:{id}"))? else {
        return e
            .send(
                user,
                "关联案件已过保留期限，原消息快照不可用。",
                Value::Null,
            )
            .await;
    };
    let probability = case
        .probability
        .map_or_else(|| "未取得评分".into(), |p| format!("{:.1}%", p * 100.0));
    let text = format!(
        "案件：{}\n用户 ID：{}\n垃圾消息概率：{probability}\n原消息快照：检测时保存的正文、按钮和可见信息，不包含媒体文件。",
        case.id, case.user
    );
    let markup = if case.kind == CaseKind::JoinProfile {
        Value::Null
    } else {
        chat.to_string().strip_prefix("-100").map_or(Value::Null, |id| json!({"inline_keyboard":[[{"text":"在群中打开（已删消息可能无法打开）","url":format!("https://t.me/c/{id}/{}", case.message)}]]}))
    };
    e.send(user, &text, markup).await?;
    match case.evidence {
        Some(evidence) => {
            send_parts(
                e,
                user,
                &format!("原消息快照 · 审计 #{}", entry.id),
                &snapshot(&evidence)?,
            )
            .await
        }
        None => {
            e.send(
                user,
                "原消息快照已按保留策略清理；审计记录仍可查看。",
                Value::Null,
            )
            .await
        }
    }
}
