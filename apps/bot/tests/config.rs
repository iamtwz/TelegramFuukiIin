use fuuki_iin_bot::config::{Config, parse_env};
use std::collections::HashMap;

fn base() -> HashMap<String, String> {
    parse_env("TELEGRAM_BOT_TOKEN=123456:synthetic\nBOT_USERNAME=test_bot\nOPENROUTER_API_KEY=synthetic\nTURNSTILE_SITE_KEY=synthetic\nTURNSTILE_SECRET_KEY=synthetic\nVERIFICATION_BASE_URL=https://verify.example.com\n").unwrap()
}

#[test]
fn managed_groups_can_be_missing_empty_or_whitespace() {
    let mut vars = base();
    assert!(Config::from_map(&vars).unwrap().chats.is_empty());
    for value in ["", " ", "\t\n"] {
        vars.insert("MANAGED_CHAT_IDS".into(), value.into());
        assert!(Config::from_map(&vars).unwrap().chats.is_empty());
    }
    vars.insert("MANAGED_CHAT_IDS".into(), " -100123, -456, -100123 ".into());
    assert_eq!(Config::from_map(&vars).unwrap().chats, [-100123, -456]);
}

#[test]
fn nonempty_managed_groups_still_reject_malformed_entries() {
    let mut vars = base();
    for value in [
        ",",
        "-123,",
        ",-123",
        "-123, , -456",
        "0",
        "123",
        "all",
        "-9007199254740992",
    ] {
        vars.insert("MANAGED_CHAT_IDS".into(), value.into());
        assert!(Config::from_map(&vars).is_err(), "accepted {value}");
    }
}

#[test]
fn super_admins_default_to_none_and_match_only_configured_ids() {
    let mut vars = base();
    assert!(Config::from_map(&vars).unwrap().super_admins.is_empty());
    for value in ["", " \t "] {
        vars.insert("SUPER_ADMIN_IDS".into(), value.into());
        assert!(Config::from_map(&vars).unwrap().super_admins.is_empty());
    }
    vars.insert("SUPER_ADMIN_IDS".into(), " 42, 123456789, 42 ".into());
    let config = Config::from_map(&vars).unwrap();
    assert_eq!(config.super_admins, [42, 123456789]);
    assert!(config.is_super_admin(42));
    assert!(config.is_super_admin(123456789));
    assert!(!config.is_super_admin(43));
    assert!(!config.is_super_admin(0));
}

#[test]
fn malformed_super_admin_ids_fail_configuration() {
    let mut vars = base();
    for value in [
        "0",
        "-42",
        "+42",
        "@admin",
        "all",
        "42,",
        ",42",
        "42, ,7",
        "42,typo",
        "9007199254740992",
        "9223372036854775808",
    ] {
        vars.insert("SUPER_ADMIN_IDS".into(), value.into());
        assert!(Config::from_map(&vars).is_err(), "accepted {value}");
    }
}
