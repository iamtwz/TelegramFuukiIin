use clap::{Parser, Subcommand};
use fuuki_iin_bot::{
    api::{Api, Services},
    config::{Config, parse_env},
    error::{Error, Result},
    logging::{LogLevel, Logger},
    menus, runtime,
};
use serde_json::json;
use std::{path::PathBuf, sync::Arc};
#[derive(Parser)]
#[command(
    version,
    about = "TelegramFuukiIin: local Telegram moderation and join verification bot"
)]
struct Cli {
    #[arg(long, default_value = ".env", global = true)]
    env_file: PathBuf,
    /// Log update summaries, API timings and job outcomes; repeat for payloads.
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    verbose: u8,
    /// Include redacted Telegram/Jev request and response bodies in logs.
    #[arg(long, global = true)]
    debug: bool,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Start the local long-polling bot.
    Run,
    /// Validate configuration without making network requests.
    Check,
    /// Remove an existing webhook (preserving updates) and set public commands.
    SetupTelegram,
    /// Send two synthetic samples to Jev using the configured OpenRouter key.
    JevSmoke,
}
#[tokio::main]
async fn main() {
    if let Err(e) = execute(Cli::parse()).await {
        // Error values are normalized codes; never pass raw HTTP errors here.
        Logger::new(LogLevel::Info, &[]).error("command.error", json!({"error":e.to_string()}));
        std::process::exit(1);
    }
}
async fn execute(cli: Cli) -> Result<()> {
    if matches!(cli.command, Command::JevSmoke) {
        let text = std::fs::read_to_string(&cli.env_file).unwrap_or_default();
        let mut vars = parse_env(&text)?;
        vars.extend(std::env::vars());
        let key = vars
            .get("OPENROUTER_API_KEY")
            .cloned()
            .ok_or(Error::Config("OPENROUTER_API_KEY"))?;
        let level = LogLevel::parse(vars.get("LOG_LEVEL").map_or("info", String::as_str))?
            .with_flags(cli.verbose, cli.debug);
        let logger = Arc::new(Logger::new(
            level,
            &[
                &key,
                vars.get("TELEGRAM_BOT_TOKEN").map_or("", String::as_str),
                vars.get("TURNSTILE_SECRET_KEY").map_or("", String::as_str),
            ],
        ));
        let api = Api::new(
            String::new(),
            key,
            vars.get("JEV_MODEL")
                .cloned()
                .unwrap_or_else(|| "typesafe/jev-1.13".into()),
            String::new(),
            logger,
        )?;
        for (name, evidence) in [
            (
                "ordinary-greeting",
                json!({"user":{"id":1,"first_name":"小林","username":"lin_dev"},"via_bot":null,"message":{"text":"大家好，我刚开始学习 Cloudflare Workers，想来群里交流。"}}),
            ),
            (
                "scam-promotion",
                json!({"user":{"id":2,"first_name":"每日稳赚"},"via_bot":{"id":3,"is_bot":true,"first_name":"Demo"},"message":{"text":"投资 USDT 保证每天 30% 收益！先打币解锁提现！","reply_markup":{"inline_keyboard":[[{"text":"立即转账","url":"https://example.invalid/claim"}]]}}}),
            ),
        ] {
            let response = api.classify(&evidence).await?;
            let (p, model) = response.decision?;
            println!(
                "{}",
                json!({"sample":name,"probability":p,"model":model,"usage":response.usage})
            );
        }
        return Ok(());
    }
    let mut config = Config::load(&cli.env_file)?;
    config.log_level = config.log_level.with_flags(cli.verbose, cli.debug);
    let config = Arc::new(config);
    if matches!(cli.command, Command::Check) {
        println!(
            "Configuration valid: {} managed groups. Super admins: {}. Log level: {}. No secrets displayed.",
            config.chats.len(),
            config.super_admins.len(),
            config.log_level.as_str()
        );
        return Ok(());
    }
    let logger = Arc::new(Logger::new(
        config.log_level,
        &[
            &config.telegram_token,
            &config.openrouter_key,
            &config.turnstile_secret,
        ],
    ));
    let api = Arc::new(Api::new(
        config.telegram_token.clone(),
        config.openrouter_key.clone(),
        config.jev_model.clone(),
        config.turnstile_secret.clone(),
        logger.clone(),
    )?);
    if matches!(cli.command, Command::SetupTelegram) {
        let me = api.telegram("getMe", json!({})).await?;
        api.telegram("deleteWebhook", json!({"drop_pending_updates":false}))
            .await?;
        menus::install_public(api.as_ref(), &config).await?;
        println!(
            "Configured @{} for local long polling; pending updates preserved.",
            me["username"].as_str().unwrap_or("unknown")
        );
        return Ok(());
    }
    runtime::run(config, api, logger).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logging_flags_work_before_and_after_subcommands() {
        for (args, expected) in [
            (vec!["bot", "--verbose", "run"], LogLevel::Verbose),
            (vec!["bot", "run", "-vv"], LogLevel::Debug),
            (vec!["bot", "--debug", "jev-smoke"], LogLevel::Debug),
            (vec!["bot", "setup-telegram", "--debug"], LogLevel::Debug),
            (vec!["bot", "check", "-v", "--debug"], LogLevel::Debug),
        ] {
            let cli = Cli::try_parse_from(args).unwrap();
            assert_eq!(LogLevel::Info.with_flags(cli.verbose, cli.debug), expected);
        }
    }
}
