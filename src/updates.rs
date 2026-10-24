//! doctor --updates: installed vendor CLI versions against their published latest version.
//! Network lookups are best effort; an unknown latest version is reported, never guessed.
use crate::{config::Config, delegate_cli};
use anyhow::Result;
use regex::Regex;
use serde_json::{Value, json};
use std::{path::PathBuf, process::Stdio, time::Duration};
use tokio::process::Command;

async fn output(config: &Config, program: &str, args: &[&str]) -> Option<String> {
    let mut command = Command::new(program);
    command
        .args(args)
        .env_clear()
        .envs(config.environment())
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let out = tokio::time::timeout(Duration::from_secs(30), command.output())
        .await
        .ok()?
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

fn semver(text: &str) -> Option<String> {
    Regex::new(r"\d+\.\d+\.\d+")
        .unwrap()
        .find(text)
        .map(|m| m.as_str().to_owned())
}

/// Kimi has no check-only flag; its own updater caches the CDN manifest locally.
fn kimi_cached_latest(config: &Config) -> Option<String> {
    let env = config.environment();
    let home = env
        .get("KIMI_CODE_HOME")
        .map(PathBuf::from)
        .or_else(|| env.get("HOME").map(|h| PathBuf::from(h).join(".kimi-code")))?;
    let text = std::fs::read_to_string(home.join("updates/latest.json")).ok()?;
    serde_json::from_str::<Value>(&text).ok()?["latest"]
        .as_str()
        .map(str::to_owned)
}

async fn latest(config: &Config, vendor: &str) -> (Option<String>, &'static str) {
    match vendor {
        "claude" => (
            output(
                config,
                "npm",
                &["view", "@anthropic-ai/claude-code", "version"],
            )
            .await,
            "npm",
        ),
        "codex" => (
            output(config, "npm", &["view", "@openai/codex", "version"]).await,
            "npm",
        ),
        "grok" => (
            output(config, "grok", &["update", "--check", "--json"])
                .await
                .and_then(|text| serde_json::from_str::<Value>(&text).ok())
                .and_then(|v| v["latestVersion"].as_str().map(str::to_owned)),
            "grok update --check",
        ),
        "kimi" => (kimi_cached_latest(config), "kimi 本地更新缓存"),
        _ => (None, "未知"),
    }
}

pub async fn command(config: &Config, json_output: bool) -> Result<i32> {
    let mut rows = vec![];
    for vendor in ["claude", "codex", "kimi", "grok", "agy"] {
        let installed = output(config, vendor, &["--version"])
            .await
            .and_then(|v| semver(&v));
        let (latest, source) = if installed.is_some() {
            latest(config, vendor).await
        } else {
            (None, "未安装")
        };
        let validated = match vendor {
            "kimi" => semver(delegate_cli::KIMI_VERSION),
            "grok" => semver(delegate_cli::GROK_VERSION),
            _ => None,
        };
        let update = matches!((&installed, &latest), (Some(a), Some(b)) if a != b);
        rows.push(
            json!({"vendor":vendor,"installed":installed,"latest":latest,"source":source,
            "update_available":update,"delegation_validated":validated,
            "upgrade_needs_revalidation":update && validated.is_some()}),
        );
    }
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({"vendors":rows}))?
        );
        return Ok(0);
    }
    println!("厂商 CLI 版本（latest 查不到时如实留空）：");
    for row in &rows {
        let text = |key: &str| row[key].as_str().unwrap_or("—").to_owned();
        let mark = if row["update_available"] == true {
            "⬆️ "
        } else {
            "  "
        };
        println!(
            "{mark}{:<7} 本机 {:<10} 最新 {:<10} （{}）",
            text("vendor"),
            text("installed"),
            text("latest"),
            text("source")
        );
        if row["upgrade_needs_revalidation"] == true {
            println!(
                "     升级后受限委派会停用，需重新验收（已验收 {}，见 docs/CLI-ISOLATION.md）",
                text("delegation_validated")
            );
        }
    }
    Ok(0)
}
