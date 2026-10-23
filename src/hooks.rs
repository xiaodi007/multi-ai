use crate::args::Args;
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::{os::fd::AsFd, path::Path};

#[derive(Deserialize)]
struct Hooks {
    #[serde(default)]
    pre_delegate_implement: Vec<Hook>,
}
#[derive(Deserialize)]
struct Hook {
    cmd: Vec<String>,
    #[serde(default)]
    required: bool,
}

pub(crate) async fn pre_delegate_implement(args: &Args) -> Result<()> {
    let cwd = Path::new(args.get("cwd").unwrap_or("."));
    let path = cwd.join(".multi-ai/hooks.json");
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| format!("read pre-hook config {}", path.display()));
        }
    };
    let hooks: Hooks = serde_json::from_slice(&bytes)
        .with_context(|| format!("invalid pre-hook config {}", path.display()))?;
    for hook in hooks.pre_delegate_implement {
        let result = async {
            let program = hook.cmd.first().context("empty hook cmd")?;
            let status = tokio::process::Command::new(program)
                .args(&hook.cmd[1..])
                .current_dir(cwd)
                // Hooks cannot see the delegate argv; pass the one switch a preflight must honour.
                .env(
                    "MULTI_AI_ALLOW_DIRTY",
                    if args.has("allow-dirty") { "1" } else { "0" },
                )
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::from(
                    std::io::stderr().as_fd().try_clone_to_owned()?,
                ))
                .stderr(std::process::Stdio::inherit())
                .kill_on_drop(true)
                .status()
                .await?;
            if !status.success() {
                bail!("hook exited with {status}");
            }
            Ok::<_, anyhow::Error>(())
        }
        .await;
        if let Err(error) = result {
            if hook.required {
                bail!(
                    "required pre_delegate_implement hook {:?} failed; dispatch refused: {error:#}",
                    hook.cmd
                );
            }
            eprintln!(
                "multi-ai: warning: optional pre_delegate_implement hook {:?} failed: {error:#}",
                hook.cmd
            );
        }
    }
    Ok(())
}
