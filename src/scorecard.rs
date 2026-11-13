//! Offline report-level quality and cost accounting, plus explicit route suggestions.
use crate::{
    args::Args,
    config::{Config, Route},
    review, runtime, stats,
};
use anyhow::{Context, Result, bail};
use chrono::{DateTime, NaiveDate, Utc};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

type Key = (String, String);
/// Decoy judgments (agreed + resisted) needed before a sycophancy rate is shown.
pub const SYCOPHANCY_MIN_SAMPLES: u64 = 5;
/// A seat at or above this sycophancy rate is marked low-trust in consensus tables.
pub const LOW_TRUST_SYCOPHANCY: f64 = 0.5;
#[derive(Default)]
struct Cell {
    calls: u64,
    boundary_warnings: u64,
    attributed_boundaries: u64,
    config_boundaries: u64,
    outcomes: BTreeMap<String, u64>,
    outcome_samples: u64,
    rework_items: u64,
    rework_item_samples: u64,
    rework_rounds: u64,
    rework_round_samples: u64,
    defect_tags: BTreeMap<String, u64>,
    failures: BTreeMap<String, u64>,
    timings: Vec<f64>,
    accepted: u64,
    accepted_samples: u64,
    false_positive_samples: u64,
    false_positive: u64,
    precision_accepted: u64,
    precision_false: u64,
    verified: u64,
    canary_hits: u64,
    canary_total: u64,
    canary_samples: u64,
    decoy_agreed: u64,
    decoy_judged: u64,
    decoy_reports: u64,
    external_seconds: f64,
    external_accepted: u64,
    external_samples: u64,
    screen_minutes: f64,
    screen_accepted: u64,
    screen_samples: u64,
    revisions: HashSet<String>,
}
fn add_count(total: &mut u64, value: u64) -> Result<()> {
    *total = total
        .checked_add(value)
        .context("feedback count total exceeds u64")?;
    Ok(())
}
fn ratio(n: f64, d: u64) -> Option<f64> {
    (d > 0).then(|| n / d as f64)
}
fn called(value: &Value) -> bool {
    value["cached"] != true
        && value["model_calls"] != 0
        && !matches!(
            value["status"].as_str(),
            Some(
                "cooldown"
                    | "missing_input"
                    | "missing"
                    | "cache_miss"
                    | "skipped"
                    | "running"
                    | "pending"
                    | "queued"
            )
        )
}
fn success(value: &Value) -> bool {
    stats::execution_success(value)
}
fn id(value: &Value) -> Option<&str> {
    value["attempt_id"].as_str().or(value["attemptId"].as_str())
}
fn key(run: &stats::RunRecords, value: &Value) -> Option<Key> {
    Some((run.manifest["run_id"].as_str()?.into(), id(value)?.into()))
}
fn canonical(mut key: Key, aliases: &HashMap<Key, Key>) -> Key {
    let mut seen = HashSet::new();
    while seen.insert(key.clone()) {
        match aliases.get(&key) {
            Some(next) => key = next.clone(),
            None => break,
        }
    }
    key
}
fn date(args: &Args) -> Result<Option<DateTime<Utc>>> {
    args.get("since")
        .map(|value| {
            let date = NaiveDate::parse_from_str(value, "%Y-%m-%d")
                .context("--since requires YYYY-MM-DD")?;
            Ok(date.and_hms_opt(0, 0, 0).unwrap().and_utc())
        })
        .transpose()
}
fn eligible(
    run: &stats::RunRecords,
    value: &Value,
    args: &Args,
    since: Option<DateTime<Utc>>,
) -> bool {
    if args
        .get("task-type")
        .is_some_and(|t| value["task_type"].as_str() != Some(t))
    {
        return false;
    }
    if let Some(since) = since {
        let time = value["started_at"]
            .as_str()
            .or(run.manifest["started_at"].as_str());
        return time
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .is_some_and(|t| t >= since);
    }
    true
}

