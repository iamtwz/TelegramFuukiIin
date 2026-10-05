use crate::{
    admin_view::{AdminReply, text_pages},
    api::{Services, can_manage, can_moderate, is_admin},
    config::Config,
    engine::Engine,
    error::Result,
    markdown::{bold, code, escape, pre},
    model::{Case, CaseKind, ScreeningMode},
    store::change,
};
use serde_json::{Value, json};

pub fn command(text: &str, bot: &str) -> Option<(String, Vec<String>)> {
    let mut words = text.split_whitespace();
    let first = words.next()?.strip_prefix('/')?;
    let (name, target) = first
        .split_once('@')
        .map_or((first, None), |(a, b)| (a, Some(b)));
    if target.is_some_and(|t| !t.eq_ignore_ascii_case(bot))
        || !name.bytes().all(|b| b.is_ascii_alphabetic())
    {
        return None;
    }
    Some((name.to_lowercase(), words.map(str::to_owned).collect()))
}
pub fn target_chat(update: &Value, config: &Config) -> Option<i64> {
    if update["callback_query"].is_object() {
        let source = &update["callback_query"];
        if source["message"]["chat"]["type"] != "private"
            || source["message"]["chat"]["id"].as_i64() != source["from"]["id"].as_i64()
            || source["message"]["message_id"]
                .as_i64()
                .is_none_or(|id| id <= 0)
        {
            return None;
        }
        return update["callback_query"]["data"]
            .as_str()?
            .split('|')
            .nth(1)?
            .parse()
            .ok();
    }
    let (cmd, args) = command(update["message"]["text"].as_str()?, &config.bot_username)?;
    if cmd == "start" {
        return args.first()?.strip_prefix("admin_")?.parse().ok();
    }
    args.first()?.parse().ok()
}
pub(crate) fn button(chat: i64, label: &str, action: &str, id: &str) -> Value {
    json!({"text":label,"callback_data":format!("{action}|{chat}|{id}")})
}
fn back(chat: i64) -> Value {
    json!({"inline_keyboard":[[button(chat,"返回上一级","panel","")]]})
}

fn base36(mut n: usize) -> String {
    let mut digits = Vec::new();
    loop {
        digits.push(char::from_digit((n % 36) as u32, 36).unwrap());
        n /= 36;
        if n == 0 {
            break;
        }
    }
    digits.into_iter().rev().collect()
}

fn case_cursor(id: &str, offset: usize, page: usize) -> String {
    format!("{id}:{}:{}", base36(offset), base36(page))
}

async fn groups<S: Services>(
    e: &Engine<S>,
    reply: AdminReply,
    offset: usize,
    stats: bool,
) -> Result<()> {
    let groups = crate::menus::readable_groups(e, reply.user).await;
    let privileged = e.config.is_super_admin(reply.user) || !groups.is_empty();
    crate::menus::set_private(e, reply.user, privileged).await?;
    if !privileged && reply.message.is_none() {
        return Ok(());
    }
    let offset = offset.min(groups.len().saturating_sub(1)) / 10 * 10;
    let mut rows = vec![];
    let mut labels = vec![];
    for group in groups.iter().skip(offset).take(10) {
        let label = crate::chat_info::label(e, *group).await?;
        let suffix = format!(" · {group}");
        labels.push(format!(
            "{} · {}",
            escape(label.strip_suffix(&suffix).unwrap_or(&label)),
            code(&group.to_string())
        ));
        rows.push(json!([button(
            *group,
            &label,
            if stats { "stats" } else { "panel" },
            ""
        )]));
    }
    let action = if stats { "groupstats" } else { "groups" };
    let mut navigation = vec![];
    if offset > 0 {
        navigation.push(button(0, "上一页", action, &(offset - 10).to_string()));
    }
    if offset + 10 < groups.len() {
        navigation.push(button(0, "下一页", action, &(offset + 10).to_string()));
    }
    if !navigation.is_empty() {
        rows.push(json!(navigation));
    }
    let text = if groups.is_empty() {
        "没有可查看的已配置群。".into()
    } else {
        format!(
            "{}\n{}\n第 {}/{} 页",
            bold("选择要查看的群："),
            labels.join("\n"),
            code(&(offset / 10 + 1).to_string()),
            code(&groups.len().div_ceil(10).to_string())
        )
    };
    reply
        .send_markdown(e, &text, json!({"inline_keyboard":rows}))
        .await
}

