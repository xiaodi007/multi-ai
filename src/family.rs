//! Reviewer independence by model family. A judge rates its own family's output as
//! "satisfied" far more often than an outside judge, so review/verify can exclude the
//! implementer's family: explicitly (`--exclude-family`), from a recorded run
//! (`--author-run`), or by default from the ledger's implement records for the `--cwd`
//! worktree (`--allow-same-family` opts out of that default only).
use crate::{
    args::Args,
    config::{Config, Route},
    runtime, stats,
};
use anyhow::{Result, bail};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

/// Excluded families and the human-readable reasons, printed before any model call.
#[derive(Debug, Default, Clone)]
pub struct FamilyExclusion {
    pub families: BTreeSet<String>,
    pub reasons: Vec<String>,
}

impl FamilyExclusion {
    pub fn contains(&self, family: &str) -> bool {
        self.families.contains(family)
    }
    pub fn list(&self) -> Vec<String> {
        self.families.iter().cloned().collect()
    }
}

fn route_family(routes: &[Route], id: &str) -> Option<String> {
    routes
        .iter()
        .find(|r| r.id == id || r.legacy_id == id)
        .map(|r| r.family.clone())
}

fn collect_families(value: &Value, routes: &[Route], out: &mut BTreeSet<String>) {
    match value {
        Value::Object(map) => {
            if let Some(family) = map.get("family").and_then(Value::as_str)
                && !family.is_empty()
                && family != "unknown"
            {
                out.insert(family.to_owned());
            } else if let Some(route) = map
                .get("route")
                .or_else(|| map.get("target"))
                .and_then(Value::as_str)
                && let Some(family) = route_family(routes, route)
            {
                out.insert(family);
            }
            for (key, item) in map {
                // Earlier attempts and fallbacks are part of who wrote the output.
                if matches!(key.as_str(), "result" | "current_status" | "results")
                    || item.is_array()
                {
                    collect_families(item, routes, out);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_families(item, routes, out);
            }
        }
        _ => {}
    }
}

/// Families that produced a recorded run: its manifest reports, status and attempt metadata.
pub fn run_families(state: &Path, run_id: &str, routes: &[Route]) -> Result<BTreeSet<String>> {
    if !runtime::safe_id(run_id) {
        bail!("--author-run 不是合法的运行编号：{run_id}");
    }
    let runs = state.join("runs");
    let exact = runs.join(run_id);
    let dir = if exact.join("manifest.json").is_file() {
        exact
    } else {
        let matches: Vec<PathBuf> = fs::read_dir(&runs)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with(run_id))
            .map(|e| e.path())
            .filter(|p| p.join("manifest.json").is_file())
            .collect();
        match matches.as_slice() {
            [one] => one.clone(),
            [] => bail!("--author-run {run_id}：在 {} 找不到该运行", runs.display()),
            _ => bail!(
                "--author-run {run_id} 匹配到 {} 个运行，请给完整编号",
                matches.len()
            ),
        }
    };
    let (records, _) = stats::records(state)?;
    let mut families = BTreeSet::new();
    if let Some(run) = records.iter().find(|r| r.directory == dir) {
        collect_families(&run.manifest, routes, &mut families);
        for attempt in &run.attempts {
            collect_families(attempt, routes, &mut families);
        }
    }
    if families.is_empty() {
        bail!(
            "--author-run {run_id}：运行记录里没有可识别的通路或家族，无法确定实施方；改用 --exclude-family"
        );
    }
    Ok(families)
}

fn git_toplevel(dir: &Path) -> Option<PathBuf> {
    let mut command = Command::new("git");
    command
        .current_dir(dir)
        .args(["rev-parse", "--show-toplevel"]);
    for (key, _) in env::vars_os() {
        if key.to_string_lossy().starts_with("GIT_") {
            command.env_remove(key);
        }
    }
    let out = command.output().ok().filter(|o| o.status.success())?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    (!text.is_empty()).then(|| PathBuf::from(text))
}

/// The worktree a path belongs to: Git top level when available, else the path itself.
pub fn worktree_of(dir: &Path) -> PathBuf {
    let canonical = fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    git_toplevel(&canonical)
        .and_then(|p| fs::canonicalize(&p).ok())
        .unwrap_or(canonical)
}

/// Families of `delegate implement ROUTE` starts recorded in the ledger for this worktree.
pub fn ledger_implement_families(
    ledger: &Path,
    worktree: &Path,
    routes: &[Route],
) -> BTreeSet<String> {
    let text = fs::read_to_string(ledger).unwrap_or_default();
    let mut families = BTreeSet::new();
    for event in text
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
    {
        if event["event"] != "start" || event["task_type"] != "implement" {
            continue;
        }
        let mut words = event["what"].as_str().unwrap_or("").split_whitespace();
        if words.next() != Some("delegate") || words.next() != Some("implement") {
            continue;
        }
        let Some(route) = words.next() else { continue };
        let Some(cwd) = event["cwd"].as_str() else {
            continue;
        };
        if worktree_of(Path::new(cwd)) != worktree {
            continue;
        }
        if let Some(family) = route_family(routes, route) {
            families.insert(family);
        }
    }
    families
}

