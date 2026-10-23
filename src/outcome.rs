//! Conservative git suggestions, never an inference of merged-after-rework.
use crate::{args::Args, runtime};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{fs, path::Path, process::Command};

pub(crate) fn has_outcome(dir: &Path, attempt: &Value, hash: &Value) -> Result<bool> {
    let text = match fs::read_to_string(dir.join("feedback.jsonl")) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e.into()),
    };
    for line in text.lines().filter(|s| !s.trim().is_empty()) {
        // Damaged feedback must not silently turn an existing outcome into a new one.
        let row: Value = serde_json::from_str(line)?;
        if row["attempt_id"] == *attempt
            && row["report_sha256"] == *hash
            && row["implementation"]["outcome"].is_string()
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn git(cwd: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("/usr/bin/git")
        .args([
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .context("git unavailable or worktree missing")?;
    if !out.status.success() {
        bail!(
            "git inspection failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8(out.stdout)?
        .trim_end_matches('\n')
        .to_owned())
}
fn names(text: &str) -> Vec<String> {
    text.split('\0')
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}
fn infer(manifest: &Value, row: &Value, into: &str, evidence: &mut Value) -> Result<&'static str> {
    // Use the immutable per-report status in the manifest, not the reusable output
    // .status.json, which may already belong to a later invocation.
    let status = &row["result"];
    let cwd = Path::new(status["cwd"].as_str().context("missing worktree")?);
    let before = status["git_before"]["head"]
        .as_str()
        .context("missing pre-run HEAD")?;
    let branch = status["git_before"]["branch"]
        .as_str()
        .filter(|s| !s.is_empty())
        .context("missing runtime branch (old or detached record)")?;
    let files = status["changed_files"]
        .as_array()
        .context("missing changed files")?
        .iter()
        .map(|v| {
            v.as_str()
                .map(str::to_owned)
                .context("invalid changed file")
        })
        .collect::<Result<Vec<_>>>()?;
    if files.is_empty() {
        bail!("no attempt file changes to attribute");
    }
    if status["git_after"]["head"] != status["git_before"]["head"]
        || status["git_after"]["branch"]
            .as_str()
            .is_some_and(|b| b != branch)
    {
        bail!("HEAD or branch changed during implementation; attribution ambiguous");
    }
    let target = git(
        cwd,
        &[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{into}^{{commit}}"),
        ],
    )?;
    let base = git(
        cwd,
        &[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{before}^{{commit}}"),
        ],
    )?;
    let changed = names(&git(
        cwd,
        &[
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--name-only",
            "--no-renames",
            "-z",
            &base,
            &target,
            "--",
        ],
    )?);
    let matched: Vec<_> = files
        .iter()
        .filter(|p| changed.contains(p))
        .cloned()
        .collect();
    *evidence = json!({"inference":"git", "ref":into,"target_commit":target,"before_commit":base,
        "cwd":cwd,"branch":branch,"attempt_files":files,"matched_files":matched, "run_started_at":manifest["started_at"]});
    // Enumerating refs distinguishes a missing branch from an inspection failure.
    let full_branch = format!("refs/heads/{branch}");
    let refs = git(
        cwd,
        &[
            "for-each-ref",
            "--format=%(refname) %(objectname)",
            "refs/heads/",
        ],
    )?;
    let tip = refs
        .lines()
        .filter_map(|l| l.split_once(' '))
        .find(|(r, _)| *r == full_branch)
        .map(|(_, h)| h);
    // Lanes delete merged branches and reuse the worktree, so a missing branch is only
    // a discard when none of the attempt paths reached the target either.
    let Some(tip) = tip else {
        if matched.is_empty() {
            evidence["reason"] =
                json!("runtime branch no longer exists and no attempt paths in base..target diff");
            return Ok("discarded");
        }
        evidence["reason"] = json!("runtime branch deleted but attempt paths reached target");
        return Ok("unknown");
    };
    evidence["branch_commit"] = json!(tip);
    let ancestor = git(cwd, &["merge-base", &base, tip])?;
    if ancestor != base {
        bail!("branch no longer descends from pre-run HEAD");
    }
    let commits = git(
        cwd,
        &["log", "--format=%H %ct", &format!("{base}..{tip}"), "--"],
    )?;
    let commits: Vec<_> = commits.lines().collect();
    evidence["branch_commits"] = json!(commits);
    let started = chrono::DateTime::parse_from_rfc3339(
        manifest["started_at"]
            .as_str()
            .context("missing run time")?,
    )?
    .timestamp();
    let mut late = Vec::new();
    for line in &commits {
        let (hash, time) = line.split_once(' ').context("invalid commit record")?;
        // Include the run's second to avoid claiming no later commit at second precision.
        if time.parse::<i64>()? >= started {
            late.push(hash);
        }
    }
    evidence["post_run_commits"] = json!(late);
    // A single host commit containing only attempt paths is the narrow association
    // we can use. Multiple commits or unrelated files may include rework: unknown.
    if commits.len() == 1 && late.len() == 1 && !matched.is_empty() {
        let commit_files = names(&git(
            cwd,
            &[
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--name-only",
                "--no-renames",
                "-z",
                &base,
                tip,
                "--",
            ],
        )?);
        evidence["commit_files"] = json!(commit_files);
        let merged_base = git(cwd, &["merge-base", tip, &target])?;
        if merged_base == tip
            && !commit_files.is_empty()
            && commit_files.iter().all(|p| files.contains(p))
        {
            evidence["reason"] = json!(
                "single host commit is target ancestor and attempt files occur in base..target diff"
            );
            return Ok("merged");
        }
    }
    // A live branch with nothing committed yet may still be under host review: never discard it.
    evidence["reason"] =
        json!("ambiguous association or subsequent commits; rework cannot be inferred");
    Ok("unknown")
}

pub fn command(args: &Args) -> Result<i32> {
    for flag in ["apply", "all"] {
        if args.values.contains_key(flag) {
            bail!("--{flag} is a flag and does not accept a value");
        }
    }
    if args.positional != ["suggest"] {
        bail!("use outcome suggest --run RUN --report ATTEMPT --into REF, or --all --into REF");
    }
    let into = args
        .get("into")
        .filter(|s| !s.is_empty())
        .context("requires --into REF")?;
    if args.has("all") && (args.has("run") || args.has("report")) {
        bail!("--all conflicts with --run/--report");
    }
    let runs = runtime::state_dir().join("runs");
    let dirs = if args.has("all") {
        match fs::read_dir(&runs) {
            Ok(entries) => entries.map(|e| Ok(e?.path())).collect::<Result<Vec<_>>>()?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e.into()),
        }
    } else {
        let run = args.get("run").context("requires --run")?;
        uuid::Uuid::parse_str(run).context("invalid run ID")?;
        let report = args.get("report").context("requires --report")?;
        if !runtime::safe_id(report) {
            bail!("invalid report ID");
        }
        vec![runs.join(run)]
    };
    let mut suggestions = Vec::new();
    for dir in dirs {
        let manifest: Value = match fs::read(dir.join("manifest.json"))
            .map_err(anyhow::Error::from)
            .and_then(|b| Ok(serde_json::from_slice(&b)?))
        {
            Ok(m) => m,
            Err(e) if args.has("all") => {
                suggestions.push(
                    json!({"outcome":"unknown","manifest":dir,"evidence":{"reason":e.to_string()}}),
                );
                continue;
            }
            Err(e) => return Err(e),
        };
        if dir.file_name().and_then(|s| s.to_str()) != manifest["run_id"].as_str() {
            bail!("manifest run identity mismatch: {}", dir.display());
        }
        if manifest["command"] != "implement" {
            continue;
        }
        let mut reports = manifest["reports"]
            .as_array()
            .context("missing reports")?
            .clone();
        let current = &manifest["current_status"];
        if current["route"] == "codex-cli"
            && current["attempt_id"].is_string()
            && !reports
                .iter()
                .any(|r| r["attempt_id"] == current["attempt_id"])
        {
            // Failed/partial attempts may have a status but no registered report.
            // Include them in the scan, but never fabricate a feedback binding.
            reports.push(json!({"attempt_id":current["attempt_id"], "result":current}));
        }
        if let Some(id) = args.get("report")
            && !reports.iter().any(|row| row["attempt_id"] == id)
        {
            bail!("report not found in run");
        }
        for row in &reports {
            if row["result"]["route"] != "codex-cli"
                || args.get("report").is_some_and(|id| row["attempt_id"] != id)
            {
                continue;
            }
            let mut suggestion = json!({"run_id":manifest["run_id"],"attempt_id":row["attempt_id"],"outcome":"unknown","applied":false,"evidence":{}});
            if has_outcome(&dir, &row["attempt_id"], &row["report_sha256"])? {
                continue;
            }
            let integrity = row["path"]
                .as_str()
                .and_then(|path| fs::read(path).ok())
                .is_some_and(|b| row["report_sha256"] == runtime::sha256(&b) && !b.is_empty());
            let mut evidence = json!({"inference":"git", "ref":into});
            let outcome = if !integrity
                || row["result"]["complete"] != true
                || manifest["status"] == "running"
            {
                evidence["reason"] = json!("incomplete or damaged report; no automatic outcome");
                "unknown"
            } else {
                match infer(&manifest, row, into, &mut evidence) {
                    Ok(outcome) => outcome,
                    Err(e) => {
                        evidence["reason"] = json!(e.to_string());
                        "unknown"
                    }
                }
            };
            suggestion["outcome"] = json!(outcome);
            suggestion["evidence"] = evidence;
            if args.has("apply") && outcome != "unknown" {
                let mut feedback = Args::default();
                feedback.set(
                    "run",
                    manifest["run_id"].as_str().context("missing run ID")?,
                );
                feedback.set(
                    "report",
                    row["attempt_id"].as_str().context("missing attempt ID")?,
                );
                feedback.set("source", "agent");
                feedback.set("outcome", outcome);
                feedback.set(
                    "evidence",
                    format!("git inference: {}", suggestion["evidence"]),
                );
                suggestion["applied"] = json!(runtime::apply_outcome(&feedback)?);
            }
            suggestions.push(suggestion);
        }
    }
    if args.has("json") {
        println!("{}", serde_json::to_string_pretty(&suggestions)?);
    } else {
        println!("RUN\tATTEMPT\tOUTCOME\tAPPLIED\tEVIDENCE");
        for row in suggestions {
            println!(
                "{}\t{}\t{}\t{}\t{}",
                row["run_id"].as_str().unwrap_or("?"),
                row["attempt_id"].as_str().unwrap_or("?"),
                row["outcome"].as_str().unwrap_or("unknown"),
                row["applied"],
                row["evidence"]
            );
        }
    }
    Ok(0)
}
