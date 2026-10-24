use multi_ai::{api::model_drift, config::Config};
use serde_json::{Value, json};
use std::fs;

/// One test per binary: the process environment is pointed at a private HOME before
/// Config::load, so no user config or credentials are read.
#[test]
fn drift_reports_retired_ids_and_ignores_unused_providers() {
    let temp = tempfile::tempdir().unwrap();
    let config_dir = temp.path().join("config");
    fs::create_dir_all(&config_dir).unwrap();
    let mut profiles: Value =
        serde_json::from_str(include_str!("../config/profiles.json")).unwrap();
    profiles["providers"]["deepseek"]["models"]["fast"] = json!("deepseek-retired");
    fs::write(
        config_dir.join("profiles.json"),
        serde_json::to_vec(&profiles).unwrap(),
    )
    .unwrap();
    unsafe {
        std::env::set_var("HOME", temp.path());
        std::env::set_var("MULTI_AI_CONFIG_DIR", &config_dir);
    }
    std::env::set_current_dir(temp.path()).unwrap();
    let config = Config::load().unwrap();
    let listed = json!({"providers":{
        "deepseek":{"ok":true,"models":["deepseek-flash","deepseek-v4-pro","deepseek-new"]},
        "kimi":{"ok":true,"models":["kimi-k3","kimi-k3.1-pro","kimi-k3.1-pro-highspeed","kimi-k2.7-code","kimi-k2.7-code-highspeed"]},
        "glm":{"ok":false,"error":"missing ZHIPU_API_KEY"}
    }});
    let drift = model_drift(&config, &listed);
    let deepseek = &drift["providers"]["deepseek"];
    assert_eq!(
        deepseek["missing"]["deepseek-retired"],
        json!(["providers.deepseek.models.fast"])
    );
    assert_eq!(deepseek["unreferenced"], json!(["deepseek-new"]));
    // The retired moonshot-v1 provider is no longer selected, so its ids are not checked.
    assert_eq!(drift["providers"]["kimi"]["missing"], json!({}));
    assert_eq!(drift["providers"]["glm"]["checked"], false);
}
