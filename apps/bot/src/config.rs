use crate::{
    error::{Error, Result},
    logging::LogLevel,
};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

#[derive(Clone)]
pub struct Config {
    pub telegram_token: String,
    pub openrouter_key: String,
    pub turnstile_secret: String,
    pub turnstile_site_key: String,
    pub verification_url: String,
    pub bot_username: String,
    pub chats: Vec<i64>,
    pub super_admins: Vec<i64>,
    pub database: PathBuf,
    pub jev_model: String,
    pub log_level: LogLevel,
}
impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let mut vars = match std::fs::read_to_string(path) {
            Ok(text) => parse_env(&text)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => HashMap::new(),
            Err(e) => return Err(e.into()),
        };
        vars.extend(std::env::vars());
        Self::from_map(&vars)
    }
    pub fn from_map(vars: &HashMap<String, String>) -> Result<Self> {
        let get = |key: &'static str| {
            vars.get(key)
                .cloned()
                .filter(|v| !v.is_empty())
                .ok_or(Error::Config(key))
        };
        let telegram_token = get("TELEGRAM_BOT_TOKEN")?;
        if telegram_token
            .split_once(':')
            .and_then(|(id, secret)| {
                id.parse::<u64>()
                    .ok()
                    .filter(|id| *id > 0 && !secret.is_empty())
            })
            .is_none()
        {
            return Err(Error::Config("invalid_bot_token"));
        }
        let turnstile_secret = get("TURNSTILE_SECRET_KEY")?;
        let turnstile_site_key = get("TURNSTILE_SITE_KEY")?;
        if turnstile_site_key.len() > 100
            || !turnstile_site_key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            return Err(Error::Config("invalid_turnstile_site_key"));
        }
        let url = reqwest::Url::parse(&get("VERIFICATION_BASE_URL")?)
            .map_err(|_| Error::Config("invalid_verification_url"))?;
        if url.scheme() != "https"
            || url.host_str().is_none()
            || url.path() != "/"
            || url.query().is_some()
            || url.fragment().is_some()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(Error::Config("verification_url_must_be_https_origin"));
        }
        let bot_username = get("BOT_USERNAME")?;
        if !bot_username
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            return Err(Error::Config("invalid_bot_username"));
        }
        let mut chats = Vec::new();
        let managed_chats = vars.get("MANAGED_CHAT_IDS").map_or("", |v| v.trim());
        // An empty allowlist permits setup commands, never management of all chats.
        for item in managed_chats
            .split(',')
            .filter(|_| !managed_chats.is_empty())
        {
            let id: i64 = item
                .trim()
                .parse()
                .map_err(|_| Error::Config("invalid_managed_chat"))?;
            if !(-9_007_199_254_740_991..0).contains(&id) {
                return Err(Error::Config("invalid_managed_chat"));
            }
            if !chats.contains(&id) {
                chats.push(id);
            }
        }
        let mut super_admins = Vec::new();
        let configured_admins = vars.get("SUPER_ADMIN_IDS").map_or("", |v| v.trim());
        for item in configured_admins
            .split(',')
            .filter(|_| !configured_admins.is_empty())
        {
            let item = item.trim();
            let id: i64 = item
                .parse()
                .map_err(|_| Error::Config("invalid_super_admin_id"))?;
            if !(1..=9_007_199_254_740_991).contains(&id)
                || !item.bytes().all(|b| b.is_ascii_digit())
            {
                return Err(Error::Config("invalid_super_admin_id"));
            }
            if !super_admins.contains(&id) {
                super_admins.push(id);
            }
        }
        Ok(Self {
            log_level: LogLevel::parse(vars.get("LOG_LEVEL").map_or("info", String::as_str))?,
            telegram_token,
            openrouter_key: get("OPENROUTER_API_KEY")?,
            turnstile_secret,
            turnstile_site_key,
            verification_url: url.origin().ascii_serialization(),
            bot_username,
            chats,
            super_admins,
            database: vars
                .get("DATABASE_PATH")
                .map_or_else(|| PathBuf::from("data/fuuki-iin.sqlite"), PathBuf::from),
            jev_model: vars
                .get("JEV_MODEL")
                .cloned()
                .unwrap_or_else(|| "typesafe/jev-1.13".into()),
        })
    }
    pub fn is_super_admin(&self, user: i64) -> bool {
        user > 0 && self.super_admins.contains(&user)
    }
    pub fn bot_id(&self) -> i64 {
        self.telegram_token
            .split(':')
            .next()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0)
    }
}
pub fn parse_env(text: &str) -> Result<HashMap<String, String>> {
    let mut result = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let (key, value) = line
            .split_once('=')
            .ok_or(Error::Config("invalid_env_line"))?;
        let key = key.trim();
        if key.is_empty() || !key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
            return Err(Error::Config("invalid_env_key"));
        }
        let value = value.trim();
        let value = if (value.starts_with('"') && value.ends_with('"'))
            || (value.starts_with('\'') && value.ends_with('\''))
        {
            value
                .get(1..value.len() - 1)
                .ok_or(Error::Config("invalid_env_quote"))?
        } else {
            value
        };
        // Literal values only: no shell execution, interpolation or escape expansion.
        result.insert(key.to_string(), value.to_string());
    }
    Ok(result)
}