async fn panel<S: Services>(
    e: &Engine<S>,
    chat: i64,
    reply: AdminReply,
    editable: bool,
) -> Result<()> {
    let s = e.settings(chat)?;
    let mut rows = vec![];
    if editable {
        rows.push(json!([
            button(
                chat,
                if s.spam {
                    "关闭检测"
                } else {
                    "开启检测"
                },
                if s.spam { "spamoff" } else { "spamon" },
                ""
            ),
            button(
                chat,
                if s.captcha {
                    "关闭验证"
                } else {
                    "开启验证"
                },
                if s.captcha { "capoff" } else { "capon" },
                ""
            )
        ]));
        rows.push(json!([if s.screening == ScreeningMode::NewMembers {
            button(chat, "改用首次发言", "firstseen", "")
        } else {
            button(chat, "改用入群首条", "newmembers", "")
        }]));
        rows.push(json!([button(
            chat,
            if s.join_profile_review {
                "关闭入群资料审核"
            } else {
                "开启入群资料审核（Jev）"
            },
            if s.join_profile_review {
                "profileoff"
            } else {
                "profileon"
            },
            ""
        )]));
    }
    rows.push(json!([
        button(chat, "待审消息", "pending", "0"),
        button(chat, "状态与权限", "health", "")
    ]));
    rows.push(json!([
        button(chat, "审计记录", "audit", ""),
        button(chat, "每日统计", "stats", "")
    ]));
    rows.push(json!([button(0, "返回上一级", "groups", "0")]));
    let controls = if editable {
        format!(
            "\n{}\n{}\n{}",
            code(&format!("/thresholds {chat} 60 80")),
            code(&format!("/retry {chat}")),
            code(&format!("/unban {chat} USER_ID")),
        )
    } else {
        "\n设置仅供查看".into()
    };
    let scope = match s.screening {
        ScreeningMode::NewMembers => "入群后的首条消息",
        ScreeningMode::FirstSeen => "本群首次记录的发言",
    };
    let group = crate::chat_info::markdown_label(e, chat).await?;
    let profile = if s.join_profile_review {
        "开启（额外调用 Jev，受检测总开关控制）"
    } else {
        "关闭（默认，发言时审核资料）"
    };
    reply.send_markdown(
        e,
        &format!(
            "{}\nBot 版本：{}\n群：{group}\n首条消息检测：{}\n检测范围：{scope}\n首次发言模式支持未入群评论；此前未记录的老用户也会检测。Guest Bot 回复逐条检测。\n入群验证：{}\n入群资料审核：{profile}\n人工审批：≥{}\n删除封禁：≥{}{controls}",
            bold("Telegram 风纪委员管理面板"),
            code(crate::VERSION),
            if s.spam { "开启" } else { "关闭" },
            if s.captcha { "开启" } else { "关闭" },
            code(&format!("{:.1}%", s.review * 100.0)),
            code(&format!("{:.1}%", s.ban * 100.0)),
        ),
        json!({"inline_keyboard":rows}),
    )
    .await
}
async fn pending<S: Services>(
    e: &Engine<S>,
    chat: i64,
    reply: AdminReply,
    offset: i64,
) -> Result<()> {
    let offset = offset.clamp(0, 1_000_000);
    let cases = e
        .store
        .list::<Case>(chat, "case:", Some("review"), offset, 6)?;
    let group = crate::chat_info::markdown_label(e, chat).await?;
    let mut text = format!(
        "{}\n群：{group}\n第 {} 页",
        bold("待审消息"),
        code(&(offset / 5 + 1).to_string())
    );
    let mut rows = vec![];
    for c in cases.iter().take(5) {
        let score = c.probability.map_or_else(
            || "评分不可用，需人工审核".into(),
            |p| format!("垃圾消息概率：{}", code(&format!("{:.1}%", p * 100.0))),
        );
        let kind = match c.kind {
            CaseKind::Message => "消息与资料",
            CaseKind::Guest => "Guest Bot 与召唤者",
            CaseKind::JoinProfile => "入群资料",
        };
        text.push_str(&format!(
            "\n\n案件：{} · {kind}\n用户：{} · {score}",
            code(&c.id),
            code(&c.user.to_string())
        ));
        rows.push(json!([button(
            chat,
            &format!("查看案件 · {}", c.user),
            "case",
            &case_cursor(&c.id, offset as usize, 0)
        )]));
    }
    if cases.is_empty() {
        text.push_str("\n\n当前没有待审消息。");
    }
    let mut navigation = vec![];
    if offset > 0 {
        navigation.push(button(
            chat,
            "上一页",
            "pending",
            &offset.saturating_sub(5).to_string(),
        ));
    }
    if cases.len() > 5 {
        navigation.push(button(chat, "下一页", "pending", &(offset + 5).to_string()));
    }
    if !navigation.is_empty() {
        rows.push(json!(navigation));
    }
    rows.push(json!([
        button(chat, "刷新", "pending", &offset.to_string()),
        button(chat, "返回上一级", "panel", "")
    ]));
    reply
        .send_markdown(e, &text, json!({"inline_keyboard":rows}))
        .await
}

