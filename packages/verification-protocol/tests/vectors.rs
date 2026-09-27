use serde_json::{Value, json};
use verification_protocol::Submission;
fn fixture() -> Value {
    serde_json::from_str(include_str!("../fixtures/v2.json")).unwrap()
}
#[test]
fn cross_language_v2_submission_and_bounds() {
    let fixture = fixture();
    let value = fixture["submission"].clone();
    let parsed = Submission::parse(&value.to_string()).unwrap();
    assert_eq!(serde_json::to_value(parsed).unwrap(), value);
    for (key, bad) in [
        ("v", json!(1)),
        ("session", json!("a")),
        ("chat", json!("-01")),
        ("chat", json!("-9007199254740992")),
        ("chat", json!("0")),
        ("token", json!("")),
        ("token", json!("x".repeat(2049))),
        ("token", json!("含中文")),
        ("token", json!("with space")),
        ("extra", json!(true)),
    ] {
        let mut changed = value.clone();
        changed[key] = bad;
        assert!(Submission::parse(&changed.to_string()).is_err());
    }
    assert!(Submission::parse(&" ".repeat(4097)).is_err());
    assert!(Submission::parse(r#"{"receipt":"old-v1-proof"}"#).is_err());
    assert!(Submission::parse(&format!("{{\"v\":2,{}", &value.to_string()[1..])).is_err());
}