/// Build the exclusion for review/verify from flags, an author run and the ledger default.
pub fn resolve(config: &Config, args: &Args) -> Result<FamilyExclusion> {
    let mut result = FamilyExclusion::default();
    let known: BTreeSet<&str> = config.routes.iter().map(|r| r.family.as_str()).collect();
    for family in args.all("exclude-family") {
        for family in family.split(',').map(str::trim).filter(|f| !f.is_empty()) {
            if !known.contains(family) {
                eprintln!("⚠ --exclude-family {family}：没有已配置通路属于该家族");
            }
            result.families.insert(family.to_owned());
        }
    }
    if !result.families.is_empty() {
        result
            .reasons
            .push(format!("--exclude-family {}", result.list().join(",")));
    }
    if let Some(run) = args.get("author-run") {
        let families = run_families(&runtime::state_dir(), run, &config.routes)?;
        result.reasons.push(format!(
            "--author-run {run} 的实施方家族 {}",
            families.iter().cloned().collect::<Vec<_>>().join(",")
        ));
        result.families.extend(families);
    }
    if let Some(cwd) = args.get("cwd")
        && let Some(ledger) = crate::ledger::ledger_file()
    {
        let worktree = worktree_of(Path::new(cwd));
        let families = ledger_implement_families(&ledger, &worktree, &config.routes);
        if !families.is_empty() {
            let names = families.iter().cloned().collect::<Vec<_>>().join(",");
            if args.has("allow-same-family") {
                eprintln!(
                    "⚠ 账本显示 {} 由 {names} 家族实施；--allow-same-family 已放开同家族评审",
                    worktree.display()
                );
            } else {
                result.reasons.push(format!(
                    "账本显示 {} 由 {names} 家族实施（--allow-same-family 可放开）",
                    worktree.display()
                ));
                result.families.extend(families);
            }
        }
    }
    if !result.families.is_empty() {
        eprintln!(
            "排除模型家族 {}：{}",
            result.list().join(","),
            result.reasons.join("；")
        );
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn routes() -> Vec<Route> {
        serde_json::from_value(json!([
            {"id":"impl-cli","legacy_id":"impl","pool":"p1","family":"openai","label":"x","agentic":true,"cost":"sub","rank":1,"argv":["/bin/true"],"extract":"stdout","default_timeout":5},
            {"id":"other-cli","legacy_id":"other","pool":"p2","family":"kimi","label":"x","agentic":true,"cost":"sub","rank":2,"argv":["/bin/true"],"extract":"stdout","default_timeout":5}
        ]))
        .unwrap()
    }

    #[test]
    fn ledger_default_matches_worktree_and_implement_only() {
        let temp = tempfile::tempdir().unwrap();
        let tree = temp.path().join("tree");
        let other = temp.path().join("other");
        fs::create_dir_all(tree.join("sub")).unwrap();
        fs::create_dir_all(&other).unwrap();
        let ledger = temp.path().join("events.jsonl");
        let lines = [
            json!({"event":"start","what":"delegate implement impl-cli","task_type":"implement","cwd":tree}),
            json!({"event":"start","what":"review","task_type":"review","cwd":tree}),
            json!({"event":"start","what":"delegate implement other-cli","task_type":"implement","cwd":other}),
        ];
        fs::write(
            &ledger,
            lines
                .iter()
                .map(|l| l.to_string())
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .unwrap();
        let routes = routes();
        let families = ledger_implement_families(&ledger, &worktree_of(&tree), &routes);
        assert_eq!(families.into_iter().collect::<Vec<_>>(), ["openai"]);
        assert!(
            ledger_implement_families(&ledger, &worktree_of(&tree.join("nope")), &routes)
                .is_empty()
        );
    }

    #[test]
    fn author_run_reads_status_and_falls_back_to_route() {
        let temp = tempfile::tempdir().unwrap();
        let run = temp.path().join("runs/abc-123");
        fs::create_dir_all(&run).unwrap();
        fs::write(
            run.join("manifest.json"),
            json!({"run_id":"abc-123","command":"implement","reports":[],"current_status":{"route":"impl-cli","status":"ok"},"outputs":{}}).to_string(),
        )
        .unwrap();
        let routes = routes();
        let families = run_families(temp.path(), "abc", &routes).unwrap();
        assert_eq!(families.into_iter().collect::<Vec<_>>(), ["openai"]);
        assert!(run_families(temp.path(), "zzz", &routes).is_err());
        assert!(run_families(temp.path(), "../x", &routes).is_err());
    }
}
