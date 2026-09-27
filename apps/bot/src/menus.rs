//! Telegram command scopes are presentation only; handlers always authorize again.
use crate::{
    api::{Services, is_admin},
    commands,
    config::Config,
    engine::Engine,
    error::{Error, Result},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;

pub const PUBLIC_HELP: &str = "*在线检查 / Check availability*：`/ping`\n*我的身份 / My identity*：`/whoami`\n*Bot 版本 / Bot version*：`/version`";
const MENU_VERSION: u32 = 5;

fn command_list(private: bool, privileged: bool) -> Vec<Value> {
    let mut entries = if privileged {
        vec![
            ("ping", "在线检查"),
            ("whoami", "我的身份"),
            ("version", "Bot 版本"),
        ]
    } else {
        commands::MENU.to_vec()
    };
    if private {
        entries.push((
            "help",
            if privileged {
                "命令帮助"
            } else {
                "命令帮助 / Command help"
            },
        ));
    }
    if privileged {
        entries.extend([
            ("admin", "群管理面板"),
            ("pending", "待审消息： /pending CHAT_ID"),
            ("health", "状态权限： /health CHAT_ID"),
            ("audit", "审计记录： /audit CHAT_ID"),
            ("stats", "每日统计： /stats 群ID"),
            ("thresholds", "阈值： /thresholds CHAT_ID 60 80"),
            ("retry", "重试任务： /retry CHAT_ID"),
            ("unban", "解除封禁： /unban CHAT_ID USER_ID"),
        ]);
    }
    entries
        .into_iter()
        .map(|(command, description)| json!({"command":command,"description":description}))
        .collect()
}

async fn set_scope<S: Services>(services: &S, scope: Value, commands: Vec<Value>) -> Result<()> {
    services
        .telegram("setMyCommands", json!({"scope":scope,"commands":commands}))
        .await?;
    // A language-specific scope takes precedence over the default language.
    for language in ["en", "zh"] {
        services
            .telegram(
                "deleteMyCommands",
                json!({"scope":scope,"language_code":language}),
            )
            .await?;
    }
    Ok(())
}

/// Replace the old all-private admin menu before accepting any updates.
pub async fn install_public<S: Services>(services: &S, config: &Config) -> Result<()> {
    for kind in [
        "default",
        "all_private_chats",
        "all_group_chats",
        "all_chat_administrators",
    ] {
        set_scope(
            services,
            json!({"type":kind}),
            command_list(kind == "all_private_chats", false),
        )
        .await?;
    }
    for chat in &config.chats {
        let mut commands = command_list(false, false);
        if config.is_verbose(*chat) {
            commands.push(json!({"command":"spamcheck","description":"Jev 判断 / Jev evaluation"}));
        }
        set_scope(services, json!({"type":"chat","chat_id":chat}), commands).await?;
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
struct PrivateMenu {
    user: i64,
    applied: Option<bool>,
    version: u32,
}

/// Persist intent first, so a lost API response cannot leave an untracked grant.
pub async fn set_private<S: Services>(e: &Engine<S>, user: i64, privileged: bool) -> Result<()> {
    if user <= 0 {
        return Ok(());
    }
    let key = format!("menu:{user}");
    let old = e.store.get::<PrivateMenu>(0, &key)?;
    if old
        .as_ref()
        .is_some_and(|m| m.applied == Some(privileged) && m.version == MENU_VERSION)
        || (old.is_none() && !privileged)
    {
        return Ok(());
    }
    let mut menu = PrivateMenu {
        user,
        applied: None,
        version: MENU_VERSION,
    };
    e.store.put(0, &key, &menu)?;
    set_scope(
        e.services.as_ref(),
        json!({"type":"chat","chat_id":user}),
        command_list(true, privileged),
    )
    .await?;
    menu.applied = Some(privileged);
    e.store.put(0, &key, &menu)
}

pub async fn readable_groups<S: Services>(e: &Engine<S>, user: i64) -> Vec<i64> {
    if e.config.is_super_admin(user) {
        return e.config.chats.clone();
    }
    let mut groups = vec![];
    for chat in &e.config.chats {
        match e.member(*chat, user).await {
            Ok(member) if is_admin(&member) => groups.push(*chat),
            Ok(_) => (),
            Err(error) => e.logger.error(
                "menu.membership_error",
                json!({"chat_id":chat,"error":error.to_string()}),
            ),
        }
    }
    groups
}

pub async fn group_admins<S: Services>(e: &Engine<S>, chat: i64) -> Result<BTreeSet<i64>> {
    let members = e
        .services
        .telegram("getChatAdministrators", json!({"chat_id":chat}))
        .await?;
    let members = members
        .as_array()
        .ok_or_else(|| Error::external("invalid_admin_list", false))?;
    Ok(members
        .iter()
        .filter(|m| is_admin(m) && m["user"]["is_bot"] == false)
        .filter_map(|m| m["user"]["id"].as_i64().filter(|id| *id > 0))
        .collect())
}

/// Refresh presentation for previously granted private menus; never an ACL cache.
pub async fn refresh<S: Services>(e: &Engine<S>) -> Result<()> {
    let menus = e.store.list::<PrivateMenu>(0, "menu:", None, 0, -1)?;
    if menus.is_empty() {
        return Ok(());
    }
    let mut allowed: BTreeSet<i64> = e.config.super_admins.iter().copied().collect();
    for chat in &e.config.chats {
        match group_admins(e, *chat).await {
            Ok(admins) => allowed.extend(admins),
            // Fail closed for this group's menus; the next refresh can restore them.
            Err(error) => e.logger.error(
                "menu.admin_list_error",
                json!({"chat_id":chat,"error":error.to_string()}),
            ),
        }
    }
    for menu in menus {
        if let Err(error) = set_private(e, menu.user, allowed.contains(&menu.user)).await {
            e.logger.error(
                "menu.sync_error",
                json!({"user_id":menu.user,"error":error.to_string()}),
            );
        }
    }
    Ok(())
}