async fn case_detail<S: Services>(
    e: &Engine<S>,
    chat: i64,
    reply: AdminReply,
    raw: &str,
    editable: bool,
    join_editable: bool,
) -> Result<()> {
    let parts = raw.split(':').collect::<Vec<_>>();
    let id = parts.first().copied().unwrap_or("");
    let offset = parts
        .get(1)
        .and_then(|v| usize::from_str_radix(v, 36).ok())
        .unwrap_or(0)
        .min(1_000_000);
    let requested = parts
        .get(2)
        .and_then(|v| usize::from_str_radix(v, 36).ok())
        .unwrap_or(0)
        .min(46_655);
    let Some(case) = e.store.get::<Case>(chat, &format!("case:{id}"))? else {
        return reply.send(e, "案件不存在或已过保留期限。", json!({"inline_keyboard":[[button(chat,"返回上一级","pending",&offset.to_string()),button(chat,"管理面板","panel","")]]})).await;
    };
    let text = serde_json::to_string_pretty(&case)?;
    let pages = text_pages(&text, 2600);
    let page = requested.min(pages.len().saturating_sub(1));
    let group = crate::chat_info::markdown_label(e, chat).await?;
    let mut rows = vec![];
    if case.state == "review" && editable && (case.kind != CaseKind::JoinProfile || join_editable) {
        rows.push(json!([
            button(chat, "放行", "allow", id),
            button(
                chat,
                if case.kind == CaseKind::JoinProfile {
                    "拒绝并封禁"
                } else {
                    "删除封禁"
                },
                "ban",
                id
            )
        ]));
    }
    if case.kind != CaseKind::JoinProfile
        && let Some(group) = chat.to_string().strip_prefix("-100")
    {
        rows.push(json!([{"text":"在群中打开（已删消息可能无法打开）","url":format!("https://t.me/c/{group}/{}",case.message)}]));
    }
    let mut navigation = vec![];
    if page > 0 {
        navigation.push(button(
            chat,
            "上一页",
            "case",
            &case_cursor(id, offset, page - 1),
        ));
    }
    if page + 1 < pages.len() {
        navigation.push(button(
            chat,
            "下一页",
            "case",
            &case_cursor(id, offset, page + 1),
        ));
    }
    if !navigation.is_empty() {
        rows.push(json!(navigation));
    }
    rows.push(json!([
        button(chat, "返回上一级", "pending", &offset.to_string()),
        button(chat, "管理面板", "panel", "")
    ]));
    let tracks_followups = case.kind == CaseKind::Message
        && (e
            .store
            .get::<crate::message_cleanup::MessageCase>(
                chat,
                &format!("message_case:{}", case.user),
            )?
            .is_some_and(|index| index.id == case.id)
            || !e
                .store
                .list::<Value>(chat, &format!("case_message:{}:", case.id), None, 0, 1)?
                .is_empty());
    let cleanup_scope = if tracks_followups {
        "\n删除范围：首条消息及封禁前记录的同一发送者后续消息；后续清理独立执行，可在审计中查看结果。"
    } else {
        ""
    };
    reply
        .send_markdown(
            e,
            &format!(
                "{}：{}\n群：{group}{cleanup_scope}\n第 {}/{} 页\n{}",
                bold("案件详情"),
                code(id),
                code(&(page + 1).to_string()),
                code(&pages.len().to_string()),
                pre(pages[page])
            ),
            json!({"inline_keyboard":rows}),
        )
        .await
}
async fn health<S: Services>(e: &Engine<S>, chat: i64, reply: AdminReply) -> Result<()> {
    let bot = e.member(chat, e.config.bot_id()).await?;
    let group = crate::chat_info::markdown_label(e, chat).await?;
    let status = match bot["status"].as_str() {
        Some("creator") => "群主",
        Some("administrator") => "管理员",
        Some("member") => "成员",
        Some("restricted") => "受限成员",
        Some("left") => "已离群",
        Some("kicked") => "已封禁",
        _ => "未知",
    };
    let permission = |name: &str| {
        if bot[name] == true || bot["status"] == "creator" {
            "有"
        } else {
            "无"
        }
    };
    let stats = e.store.stats(chat)?;
    reply.send_markdown(
        e,
        &format!(
            "{}\nBot 版本：{}\n群：{group}\nBot 状态：{status}\n删除权限：{}\n封禁权限：{}\n审批入群权限：{}\n任务：待执行 {}，执行中 {}，已完成 {}，失败 {}",
            bold("状态与权限"),
            code(crate::VERSION),
            permission("can_delete_messages"),
            permission("can_restrict_members"),
            permission("can_invite_users"),
            code(&stats["pending"].as_i64().unwrap_or(0).to_string()),
            code(&stats["running"].as_i64().unwrap_or(0).to_string()),
            code(&stats["done"].as_i64().unwrap_or(0).to_string()),
            code(&stats["dead"].as_i64().unwrap_or(0).to_string()),
        ),
        back(chat),
    )
    .await
}
pub fn decide<S: Services>(
    e: &Engine<S>,
    chat: i64,
    id: &str,
    ban: bool,
    user: i64,
) -> Result<bool> {
    let Some(mut c) = e.store.get::<Case>(chat, &format!("case:{id}"))? else {
        return Ok(false);
    };
    if c.state != "review" {
        return Ok(false);
    }
    if c.created_at < e.now() - 2_592_000 {
        c.state = "expired".into();
        e.store.put(chat, &format!("case:{id}"), &c)?;
        return Ok(false);
    }
    if c.kind == CaseKind::JoinProfile && e.active_join_case(chat, &c)?.is_none() {
        c.state = "expired".into();
        e.store.put(chat, &format!("case:{id}"), &c)?;
        return Ok(false);
    }
    c.state = if ban { "enforcing" } else { "allowed" }.into();
    c.actor = Some(user);
    let jobs = if ban {
        vec![crate::model::NewJob::new(
            chat,
            "enforce",
            id,
            json!({"id":id}),
            e.now(),
        )]
    } else {
        crate::join_screening::approval_job(&c, chat, e.now(), "review")
            .into_iter()
            .collect()
    };
    e.store.apply(
        vec![change(chat, format!("case:{id}"), &c)?],
        &jobs,
        e.now(),
    )?;
    e.audit(
        chat,
        if ban { "review_ban" } else { "review_allow" },
        id,
        "",
        Some(user),
    )?;
    Ok(true)
}
pub async fn handle<S: Services>(e: &Engine<S>, chat: i64, u: &Value) -> Result<()> {
    let callback = u["callback_query"].is_object();
    let source = if callback {
        &u["callback_query"]
    } else {
        &u["message"]
    };
    let context = if callback { &source["message"] } else { source };
    let Some(user) = source["from"]["id"].as_i64().filter(|id| *id > 0) else {
        return Ok(());
    };
    if source["from"]["is_bot"] == true || context["chat"]["type"] != "private" {
        return Ok(());
    }
    let message = if callback {
        if context["chat"]["id"].as_i64() != Some(user) {
            return Ok(());
        }
        let Some(id) = context["message_id"].as_i64().filter(|id| *id > 0) else {
            return Ok(());
        };
        Some(id)
    } else {
        None
    };
    let reply = AdminReply::new(user, message);
    let callback_parts = source["data"]
        .as_str()
        .unwrap_or("")
        .split('|')
        .collect::<Vec<_>>();
    if callback
        && (callback_parts.len() != 3
            || callback_parts.get(1).and_then(|s| s.parse::<i64>().ok()) != Some(chat))
    {
        return Ok(());
    }
    if callback && chat == 0 {
        let action = callback_parts[0];
        if !matches!(action, "groups" | "groupstats") {
            return Ok(());
        }
        e.services
            .telegram(
                "answerCallbackQuery",
                json!({"callback_query_id":source["id"]}),
            )
            .await?;
        return groups(
            e,
            reply,
            callback_parts[2]
                .parse::<usize>()
                .unwrap_or(0)
                .min(1_000_000),
            action == "groupstats",
        )
        .await;
    }
    let super_admin = e.config.is_super_admin(user);
    let parsed = command(
        source["text"].as_str().unwrap_or(""),
        &e.config.bot_username,
    );
    if !callback
        && parsed.as_ref().is_some_and(|(name, _)| {
            !matches!(
                name.as_str(),
                "admin"
                    | "start"
                    | "help"
                    | "pending"
                    | "case"
                    | "health"
                    | "audit"
                    | "stats"
                    | "thresholds"
                    | "retry"
                    | "unban"
            )
        })
    {
        return Ok(());
    }
    if !callback
        && parsed.is_none()
        && source["text"]
            .as_str()
            .is_some_and(|text| text.trim_start().starts_with('/'))
    {
        return Ok(());
    }
    if !callback && chat == 0 {
        let name = parsed.as_ref().map(|(name, _)| name.as_str());
        if !matches!(name, Some("admin" | "help" | "start" | "stats")) {
            return Ok(());
        }
        if matches!(name, Some("admin" | "stats")) {
            return groups(e, reply, 0, name == Some("stats")).await;
        }
        let readable = crate::menus::readable_groups(e, user).await;
        let privileged = super_admin || !readable.is_empty();
        crate::menus::set_private(e, user, privileged).await?;
        if name == Some("help") {
            let help = if privileged {
                format!(
                    "{}：{}\n{}：{}\n{}：{}\n{}：{}",
                    bold("在线检查"),
                    code("/ping"),
                    bold("我的身份"),
                    code("/whoami"),
                    bold("版本查询"),
                    code("/version"),
                    bold("管理面板"),
                    code("/admin"),
                )
            } else {
                crate::menus::PUBLIC_HELP.into()
            };
            return e.send_markdown(user, &help, Value::Null).await;
        }
        return Ok(());
    }
    if !e.config.chats.contains(&chat) {
        return Ok(());
    }
    // Local super-admin IDs grant cross-group read access only. A failed
    // membership lookup never grants mutation rights, even to a super admin.
    let member = if super_admin {
        e.member(chat, user).await.unwrap_or(Value::Null)
    } else {
        e.member(chat, user).await?
    };
    if !super_admin && !is_admin(&member) {
        let still_admin = !crate::menus::readable_groups(e, user).await.is_empty();
        crate::menus::set_private(e, user, still_admin).await?;
        if callback {
            e.services
                .telegram(
                    "answerCallbackQuery",
                    json!({"callback_query_id":source["id"],"text":"不可用","show_alert":true}),
                )
                .await?;
        }
        return Ok(());
    }
    crate::menus::set_private(e, user, true).await?;
    if callback {
        let action = callback_parts[0];
        let id = callback_parts[2];
        if !matches!(
            action,
            "panel"
                | "pending"
                | "case"
                | "health"
                | "stats"
                | "audit"
                | "auditentry"
                | "spamon"
                | "spamoff"
                | "capon"
                | "capoff"
                | "firstseen"
                | "newmembers"
                | "profileon"
                | "profileoff"
                | "allow"
                | "ban"
        ) {
            e.services.telegram("answerCallbackQuery",json!({"callback_query_id":source["id"],"text":"菜单不可用，请重新打开管理面板。","show_alert":true})).await?;
            return Ok(());
        }
        let setting = [
            "spamon",
            "spamoff",
            "capon",
            "capoff",
            "firstseen",
            "newmembers",
            "profileon",
            "profileoff",
        ]
        .contains(&action);
        let moderation = ["ban", "allow"].contains(&action);
        let join_review = moderation
            && e.store
                .get::<Case>(chat, &format!("case:{id}"))?
                .is_some_and(|c| c.kind == CaseKind::JoinProfile);
        if (setting && !can_manage(&member))
            || (moderation && !can_moderate(&member))
            || (join_review && !can_manage(&member))
        {
            e.services.telegram("answerCallbackQuery",json!({"callback_query_id":source["id"],"text":"你的管理员权限不足","show_alert":true})).await?;
            return Ok(());
        }
        let mut notice = "已打开";
        if moderation {
            notice = if decide(e, chat, id, action == "ban", user)? {
                if action == "ban" {
                    if join_review {
                        "已提交拒绝和封禁"
                    } else {
                        "已提交删除和封禁"
                    }
                } else {
                    if join_review {
                        "资料已放行，仍需完成人机验证"
                    } else {
                        "已放行"
                    }
                }
            } else {
                "案件已处理或过期"
            };
        }
        if setting {
            notice = "已更新";
            let mut s = e.settings(chat)?;
            match action {
                "spamon" | "spamoff" => s.spam = action == "spamon",
                "capon" | "capoff" => s.captcha = action == "capon",
                "profileon" | "profileoff" => s.join_profile_review = action == "profileon",
                "firstseen" => s.screening = ScreeningMode::FirstSeen,
                "newmembers" => s.screening = ScreeningMode::NewMembers,
                _ => (),
            }
            e.store.put(chat, "settings", &s)?;
            e.audit(
                chat,
                "settings_changed",
                &chat.to_string(),
                &serde_json::to_string(&s)?,
                Some(user),
            )?;
        }
        e.services
            .telegram(
                "answerCallbackQuery",
                json!({"callback_query_id":source["id"],"text":notice}),
            )
            .await?;
        return match action {
            "allow" | "ban" => reply.send_markdown(e, &format!("{}：{}\n{}", bold("案件"), code(id), escape(notice)),json!({"inline_keyboard":[[button(chat,"返回待审消息","pending","0"),button(chat,"管理面板","panel","")]]})).await,
            "pending" => pending(e, chat, reply, id.parse::<i64>().unwrap_or(0)).await,
            "case" => case_detail(e,chat,reply,id,can_moderate(&member),can_manage(&member)).await,
            "health" => health(e, chat, reply).await,
            "stats" => crate::statistics::show(e, chat, reply, id).await,
            "audit" => crate::admin_audit::show(e, chat, reply, id).await,
            "auditentry" => crate::admin_audit::detail(e, chat, reply, id).await,
            _ => panel(e, chat, reply, can_manage(&member)).await,
        };
    }
    let Some((cmd, args)) = parsed else {
        return Ok(());
    };
    match cmd.as_str() {
        "admin" | "start" => return panel(e, chat, reply, can_manage(&member)).await,
        "pending" => return pending(e, chat, reply, 0).await,
        "case" => {
            return case_detail(
                e,
                chat,
                reply,
                args.get(1).map_or("", String::as_str),
                can_moderate(&member),
                can_manage(&member),
            )
            .await;
        }
        "health" => return health(e, chat, reply).await,
        "audit" => return crate::admin_audit::show(e, chat, reply, "").await,
        "stats" => {
            return crate::statistics::show(e, chat, reply, args.get(1).map_or("", String::as_str))
                .await;
        }
        _ => (),
    }
    if !can_manage(&member) {
        return e
            .send(
                user,
                "此操作需要删除、限制成员和邀请权限，或群主身份。",
                Value::Null,
            )
            .await;
    }
    match cmd.as_str() {
        "thresholds" => {
            let parse = |index| {
                args.get(index)
                    .and_then(|s: &String| s.parse::<f64>().ok())
                    .unwrap_or(f64::NAN)
                    / 100.0
            };
            let review = parse(1);
            let ban = parse(2);
            if args.len() != 3
                || !review.is_finite()
                || !ban.is_finite()
                || review < 0.0
                || review >= ban
                || ban > 1.0
            {
                let usage = format!(
                    "用法： {}\n阈值要求：0 ≤ 人工审核阈值 < 删除封禁阈值 ≤ 100。",
                    code(&format!("/thresholds {chat} 60 80"))
                );
                return e.send_markdown(user, &usage, Value::Null).await;
            }
            let mut settings = e.settings(chat)?;
            settings.review = review;
            settings.ban = ban;
            e.store.put(chat, "settings", &settings)?;
            e.audit(
                chat,
                "thresholds_changed",
                &chat.to_string(),
                &serde_json::to_string(&settings)?,
                Some(user),
            )?;
            panel(e, chat, reply, can_manage(&member)).await
        }
        "retry" => {
            let count = e.store.retry(chat, e.now())?;
            e.audit(
                chat,
                "jobs_retried",
                &chat.to_string(),
                &count.to_string(),
                Some(user),
            )?;
            e.send_markdown(
                user,
                &format!("已安排重试 {} 个失败任务。", code(&count.to_string())),
                Value::Null,
            )
            .await
        }
        "unban" => {
            let Some(target) = args
                .get(1)
                .and_then(|s| s.parse::<i64>().ok())
                .filter(|id| *id > 0)
            else {
                return e
                    .send_markdown(
                        user,
                        &format!("用法： {}", code(&format!("/unban {chat} USER_ID"))),
                        Value::Null,
                    )
                    .await;
            };
            let mut offset = 0;
            // Materialize all matching cases before mutation to avoid pagination skips.
            let mut cases = vec![];
            loop {
                let page = e.store.list::<Case>(chat, "case:", None, offset, 100)?;
                let count = page.len();
                cases.extend(page);
                if count < 100 {
                    break;
                }
                offset += 100;
            }
            let changes = cases
                .into_iter()
                .filter(|c| {
                    (c.user == target
                        || c.guest_caller
                            .as_ref()
                            .is_some_and(|caller| caller.user == target))
                        && matches!(c.state.as_str(), "enforcing" | "classifying" | "review")
                })
                .map(|mut c| {
                    c.state = "allowed".into();
                    c.actor = Some(user);
                    change(chat, format!("case:{}", c.id), &c)
                })
                .collect::<Result<Vec<_>>>()?;
            e.store.apply(changes, &[], e.now())?;
            e.services
                .telegram(
                    "unbanChatMember",
                    json!({"chat_id":chat,"user_id":target,"only_if_banned":true}),
                )
                .await?;
            e.audit(chat, "user_unbanned", &target.to_string(), "", Some(user))?;
            e.send(user, "已解除封禁，用户可重新申请并完成验证。", Value::Null)
                .await
        }
        _ => {
            e.send_markdown(
                user,
                &format!(
                    "{}：\n{}\n请附带群 ID。",
                    bold("管理命令"),
                    code("/admin /pending /case /health /audit /stats /thresholds /retry /unban")
                ),
                Value::Null,
            )
            .await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn case_cursors_keep_the_origin_page_inside_telegram_callback_limit() {
        let id = "a".repeat(32);
        let cursor = case_cursor(&id, 1_000_000, 46_655);
        assert!(
            button(-9_007_199_254_740_991, "案件", "case", &cursor)["callback_data"]
                .as_str()
                .unwrap()
                .len()
                <= 64
        );
        let parts = cursor.split(':').collect::<Vec<_>>();
        assert_eq!(parts[0], id);
        assert_eq!(usize::from_str_radix(parts[1], 36).unwrap(), 1_000_000);
        assert_eq!(usize::from_str_radix(parts[2], 36).unwrap(), 46_655);
    }
}