pub fn collect(state: &Path, args: &Args) -> Result<Value> {
    let since = date(args)?;
    let (runs, diagnostics) = stats::records(state)?;
    let mut aliases = HashMap::new();
    for run in &runs {
        for value in &run.attempts {
            if value["cached"] == true
                && let (Some(current), Some(r), Some(a)) = (
                    key(run, value),
                    value["reused_from"]["run_id"].as_str(),
                    value["reused_from"]["attempt_id"].as_str(),
                )
            {
                aliases.insert(current, (r.into(), a.into()));
            }
        }
    }
    let mut feedback = HashMap::<Key, Value>::new();
    let mut events = Vec::new();
    let mut invalid_feedback = 0u64;
    let mut seen_events = HashSet::new();
    for run in &runs {
        let path = run.directory.join("feedback.jsonl");
        if !path.exists() {
            continue;
        }
        if !fs::symlink_metadata(&path).is_ok_and(|m| m.is_file() && m.len() <= 32 * 1024 * 1024) {
            invalid_feedback += 1;
            continue;
        }
        for (index, line) in fs::read_to_string(&path)?.lines().enumerate() {
            let Ok(event) = serde_json::from_str::<Value>(line) else {
                invalid_feedback += 1;
                continue;
            };
            let Some(report) = run.manifest["reports"]
                .as_array()
                .and_then(|rows| rows.iter().find(|r| r["attempt_id"] == event["attempt_id"]))
            else {
                invalid_feedback += 1;
                continue;
            };
            let valid = event["run_id"] == run.manifest["run_id"]
                && event["report_sha256"]
                    .as_str()
                    .is_some_and(|h| h.len() == 64)
                && event["report_sha256"] == report["report_sha256"]
                && (report["result"]["complete"] != false
                    || (report["result"]["status"] == "boundary_violation"
                        && report["result"]["summary_published"] == true
                        && success(&report["result"])));
            if !valid {
                invalid_feedback += 1;
                continue;
            }
            // Hash validation is local and only reads the bound complete artifact, never logs.
            let intact = report["path"].as_str().is_some_and(|path| {
                fs::symlink_metadata(path).is_ok_and(|m| m.is_file() && m.len() <= 32 * 1024 * 1024)
                    && fs::read(path)
                        .is_ok_and(|bytes| runtime::sha256(&bytes) == event["report_sha256"])
            });
            if !intact {
                invalid_feedback += 1;
                continue;
            }
            if let Some(event_id) = event["event_id"].as_str()
                && !seen_events.insert(event_id.to_owned())
            {
                continue;
            }
            events.push((
                event["created_at"].as_str().unwrap_or("").to_owned(),
                run.directory.clone(),
                index,
                event,
            ));
        }
    }
    events.sort_by(|a, b| (&a.0, &a.1, a.2).cmp(&(&b.0, &b.1, b.2)));
    for (_, _, _, event) in events {
        let Some(run_id) = event["run_id"].as_str() else {
            continue;
        };
        let Some(attempt) = event["attempt_id"].as_str() else {
            continue;
        };
        let target = canonical((run_id.into(), attempt.into()), &aliases);
        let row = feedback
            .entry(target)
            .or_insert_with(|| json!({"counts":{},"timing":{},"implementation":{}}));
        match event["source"].as_str() {
            Some("agent" | "human") => {
                // A correction replaces only fields explicitly supplied; unknown stays unknown.
                for field in ["accepted", "false-positive", "duplicate", "uncertain"] {
                    if event["counts"][field].is_u64() {
                        row["counts"][field] = event["counts"][field].clone();
                    }
                }
                for field in ["outcome", "rework-items", "rework-rounds", "defect-tags"] {
                    if !event["implementation"][field].is_null() {
                        row["implementation"][field] = event["implementation"][field].clone();
                    }
                }
                for field in ["screen-min", "rework-min"] {
                    if event["timing"][field]
                        .as_f64()
                        .is_some_and(|n| n.is_finite() && n >= 0.0)
                    {
                        row["timing"][field] = event["timing"][field].clone();
                    }
                }
            }
            Some("canary") => {
                if let (Some(h), Some(t)) = (
                    event["canary_hits"].as_u64(),
                    event["canary_total"].as_u64(),
                ) && h <= t
                {
                    row["canary_hits"] = json!(h);
                    row["canary_total"] = json!(t);
                }
                if let (Some(a), Some(r)) = (
                    event["decoy_agreed"].as_u64(),
                    event["decoy_resisted"].as_u64(),
                ) && a + r <= event["decoy_total"].as_u64().unwrap_or(0)
                {
                    row["decoy_agreed"] = json!(a);
                    row["decoy_resisted"] = json!(r);
                }
            }
            _ => {
                invalid_feedback += 1;
            }
        }
    }
    let mut cells = BTreeMap::<(String, Option<String>, Option<String>), Cell>::new();
    for run in &runs {
        for value in &run.attempts {
            if !called(value) || !eligible(run, value, args, since) {
                continue;
            }
            let revision_key = format!(
                "route-revision:{}",
                value["recorded_route"].as_str().unwrap_or("unknown")
            );
            if args
                .get(&revision_key)
                .is_some_and(|r| value["route_revision"].as_str() != Some(r))
            {
                continue;
            }
            let cell = cells
                .entry((
                    value["recorded_route"].as_str().unwrap_or("unknown").into(),
                    value["task_type"].as_str().map(str::to_owned),
                    value["model"].as_str().map(str::to_owned),
                ))
                .or_default();
            cell.calls += 1;
            if let Some(revision) = value["route_revision"].as_str() {
                cell.revisions.insert(revision.into());
            }
            if stats::has_boundary_warnings(value) {
                cell.boundary_warnings += 1;
            }
            if value["attributed_boundary_count"].as_u64().unwrap_or(0) > 0 {
                cell.attributed_boundaries += 1;
            }
            if value["boundary_buckets"]["claude_config"]["count"]
                .as_u64()
                .unwrap_or(0)
                > 0
            {
                cell.config_boundaries += 1;
            }
            if !success(value) && value["status"] != "boundary_violation"
                || (!success(value)
                    && value["execution_status"].is_string()
                    && value["execution_status"] != "ok")
            {
                *cell
                    .failures
                    .entry(
                        value["execution_status"]
                            .as_str()
                            .or(value["status"].as_str())
                            .unwrap_or("unknown")
                            .into(),
                    )
                    .or_default() += 1;
            }
            let seconds = stats::duration(value).map(|ms| ms / 1000.0);
            if let Some(seconds) = seconds {
                cell.timings.push(seconds);
            }
            if !success(value) {
                if let Some(seconds) = seconds {
                    cell.external_seconds += seconds;
                    cell.external_samples += 1;
                }
                continue;
            }
            let Some(feedback) = key(run, value).and_then(|key| feedback.get(&key)) else {
                continue;
            };
            if value["task_type"] == "implement" {
                if let Some(outcome) = feedback["implementation"]["outcome"].as_str() {
                    cell.outcome_samples += 1;
                    *cell.outcomes.entry(outcome.into()).or_default() += 1;
                }
                if let Some(n) = feedback["implementation"]["rework-items"].as_u64() {
                    add_count(&mut cell.rework_items, n)?;
                    cell.rework_item_samples += 1;
                }
                if let Some(n) = feedback["implementation"]["rework-rounds"].as_u64() {
                    add_count(&mut cell.rework_rounds, n)?;
                    cell.rework_round_samples += 1;
                }
                if let Some(tags) = feedback["implementation"]["defect-tags"].as_array() {
                    // Feedback is append-only; older synonyms are merged when read.
                    for tag in tags
                        .iter()
                        .filter_map(Value::as_str)
                        .filter_map(canonical_defect_tag)
                        .collect::<HashSet<_>>()
                    {
                        *cell.defect_tags.entry(tag).or_default() += 1;
                    }
                }
                continue;
            }
            let accepted = feedback["counts"]["accepted"].as_u64();
            let fp = feedback["counts"]["false-positive"].as_u64();
            cell.accepted_samples += u64::from(accepted.is_some());
            cell.false_positive_samples += u64::from(fp.is_some());
            add_count(&mut cell.accepted, accepted.unwrap_or(0))?;
            add_count(&mut cell.false_positive, fp.unwrap_or(0))?;
            if let (Some(a), Some(f)) = (accepted, fp) {
                cell.verified += 1;
                add_count(&mut cell.precision_accepted, a)?;
                add_count(&mut cell.precision_false, f)?;
            }
            if let (Some(a), Some(seconds)) = (accepted, seconds) {
                cell.external_seconds += seconds;
                add_count(&mut cell.external_accepted, a)?;
                cell.external_samples += 1;
            }
            if let (Some(a), Some(minutes)) = (accepted, feedback["timing"]["screen-min"].as_f64())
            {
                cell.screen_minutes += minutes;
                add_count(&mut cell.screen_accepted, a)?;
                cell.screen_samples += 1;
            }
            if let (Some(h), Some(t)) = (
                feedback["canary_hits"].as_u64(),
                feedback["canary_total"].as_u64(),
            ) && t > 0
            {
                add_count(&mut cell.canary_hits, h)?;
                add_count(&mut cell.canary_total, t)?;
                cell.canary_samples += 1;
            }
            if let (Some(a), Some(r)) = (
                feedback["decoy_agreed"].as_u64(),
                feedback["decoy_resisted"].as_u64(),
            ) && a + r > 0
            {
                add_count(&mut cell.decoy_agreed, a)?;
                add_count(&mut cell.decoy_judged, a + r)?;
                cell.decoy_reports += 1;
            }
        }
    }
    let rows = cells.into_iter().map(|((route,task_type,model),mut c)| {
        c.timings.sort_by(f64::total_cmp);
        let n = c.timings.len();
        let median = if n == 0 { None } else if n % 2 == 0 {Some(c.timings[n/2-1]/2.0+c.timings[n/2]/2.0)} else {Some(c.timings[n/2])};
        let p90 = (n > 0).then(||c.timings[(9*n).div_ceil(10)-1]);
        let denominator = c.precision_accepted as f64 + c.precision_false as f64;
        let precision = (denominator > 0.0).then(||c.precision_accepted as f64/denominator);
        let recall = ratio(c.canary_hits as f64,c.canary_total);
        let implementation = task_type.as_deref() == Some("implement");
        let sufficient = c.calls >= 5 && if implementation {c.outcome_samples >= 5 && c.rework_item_samples >= 5} else {c.verified >= 5 && c.canary_samples >= 5};
        // Name what blocks a comparison: usually missing human feedback, not too few calls.
        let need = |label: &str, have: u64, missing: &mut Vec<String>| if have < 5 { missing.push(format!("{label}{have}/5")) };
        let mut missing = Vec::new();
        need("调用", c.calls, &mut missing);
        if implementation {
            need("合入结果", c.outcome_samples, &mut missing);
            need("返工条数", c.rework_item_samples, &mut missing);
        } else {
            need("人工核对", c.verified, &mut missing);
            need("金丝雀", c.canary_samples, &mut missing);
        }
        let failed: u64 = c.failures.values().sum();
        let rates: BTreeMap<_,_> = ["quota","rate_limit","capacity","timeout"].into_iter().map(|s|(s,ratio(*c.failures.get(s).unwrap_or(&0) as f64,c.calls))).collect();
        let mut revisions: Vec<_> = c.revisions.into_iter().collect(); revisions.sort();
        let mut row = json!({"route":route,"task_type":task_type,"model":model,"sample_count":c.calls,"verified_samples":c.verified,
            "assessment":if sufficient && (implementation || (precision.is_some() && recall.is_some())){"可比较"}else{"样本不足"},"assessment_missing":missing,
            "accepted":(c.accepted_samples>0).then_some(c.accepted),"accepted_samples":c.accepted_samples,"false_positive":(c.false_positive_samples>0).then_some(c.false_positive),"false_positive_samples":c.false_positive_samples,"precision":precision,
            "precision_accepted":c.precision_accepted,"precision_false_positive":c.precision_false,
            "canary_hits":c.canary_hits,"canary_total":c.canary_total,"canary_samples":c.canary_samples,"canary_recall":recall,
            "miss_rate":recall.map(|r|1.0-r),
            "decoy_agreed":c.decoy_agreed,"decoy_judged":c.decoy_judged,"decoy_reports":c.decoy_reports,
            "sycophancy_rate":(c.decoy_judged >= SYCOPHANCY_MIN_SAMPLES).then(||c.decoy_agreed as f64/c.decoy_judged as f64),
            "sycophancy_assessment":if c.decoy_judged >= SYCOPHANCY_MIN_SAMPLES {"可比较"} else {"样本不足"},
            "median_seconds":median,"p90_seconds":p90,"timing_samples":n,
            "failure_rate":ratio(failed as f64,c.calls),"failure_rates":rates,"failures":c.failures,
            "external_seconds_per_accepted":ratio(c.external_seconds,c.external_accepted),"external_cost_samples":c.external_samples,"external_cost_accepted":c.external_accepted,
            "host_screen_minutes_per_accepted":ratio(c.screen_minutes,c.screen_accepted),"host_cost_samples":c.screen_samples,"host_cost_accepted":c.screen_accepted,
            "route_revisions":revisions,"boundary_warning_count":c.boundary_warnings,"attributed_boundary_count":c.attributed_boundaries,"claude_config_boundary_count":c.config_boundaries});
        if implementation {
            for key in ["precision", "precision_accepted", "precision_false_positive", "accepted", "accepted_samples", "false_positive", "false_positive_samples", "canary_hits", "canary_total", "canary_samples", "canary_recall", "miss_rate", "external_seconds_per_accepted", "host_screen_minutes_per_accepted", "decoy_agreed", "decoy_judged", "decoy_reports", "sycophancy_rate", "sycophancy_assessment"] {row.as_object_mut().unwrap().remove(key);}
            let merged = *c.outcomes.get("merged").unwrap_or(&0);
            let reworked = *c.outcomes.get("merged-after-rework").unwrap_or(&0);
            row["outcome_samples"] = json!(c.outcome_samples);
            row["outcomes"] = json!(c.outcomes);
            row["merge_rate"] = json!(ratio((merged + reworked) as f64, c.outcome_samples));
            row["merged_after_rework_rate"] = json!(ratio(reworked as f64, c.outcome_samples));
            row["discard_rate"] = json!(ratio(*c.outcomes.get("discarded").unwrap_or(&0) as f64, c.outcome_samples));
            row["mean_rework_items"] = json!(ratio(c.rework_items as f64, c.rework_item_samples));
            row["mean_rework_rounds"] = json!(ratio(c.rework_rounds as f64, c.rework_round_samples));
            row["rework_item_samples"] = json!(c.rework_item_samples);
            row["rework_round_samples"] = json!(c.rework_round_samples);
            row["defect_tags"] = json!(c.defect_tags);
        }
        row
    }).collect::<Vec<_>>();
    Ok(
        json!({"schema_version":1,"since":args.get("since"),"task_type":args.get("task-type"),"diagnostics":diagnostics,"invalid_feedback":invalid_feedback,"cells":rows,
        "decision_rule":"宿主核验成本＋漏报率×返工成本 < 宿主自己做的成本；返工成本和自己做的成本由宿主估计，工具不计算。",
        "accounting":"按真实调用计样本；缓存不重复计分。反馈更正按字段取最后已知值；精度只用 accepted 与 false_positive 均已知的报告。成本只用对应时间和采纳数均已知的配对样本。"}),
    )
}

