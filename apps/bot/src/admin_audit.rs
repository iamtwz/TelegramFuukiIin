use crate::{
    admin::button,
    admin_view::{AdminReply, text_pages},
    api::Services,
    audit::{Cursor, Filter},
    chat_info,
    engine::Engine,
    error::Result,
    markdown::{bold, code, escape, pre},
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

pub async fn show<S: Services>(
    e: &Engine<S>,
    chat: i64,
    reply: AdminReply,
    raw: &str,
) -> Result<()> {
    let Some(cursor) = Cursor::parse(raw) else {
        return reply
            .send(
                e,
                "分页参数无效，请重新打开审计记录。",
                detail_navigation(chat),
            )
            .await;
    };
    let page = e.store.audit_page(chat, cursor)?;
    let group = chat_info::markdown_label(e, chat).await?;
    let mut text = format!(
        "{}\n{group}\n类型：{} · 第 {}/{} 页 · 共 {} 条\n时间：{}",
        bold("审计记录"),
        escape(cursor.filter.label()),
        page.cursor.page + 1,
        page.pages,
        page.total,
        code("UTC")
    );
    let mut rows = vec![];
    for entry in page.entries {
        text.push_str(&format!(
            "\n\n{} {} · {}\n目标：{}\n操作人：{}",
            code(&format!("#{}", entry.id)),
            code(&entry.time),
            bold(&excerpt(entry.label(), 60)),
            code(&excerpt(&entry.target, 60)),
            entry
                .actor
                .map_or_else(|| "系统".into(), |id| code(&id.to_string()))
        ));
        if !entry.detail.is_empty() {
            text.push_str(&format!("\n{}", code(&excerpt(&entry.detail, 120))));
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
            &format!("{}:0", entry.id)
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
        button(chat, "返回上一级", "panel", "")
    ]));
    reply
        .send_markdown(e, &text, json!({"inline_keyboard":rows}))
        .await
}

const DETAIL_TEXT_LIMIT: usize = 4096;
const DETAIL_PAGE_LIMIT: usize = 3000;

fn detail_text(id: i64, page: usize, pages: usize, raw: &str) -> String {
    let heading = format!("审计 #{id} · 第 {page}/{pages} 页");
    debug_assert!(
        heading.encode_utf16().count() + 2 + raw.encode_utf16().count() <= DETAIL_TEXT_LIMIT
    );
    format!("{}\n\n{}", bold(&heading), pre(raw))
}

fn detail_cursor(raw: &str) -> Option<(i64, u32)> {
    let (id, page) = raw.split_once(':').unwrap_or((raw, "0"));
    Some((id.parse().ok().filter(|id| *id > 0)?, page.parse().ok()?))
}

