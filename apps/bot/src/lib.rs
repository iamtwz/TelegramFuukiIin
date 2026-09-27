/// Application release version, shared by CLI, chat replies, logs and HTTP requests.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod admin;
mod admin_audit;
pub mod api;
pub mod audit;
pub mod captcha;
pub mod chat_info;
pub mod commands;
pub mod config;
pub mod engine;
pub mod error;
pub mod evidence;
mod guest;
pub mod join_screening;
pub mod logging;
pub mod markdown;
pub mod menus;
pub mod model;
pub mod notifications;
pub mod profiles;
pub mod runtime;
pub mod statistics;
pub mod store;