/// Decoy outcomes per route across task types: (agreed, judged).
pub fn sycophancy_by_route(state: &Path) -> Result<BTreeMap<String, (u64, u64)>> {
    let report = collect(state, &Args::default())?;
    let mut totals = BTreeMap::<String, (u64, u64)>::new();
    for cell in report["cells"].as_array().into_iter().flatten() {
        let (Some(route), Some(agreed), Some(judged)) = (
            cell["route"].as_str(),
            cell["decoy_agreed"].as_u64(),
            cell["decoy_judged"].as_u64(),
        ) else {
            continue;
        };
        let entry = totals.entry(route.to_owned()).or_default();
        entry.0 += agreed;
        entry.1 += judged;
    }
    Ok(totals)
}

pub fn command(args: &Args) -> Result<i32> {
    let state = args
        .get("state-dir")
        .map(PathBuf::from)
        .unwrap_or_else(runtime::state_dir);
    let report = collect(&state, args)?;
    if args.has("json") {
        runtime::stdout_line(format_args!("{}", serde_json::to_string_pretty(&report)?))?;
    } else {
        if report["cells"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["task_type"] != "implement")
        {
            runtime::stdout_line(format_args!(
                "ROUTE\tTASK TYPE\tMODEL\tN\tPRECISION\tCANARY\t附和率\tMEDIAN s\tP90 s\tFAIL RATE (quota/rate/capacity/timeout)\tEXT s/accepted\tHOST min/accepted\tASSESSMENT"
            ))?;
        }
        let metric = |v: &Value| v.as_f64().map_or_else(|| "-".into(), |n| format!("{n:.3}"));
        let sycophancy = |row: &Value| match row["sycophancy_rate"].as_f64() {
            Some(rate) => format!(
                "{rate:.3} ({}/{})",
                row["decoy_agreed"], row["decoy_judged"]
            ),
            None => format!(
                "样本不足 ({}/{SYCOPHANCY_MIN_SAMPLES})",
                row["decoy_judged"]
            ),
        };
        let assessment = |row: &Value| {
            let label = row["assessment"].as_str().unwrap_or("-");
            let missing = row["assessment_missing"].as_array().map(|m| {
                m.iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join("、")
            });
            match missing {
                Some(m) if label == "样本不足" && !m.is_empty() => format!("{label}（缺 {m}）"),
                _ => label.to_owned(),
            }
        };
        for row in report["cells"].as_array().unwrap() {
            if row["task_type"] == "implement" {
                runtime::stdout_line(format_args!(
                    "{}\timplement\tmodel={}\tN={}\tmerge={}\tmerged-after-rework={}\tdiscard={}\trework-items={}\trework-rounds={}\tmedian-s={}\tp90-s={}\tfailure={}\twarnings={}\tattributed={}\tdefects={}\t{}",
                    row["route"].as_str().unwrap(),
                    row["model"].as_str().unwrap_or("unknown"),
                    row["sample_count"],
                    metric(&row["merge_rate"]),
                    metric(&row["merged_after_rework_rate"]),
                    metric(&row["discard_rate"]),
                    metric(&row["mean_rework_items"]),
                    metric(&row["mean_rework_rounds"]),
                    metric(&row["median_seconds"]),
                    metric(&row["p90_seconds"]),
                    metric(&row["failure_rate"]),
                    row["boundary_warning_count"],
                    row["attributed_boundary_count"],
                    row["defect_tags"],
                    assessment(row)
                ))?;
                continue;
            }
            runtime::stdout_line(format_args!(
                "{}\t{}\t{}\t{}\t{}\t{}/{}\t{}\t{}\t{}\t{} ({}/{}/{}/{})\t{}\t{}\t{}",
                row["route"].as_str().unwrap(),
                row["task_type"].as_str().unwrap_or("-"),
                // Cells are keyed by model too; without it same-route rows look duplicated.
                row["model"].as_str().unwrap_or("-"),
                row["sample_count"],
                metric(&row["precision"]),
                row["canary_hits"],
                row["canary_total"],
                sycophancy(row),
                metric(&row["median_seconds"]),
                metric(&row["p90_seconds"]),
                metric(&row["failure_rate"]),
                metric(&row["failure_rates"]["quota"]),
                metric(&row["failure_rates"]["rate_limit"]),
                metric(&row["failure_rates"]["capacity"]),
                metric(&row["failure_rates"]["timeout"]),
                metric(&row["external_seconds_per_accepted"]),
                metric(&row["host_screen_minutes_per_accepted"]),
                assessment(row)
            ))?;
        }
        runtime::stdout_line(format_args!(
            "{}\n{}",
            report["accounting"].as_str().unwrap(),
            report["decision_rule"].as_str().unwrap()
        ))?;
    }
    Ok(0)
}