fn detail_navigation(chat: i64) -> Value {
    json!({"inline_keyboard":[[
        button(chat, "返回审计记录", "audit", ""),
        button(chat, "管理面板", "panel", "")
    ]]})
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

pub async fn detail<S: Services>(
    e: &Engine<S>,
    chat: i64,
    reply: AdminReply,
    raw: &str,
) -> Result<()> {
    let Some((id, requested_page)) = detail_cursor(raw) else {
        return reply
            .send(e, "记录不存在或已过保留期限。", detail_navigation(chat))
            .await;
    };
    let Some(entry) = e.store.audit_entry(chat, id)? else {
        return reply
            .send(e, "记录不存在或已过保留期限。", detail_navigation(chat))
            .await;
    };
    let group = chat_info::label(e, chat).await?;
    let mut text = format!(
        "审计 #{} · {}\n{group}\n\n时间：{} UTC\n目标：{}\n操作人：{}\n详情：{}",
        entry.id,
        excerpt(entry.label(), 60),
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
    let mut original_message = None;
    if let Some(id) = entry.case_id() {
        if let Some(case) = e.store.get::<Case>(chat, &format!("case:{id}"))? {
            let probability = case
                .probability
                .map_or_else(|| "未取得评分".into(), |p| format!("{:.1}%", p * 100.0));
            text.push_str(&format!(
                "\n\n案件：{}\n用户 ID：{}\n垃圾消息概率：{probability}\n原消息快照：检测时保存的正文、按钮和可见信息，不包含媒体文件。",
                case.id, case.user
            ));
            if case.kind != CaseKind::JoinProfile
                && let Some(id) = chat.to_string().strip_prefix("-100")
            {
                original_message = Some(json!({
                    "text":"在群中打开（已删消息可能无法打开）",
                    "url":format!("https://t.me/c/{id}/{}", case.message)
                }));
            }
            match case.evidence {
                Some(evidence) => {
                    text.push_str("\n\n原消息快照：\n");
                    text.push_str(&snapshot(&evidence)?);
                }
                None => text.push_str("\n\n原消息快照已按保留策略清理；审计记录仍可查看。"),
            }
        } else {
            text.push_str("\n\n关联案件已过保留期限，原消息快照不可用。");
        }
    }
    let pages = text_pages(&text, DETAIL_PAGE_LIMIT);
    let page = (requested_page as usize).min(pages.len() - 1);
    let text = detail_text(entry.id, page + 1, pages.len(), pages[page]);
    let mut rows = vec![];
    let mut navigation = vec![];
    if page > 0 {
        navigation.push(button(
            chat,
            "上一页",
            "auditentry",
            &format!("{}:{}", entry.id, page - 1),
        ));
    }
    if page + 1 < pages.len() {
        navigation.push(button(
            chat,
            "下一页",
            "auditentry",
            &format!("{}:{}", entry.id, page + 1),
        ));
    }
    if !navigation.is_empty() {
        rows.push(json!(navigation));
    }
    if let Some(button) = original_message {
        rows.push(json!([button]));
    }
    rows.push(json!([
        button(chat, "返回审计记录", "audit", ""),
        button(chat, "管理面板", "panel", "")
    ]));
    reply
        .send_markdown(e, &text, json!({"inline_keyboard":rows}))
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detail_pagination_preserves_complete_unicode_evidence() {
        let text = format!(
            "{}💰正文{}结尾\n{}",
            "a".repeat(DETAIL_PAGE_LIMIT - 1),
            "💰".repeat(2000),
            "汉字".repeat(2000)
        );
        let pages = text_pages(&text, DETAIL_PAGE_LIMIT);
        assert!(pages.len() > 1);
        assert_eq!(pages.concat(), text);
        assert_eq!(pages[0].len(), DETAIL_PAGE_LIMIT - 1);
        assert!(pages[1].starts_with('💰'));
        for (index, page) in pages.iter().enumerate() {
            assert!(page.encode_utf16().count() <= DETAIL_PAGE_LIMIT);
            let wire = detail_text(i64::MAX, index + 1, pages.len(), page);
            let heading = format!("审计 #{} · 第 {}/{} 页", i64::MAX, index + 1, pages.len());
            assert!(wire.starts_with(&bold(&heading)));
            assert!(wire.ends_with(&pre(page)));
            assert!(
                heading.encode_utf16().count() + 2 + page.encode_utf16().count()
                    <= DETAIL_TEXT_LIMIT
            );
        }
        assert_eq!(text_pages("", DETAIL_PAGE_LIMIT), vec![""]);
    }

    #[test]
    fn detail_cursors_keep_legacy_ids_and_fit_callback_size_limits() {
        assert_eq!(detail_cursor("42"), Some((42, 0)));
        assert_eq!(detail_cursor("42:3"), Some((42, 3)));
        assert_eq!(
            detail_cursor(&format!("{}:{}", i64::MAX, u32::MAX)),
            Some((i64::MAX, u32::MAX))
        );
        for raw in [
            "",
            "0",
            "-1",
            "42:",
            "42:-1",
            "42:x",
            "42:0:extra",
            "42:4294967296",
        ] {
            assert_eq!(detail_cursor(raw), None, "accepted {raw}");
        }
        let callback = button(
            i64::MIN,
            "下一页",
            "auditentry",
            &format!("{}:{}", i64::MAX, u32::MAX),
        );
        assert!(callback["callback_data"].as_str().unwrap().len() <= 64);
    }

    #[test]
    fn detail_pages_preserve_backticks_and_backslashes_before_markdown_formatting() {
        let evidence = "正文💰`\\```[链接](https://example.invalid)\n".repeat(200);
        let pages = text_pages(&evidence, DETAIL_PAGE_LIMIT);
        assert!(pages.len() > 1);
        assert_eq!(pages.concat(), evidence);
        for page in pages {
            let block = pre(page);
            let mut encoded = block
                .strip_prefix("```\n")
                .unwrap()
                .strip_suffix("\n```")
                .unwrap()
                .chars();
            let mut decoded = String::new();
            while let Some(character) = encoded.next() {
                if character == '\\' {
                    let escaped = encoded.next().unwrap();
                    assert!(matches!(escaped, '\\' | '`'));
                    decoded.push(escaped);
                } else {
                    decoded.push(character);
                }
            }
            assert_eq!(decoded, page);
            assert!(page.encode_utf16().count() <= DETAIL_PAGE_LIMIT);
        }
        let escape_heavy = "`\\".repeat(1500);
        let wire = detail_text(i64::MAX, 1, 1, &escape_heavy);
        assert!(wire.encode_utf16().count() > DETAIL_TEXT_LIMIT);
        assert!(escape_heavy.encode_utf16().count() <= DETAIL_PAGE_LIMIT);
    }
}
