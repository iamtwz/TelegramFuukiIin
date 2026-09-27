use crate::{
    api::Services,
    config::Config,
    engine::Engine,
    error::{Error, Result},
    logging::{LogLevel, Logger},
    store::Store,
};
use serde_json::json;
use std::{
    fs::File,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::watch;
pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
pub fn instance_lock(config: &Config) -> Result<File> {
    let parent = config
        .database
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(std::path::Path::new("."));
    std::fs::create_dir_all(parent)?;
    let path = config.database.with_extension("instance.lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    file.try_lock()
        .map_err(|_| Error::Config("another_bot_instance_uses_this_database"))?;
    Ok(file)
}
pub async fn run<S: Services + 'static>(
    config: Arc<Config>,
    services: Arc<S>,
    logger: Arc<Logger>,
) -> Result<()> {
    logger.event(
        LogLevel::Info,
        "bot.starting",
        json!({"version":crate::VERSION,"log_level":logger.level().as_str(),"managed_groups":config.chats.len()}),
    );
    let store = Arc::new(Store::open(&config.database)?);
    let _lock = instance_lock(&config)?;
    let info = services.telegram("getWebhookInfo", json!({})).await?;
    if info["url"].as_str().is_some_and(|s| !s.is_empty()) {
        return Err(Error::Config("webhook_exists_run_setup_telegram_first"));
    }
    let me = services.telegram("getMe", json!({})).await?;
    if me["username"]
        .as_str()
        .is_none_or(|s| !s.eq_ignore_ascii_case(&config.bot_username))
    {
        return Err(Error::Config("bot_username_does_not_match_token"));
    }
    store.recover(now())?;
    let engine = Engine {
        store: store.clone(),
        config,
        services: services.clone(),
        clock: Arc::new(now),
        logger: logger.clone(),
    };
    for chat in &engine.config.chats {
        store.start_statistics(*chat, engine.now())?;
    }
    crate::menus::install_public(services.as_ref(), &engine.config).await?;
    crate::menus::refresh(&engine).await?;
    let (stop_tx, stop_rx) = watch::channel(false);
    let polling = async {
        let mut stop = stop_rx.clone();
        loop {
            if *stop.borrow() {
                break;
            }
            let fetch=services.telegram("getUpdates",json!({"offset":store.offset()?,"timeout":25,"limit":100,"allowed_updates":["message","chat_member","chat_join_request","callback_query"]}));
            let response = tokio::select! {_ = stop.changed()=>break,r=fetch=>r};
            match response {
                Ok(data) => store.persist_updates(
                    data.as_array()
                        .ok_or(Error::Config("invalid_updates_response"))?,
                    now(),
                )?,
                Err(error) => {
                    logger.error("poll.error", json!({"error":error.to_string(),"retry_after_seconds":error.retry_after().max(3)}));
                    let delay = error.retry_after().max(3);
                    tokio::select! {_=stop.changed()=>break,_=tokio::time::sleep(Duration::from_secs(delay))=>{}}
                }
            }
        }
        Ok::<_, Error>(())
    };
    let working = async {
        let mut stop = stop_rx.clone();
        let mut cleanup = now() + 60;
        loop {
            if *stop.borrow() {
                break;
            }
            if now() >= cleanup {
                engine.store.prune(now())?;
                crate::menus::refresh(&engine).await?;
                cleanup = now() + 60;
            }
            if let Some(job) = engine.store.claim(now())? {
                engine.run(&job).await?;
            } else {
                tokio::select! {_=stop.changed()=>break,_=tokio::time::sleep(Duration::from_millis(200))=>{}}
            }
        }
        Ok::<_, Error>(())
    };
    let shutdown = async {
        #[cfg(unix)]
        {
            let mut term =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
            tokio::select! {r=tokio::signal::ctrl_c()=>r?,_=term.recv()=>{}}
        }
        #[cfg(not(unix))]
        tokio::signal::ctrl_c().await?;
        let _ = stop_tx.send(true);
        Ok::<_, Error>(())
    };
    logger.event(
        LogLevel::Info,
        "bot.started",
        json!({"mode":"long_polling"}),
    );
    tokio::try_join!(polling, working, shutdown)?;
    logger.event(LogLevel::Info, "bot.stopped", json!({}));
    Ok(())
}