// The same resolver/fingerprint as execution; no subprocess or model request.
pub(crate) fn revision(route: &Route, resolved: &Value) -> String {
    let resolved = if route.api_alias.is_some() {
        json!({"provider":resolved["provider"],"model":resolved["model"],"url":resolved["url"],"model_source":resolved["model_source"]})
    } else {
        resolved.clone()
    };
    runtime::sha256(&serde_json::to_vec(&json!({"route":route,"resolved":resolved})).unwrap())
}
pub(crate) fn current_revision(
    config: &Config,
    route: &Route,
    args: &Args,
) -> Result<(String, bool)> {
    let (resolved, known) = if let Some(alias) = &route.api_alias {
        (config.alias_metadata(alias, &Args::default())?, true)
    } else {
        review::cli_identity(route, config, args)?
    };
    Ok((revision(route, &resolved), known))
}
fn recommendation(
    config: &Config,
    task: &str,
    jobs: usize,
    text_ready: bool,
    args: &Args,
) -> Result<Value> {
    if jobs == 0 {
        bail!("--jobs must be positive");
    }
    let mut filter = Args::default();
    filter.set("task-type", task);
    let mut current = HashMap::new();
    let eligible = config
        .routes
        .iter()
        .filter(|route| {
            if review::blocked(&route.pool).is_some() || (!text_ready && !route.agentic) {
                return false;
            }
            if (!args.all("only").is_empty() && !args.all("only").contains(&route.id))
                || args.all("exclude").contains(&route.id)
            {
                return false;
            }
            if route.api_alias.is_none() {
                return route.argv.first().is_some_and(|program| {
                    review::resolve_executable(program, Some(config)).is_some()
                });
            }
            true
        })
        .collect::<Vec<_>>();
    let mut ready = Vec::new();
    for route in eligible {
        if let Some(alias) = &route.api_alias {
            let metadata = config.alias_metadata(alias, &Args::default())?;
            if config
                .first_key(
                    config.provider(metadata["provider"].as_str().context("missing provider")?)?,
                )
                .1
                .is_empty()
            {
                continue;
            }
        }
        let (revision, known) = current_revision(config, route, args)?;
        filter.set(&format!("route-revision:{}", route.id), &revision);
        current.insert(route.id.clone(), (revision, known));
        ready.push(route);
    }
    let report = collect(&runtime::state_dir(), &filter)?;
    let mut candidates = Vec::new();
    for route in ready {
        let (revision, known) = &current[&route.id];
        let cell = report["cells"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["route"] == route.id);
        let usable = *known
            && cell.is_some_and(|c| {
                c["assessment"] == "可比较"
                    && c["route_revisions"]
                        .as_array()
                        .is_some_and(|r| r.len() == 1 && r[0].as_str() == Some(revision.as_str()))
            });
        let score = if usable {
            cell.and_then(|c| {
                if task == "implement" {
                    c["merge_rate"].as_f64()
                } else {
                    Some(c["precision"].as_f64()? * c["canary_recall"].as_f64()?)
                }
            })
        } else {
            None
        };
        candidates.push(json!({"route":route.id,"pool":route.pool,"mean_rework_items":cell.and_then(|c|c["mean_rework_items"].as_f64()),"score":score,"failure_rate":cell.map(|c|c["failure_rate"].clone()),"failure_rates":cell.map(|c|c["failure_rates"].clone()),"reason":if score.is_some(){if task == "implement" {"合入率优先、返工条数次序"} else {"精度×召回"}}else{"样本不足或模型/CLI身份已变化"},"jobs":0,"sample_count":cell.and_then(|c|c["sample_count"].as_u64()).unwrap_or(0),"weight":0.0}));
    }
    if candidates.is_empty() {
        bail!("no eligible routes outside cooldown");
    }
    let best = candidates
        .iter()
        .enumerate()
        .filter_map(|(i, r)| r["score"].as_f64().map(|s| (i, s)))
        .max_by(|a, b| {
            a.1.total_cmp(&b.1)
                .then_with(|| {
                    if task == "implement" {
                        candidates[b.0]["mean_rework_items"]
                            .as_f64()
                            .unwrap_or(f64::INFINITY)
                            .total_cmp(
                                &candidates[a.0]["mean_rework_items"]
                                    .as_f64()
                                    .unwrap_or(f64::INFINITY),
                            )
                    } else {
                        std::cmp::Ordering::Equal
                    }
                })
                .then_with(|| b.0.cmp(&a.0))
        })
        .map(|(i, _)| i);
    let exploring = candidates
        .iter()
        .enumerate()
        .filter(|(_, r)| r["score"].is_null())
        .map(|(i, _)| i)
        .collect::<Vec<_>>();
    if let Some(best) = best {
        let exploration = if exploring.is_empty() { 0.0 } else { 0.2 };
        candidates[best]["weight"] = json!(1.0 - exploration);
        for i in &exploring {
            candidates[*i]["weight"] = json!(exploration / exploring.len() as f64);
        }
    } else {
        let n = candidates.len();
        for row in &mut candidates {
            row["weight"] = json!(1.0 / n as f64);
        }
    }
    // Largest remainder, deterministic tie-break; small batches cannot promise exact 20%.
    let mut counts = candidates
        .iter()
        .map(|r| (r["weight"].as_f64().unwrap() * jobs as f64).floor() as usize)
        .collect::<Vec<_>>();
    let mut order: Vec<_> = (0..candidates.len()).collect();
    order.sort_by(|a, b| {
        let remainder =
            |i: usize| candidates[i]["weight"].as_f64().unwrap() * jobs as f64 - counts[i] as f64;
        remainder(*b)
            .total_cmp(&remainder(*a))
            .then_with(|| {
                candidates[*a]["sample_count"]
                    .as_u64()
                    .cmp(&candidates[*b]["sample_count"].as_u64())
            })
            .then(a.cmp(b))
    });
    for i in order.into_iter().take(jobs - counts.iter().sum::<usize>()) {
        counts[i] += 1;
    }
    if jobs < 5 {
        // Carry exploration debt through actual invocation history, so jobs=1 cannot
        // starve an unmeasured route forever merely because 20% rounds to zero.
        counts.fill(0);
        let total: u64 = candidates
            .iter()
            .map(|r| r["sample_count"].as_u64().unwrap())
            .sum();
        for step in 0..jobs {
            let index = (0..candidates.len())
                .filter(|i| candidates[*i]["weight"].as_f64().unwrap() > 0.0)
                .max_by(|a, b| {
                    let deficit = |i: usize| {
                        candidates[i]["weight"].as_f64().unwrap()
                            * (total as f64 + step as f64 + 1.0)
                            - candidates[i]["sample_count"].as_u64().unwrap() as f64
                            - counts[i] as f64
                    };
                    deficit(*a).total_cmp(&deficit(*b)).then_with(|| b.cmp(a))
                })
                .unwrap();
            counts[index] += 1;
        }
    }
    for (row, count) in candidates.iter_mut().zip(counts) {
        row["jobs"] = json!(count);
    }
    Ok(
        json!({"task_type":task,"jobs":jobs,"primary":best.map(|i|candidates[i]["route"].clone()),"routes":candidates,"exploration_fraction":if best.is_some() && !exploring.is_empty(){0.2}else if best.is_none(){1.0}else{0.0},"note":"仅是成绩单建议；5项以上按最大余数取整，小批次按历史样本补探索欠额。无足够实测时均匀探索，不下优劣结论。auto 仍须用户授权候选通路和材料；--only/--exclude 可收窄。"}),
    )
}
pub(crate) fn allocation(
    config: &Config,
    task: &str,
    jobs: usize,
    text_ready: bool,
    args: &Args,
) -> Result<Vec<String>> {
    let report = recommendation(config, task, jobs, text_ready, args)?;
    let mut result = Vec::new();
    for row in report["routes"].as_array().unwrap() {
        result.extend(std::iter::repeat_n(
            row["route"].as_str().unwrap().to_owned(),
            row["jobs"].as_u64().unwrap() as usize,
        ));
    }
    Ok(result)
}
pub fn recommend_command(config: &Config, args: &Args) -> Result<i32> {
    let task = args
        .get("task-type")
        .context("recommend requires --task-type X")?;
    for name in args.all("only").iter().chain(args.all("exclude").iter()) {
        config.route(name, false)?;
    }
    let jobs = usize::try_from(args.number("jobs", 10)?)?;
    let report = recommendation(config, task, jobs, true, args)?;
    if args.has("json") {
        runtime::stdout_line(format_args!("{}", serde_json::to_string_pretty(&report)?))?;
    } else {
        runtime::stdout_line(format_args!("ROUTE\tSHARE\tJOBS\tBASIS"))?;
        for row in report["routes"].as_array().unwrap() {
            runtime::stdout_line(format_args!(
                "{}\t{:.1}%\t{}\t{}",
                row["route"].as_str().unwrap(),
                row["weight"].as_f64().unwrap() * 100.0,
                row["jobs"],
                row["reason"].as_str().unwrap()
            ))?;
        }
        runtime::stdout_line(format_args!("{}", report["note"].as_str().unwrap()))?;
    }
    Ok(0)
}

/// Same defect, different wording or language, observed across real feedback rounds.
/// Only clear synonyms are merged; distinct defects stay distinct. "无" means no defect.
const DEFECT_TAG_ALIASES: &[(&str, &str)] = &[
    ("criterion-duplicated", "判据写两份"),
    ("sort-tiebreak", "排序无兜底"),
    ("browser-only", "单测过浏览器才抓到"),
    ("id-collision", "撞号"),
    ("performance", "性能未达标"),
    ("comment-claims-wrong", "注释与代码不符"),
    ("over-truncation", "截断过度"),
    ("premature-discard", "过早丢弃"),
    ("registry-docs", "登记表未同步"),
    ("scope-creep-card-edit", "越界改任务卡"),
];
pub fn canonical_defect_tag(tag: &str) -> Option<String> {
    let tag = tag.trim();
    if tag.is_empty() || ["无", "none", "-"].contains(&tag.to_lowercase().as_str()) {
        return None;
    }
    let lower = tag.to_lowercase();
    Some(
        DEFECT_TAG_ALIASES
            .iter()
            .find(|(alias, _)| *alias == lower)
            .map_or(tag, |(_, canonical)| canonical)
            .to_owned(),
    )
}
