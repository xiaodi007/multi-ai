//! Offline statistics from durable invocation metadata, never report or log bodies.
use crate::{args::Args, runtime};
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

#[derive(Default)]
struct Totals {
    attempts: u64,
    calls: u64,
    successful: u64,
    cached: u64,
    skipped: u64,
    in_progress: u64,
    failures: BTreeMap<String, u64>,
    boundary_warnings: u64,
    attributed_boundaries: u64,
    timings: Vec<f64>,
    input: u64,
    input_samples: u64,
    output: u64,
    output_samples: u64,
}

fn string<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| value[*key].as_str().filter(|s| !s.is_empty()))
}

fn read_json(path: &Path, invalid: &mut u64) -> Option<Value> {
    // In particular, do not follow a planted metadata symlink to another local file.
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(_) => {
            *invalid += 1;
            return None;
        }
    };
    if !metadata.is_file() || metadata.len() > 32 * 1024 * 1024 {
        *invalid += 1;
        return None;
    }
    match fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .filter(Value::is_object)
    {
        Some(value) => Some(value),
        None => {
            *invalid += 1;
            None
        }
    }
}

fn merge(records: &mut BTreeMap<String, Value>, value: &Value, fallback_id: Option<&str>) {
    let Some(id) = string(value, &["attempt_id", "attemptId"]).or(fallback_id) else {
        return;
    };
    let Some(fields) = value.as_object() else {
        return;
    };
    let record = records.entry(id.into()).or_insert_with(|| json!({}));
    for (key, item) in fields {
        if !item.is_null() {
            record[key] = item.clone();
        }
    }
}

fn merge_tree(records: &mut BTreeMap<String, Value>, value: &Value, fallback_id: Option<&str>) {
    merge(records, value, fallback_id);
    if let Some(result) = value.get("result") {
        merge_tree(records, result, None);
    }
    // Retries and quota failover retain each original attempt in these envelopes.
    // Visit inner attempts last: their status describes the call rather than policy backoff.
    for key in ["results", "route_attempts", "attempts"] {
        if let Some(children) = value[key].as_array() {
            for child in children {
                merge_tree(records, child, None);
            }
        }
    }
}

pub(crate) fn route(value: &Value, command: &str) -> String {
    if let Some(name) = string(value, &["route", "target"]) {
        return name.into();
    }
    if let Some(alias) = command.strip_prefix("text:")
        && !["run", "dispatch", "roundtable"].contains(&alias)
    {
        return format!("text:{alias}");
    }
    if let Some(label) = string(value, &["label"])
        && ![
            "run",
            "dispatch",
            "review",
            "verify",
            "roundtable",
            "request",
        ]
        .contains(&label)
    {
        return label.into();
    }
    if let Some(provider) = string(value, &["provider", "providerName"]) {
        return format!(
            "text:{provider}/{}",
            string(value, &["model"]).unwrap_or("unknown")
        );
    }
    "unknown".into()
}

pub(crate) fn duration(value: &Value) -> Option<f64> {
    [
        value["totalMs"].as_f64(),
        value["timings"]["totalMs"].as_f64(),
        value["seconds"].as_f64().map(|n| n * 1000.0),
    ]
    .into_iter()
    .flatten()
    .find(|n| n.is_finite() && *n >= 0.0)
}

fn input_bytes(value: &Value) -> Option<u64> {
    value["inputBytes"]
        .as_u64()
        .or_else(|| value["input_bytes"].as_u64())
}

fn output_bytes(value: &Value) -> Option<u64> {
    value["outputBytes"]
        .as_u64()
        .or_else(|| value["output_bytes"].as_u64())
        .or_else(|| {
            [
                &value["output"],
                &value["content"],
                &value["response"]["content"],
            ]
            .into_iter()
            .find_map(|v| v.as_str().map(|s| s.len() as u64))
        })
}

pub(crate) fn execution_success(value: &Value) -> bool {
    if let Some(status) = value["execution_status"].as_str() {
        return status == "ok"
            && value["response"]["complete"] == true
            && value["response"]["exitCode"] == 0;
    }
    matches!(value["status"].as_str(), Some("ok" | "complete")) && value["complete"] != false
}
pub(crate) fn has_boundary_warnings(value: &Value) -> bool {
    value["boundary_warnings"]
        .as_object()
        .is_some_and(|b| b.values().any(|v| v["count"].as_u64().unwrap_or(0) > 0))
}

impl Totals {
    fn add(&mut self, value: &Value) {
        self.attempts += 1;
        let status = string(value, &["status"]).unwrap_or_else(|| {
            if value["complete"] == true {
                "complete"
            } else {
                "unknown"
            }
        });
        if value["cached"] == true {
            self.cached += 1;
            return;
        }
        if value["model_calls"] == 0
            || [
                "cooldown",
                "missing_input",
                "missing",
                "cache_miss",
                "skipped",
            ]
            .contains(&status)
        {
            self.skipped += 1;
            return;
        }
        if ["running", "pending", "queued"].contains(&status) {
            self.in_progress += 1;
            return;
        }
        self.calls += 1;
        self.boundary_warnings += u64::from(has_boundary_warnings(value));
        self.attributed_boundaries +=
            u64::from(value["attributed_boundary_count"].as_u64().unwrap_or(0) > 0);
        if execution_success(value) {
            self.successful += 1;
        } else {
            *self.failures.entry(status.into()).or_default() += 1;
        }
        if let Some(milliseconds) = duration(value) {
            self.timings.push(milliseconds);
        }
        if let Some(bytes) = input_bytes(value) {
            self.input = self.input.saturating_add(bytes);
            self.input_samples += 1;
        }
        if let Some(bytes) = output_bytes(value) {
            self.output = self.output.saturating_add(bytes);
            self.output_samples += 1;
        }
    }

    fn value(mut self, route: String) -> Value {
        self.timings.sort_by(f64::total_cmp);
        let n = self.timings.len();
        let median = if n == 0 {
            None
        } else if n % 2 == 0 {
            Some(self.timings[n / 2 - 1] / 2.0 + self.timings[n / 2] / 2.0)
        } else {
            Some(self.timings[n / 2])
        };
        let p90 = (n > 0).then(|| self.timings[(9 * n).div_ceil(10) - 1]);
        json!({
            "route":route,"attempts":self.attempts,"calls":self.calls,"successful":self.successful,
            "success_rate_pct":(self.calls > 0).then(|| self.successful as f64 * 100.0 / self.calls as f64),
            "median_ms":median,"p90_ms":p90,"timing_samples":n,
            "cached":self.cached,"skipped":self.skipped,"in_progress":self.in_progress,
            "failures":self.failures,"boundary_warning_count":self.boundary_warnings,"attributed_boundary_count":self.attributed_boundaries,
            "input_bytes":(self.input_samples > 0).then_some(self.input),"input_bytes_samples":self.input_samples,
            "output_bytes":(self.output_samples > 0).then_some(self.output),"output_bytes_samples":self.output_samples
        })
    }
}

/// Missing metrics stay null; neither current configuration nor report files fill gaps.
pub(crate) struct RunRecords {
    pub directory: PathBuf,
    pub manifest: Value,
    pub attempts: Vec<Value>,
}

pub(crate) fn records(state: &Path) -> Result<(Vec<RunRecords>, Value)> {
    let runs_dir = state.join("runs");
    let entries = match fs::read_dir(&runs_dir) {
        Ok(entries) => Some(entries),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error).context("read local run records"),
    };
    let (mut runs, mut invalid, mut missing_progress, mut attempts) = (0, 0, 0, 0);
    let mut loaded = Vec::new();
    for entry in entries.into_iter().flatten() {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let Some(manifest) = read_json(&entry.path().join("manifest.json"), &mut invalid) else {
            continue;
        };
        runs += 1;
        let command = manifest["command"].as_str().unwrap_or("");
        let mut records = BTreeMap::new();
        if let Some(reports) = manifest["reports"].as_array() {
            for report in reports {
                let mut result = report["result"].clone();
                if result.is_object() {
                    for field in ["cached", "model_calls"] {
                        if result[field].is_null() && !report[field].is_null() {
                            result[field] = report[field].clone();
                        }
                    }
                    merge_tree(&mut records, &result, string(report, &["attempt_id"]));
                }
            }
        }
        let current = &manifest["current_status"];
        merge_tree(&mut records, current, None);
        let progress = manifest["outputs"]["progress"].as_str().map(|path| {
            let path = PathBuf::from(path);
            if path.is_absolute() {
                path
            } else {
                entry.path().join(path)
            }
        });
        let progress_entries = progress.as_ref().and_then(|path| {
            fs::symlink_metadata(path)
                .ok()
                .filter(|metadata| metadata.is_dir())
                .and_then(|_| fs::read_dir(path).ok())
        });
        if let Some(entries) = progress_entries {
            for child in entries {
                let child = child?;
                if !child.file_type()?.is_dir() {
                    continue;
                }
                if let Some(metadata) = read_json(&child.path().join("metadata.json"), &mut invalid)
                {
                    merge(&mut records, &metadata, child.file_name().to_str());
                }
            }
        } else {
            missing_progress += 1;
        }
        attempts += records.len();
        let mut values: Vec<_> = records.into_values().collect();
        for value in &mut values {
            if value["task_type"].is_null() {
                value["task_type"] = manifest["task_type"].clone();
            }
            value["recorded_route"] = json!(route(value, command));
        }
        loaded.push(RunRecords {
            directory: entry.path(),
            manifest,
            attempts: values,
        });
    }
    Ok((
        loaded,
        json!({"schema_version":1,"state_dir":state,"runs":runs,"attempts":attempts,
        "invalid_records":invalid,"missing_progress_runs":missing_progress}),
    ))
}

/// Missing metrics stay null; neither current configuration nor report files fill gaps.
pub fn collect(state: &Path) -> Result<Value> {
    let (runs, mut report) = records(state)?;
    let mut totals = BTreeMap::<String, Totals>::new();
    let mut typed = BTreeMap::<(String, Option<String>), Totals>::new();
    for run in runs {
        for value in run.attempts {
            let route = value["recorded_route"].as_str().unwrap().to_owned();
            let task_type = value["task_type"].as_str().map(str::to_owned);
            totals.entry(route.clone()).or_default().add(&value);
            typed.entry((route, task_type)).or_default().add(&value);
        }
    }
    report["routes"] = json!(
        totals
            .into_iter()
            .map(|(route, totals)| totals.value(route))
            .collect::<Vec<_>>()
    );
    report["task_types"] = json!(
        typed
            .into_iter()
            .map(|((route, task_type), totals)| {
                let mut row = totals.value(route);
                row["task_type"] = json!(task_type);
                row
            })
            .collect::<Vec<_>>()
    );
    Ok(report)
}

fn metric(value: &Value, decimals: usize) -> String {
    value
        .as_f64()
        .map_or_else(|| "-".into(), |n| format!("{n:.decimals$}"))
}

pub fn command(args: &Args) -> Result<i32> {
    let state = args
        .get("state-dir")
        .map(PathBuf::from)
        .unwrap_or_else(runtime::state_dir);
    let mut report = collect(&state)?;
    if let Some(task) = args.get("task-type") {
        report["routes"] = json!(
            report["task_types"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|r| r["task_type"].as_str() == Some(task))
                .cloned()
                .collect::<Vec<_>>()
        );
        report["task_type"] = json!(task);
    }
    if args.has("json") {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "{:<32} {:>6} {:>8} {:>10} {:>10} {:>7} {:>7} {:>12} {:>12} FAILURES",
            "ROUTE",
            "CALLS",
            "OK %",
            "MEDIAN ms",
            "P90 ms",
            "CACHED",
            "SKIPPED",
            "INPUT bytes",
            "OUTPUT bytes"
        );
        for row in report["routes"].as_array().unwrap() {
            let failures = row["failures"]
                .as_object()
                .unwrap()
                .iter()
                .map(|(kind, count)| format!("{kind}={count}"))
                .collect::<Vec<_>>()
                .join(",");
            println!(
                "{:<32} {:>6} {:>8} {:>10} {:>10} {:>7} {:>7} {:>12} {:>12} {}",
                row["route"].as_str().unwrap(),
                row["calls"].to_string(),
                metric(&row["success_rate_pct"], 1),
                metric(&row["median_ms"], 1),
                metric(&row["p90_ms"], 1),
                row["cached"].to_string(),
                row["skipped"].to_string(),
                metric(&row["input_bytes"], 0),
                metric(&row["output_bytes"], 0),
                failures
            );
        }
        println!(
            "runs={} attempts={} invalid_records={} missing_progress_runs={}; 字节为已知样本合计，缺失为 -；缓存/跳过/进行中不计调用与耗时。",
            report["runs"],
            report["attempts"],
            report["invalid_records"],
            report["missing_progress_runs"]
        );
        println!(
            "中位数：偶数取平均；P90：nearest rank。历史 text 别名单列，不按当前配置推测通路。--json 包含每项指标的样本数及进行中数量。"
        );
    }
    Ok(0)
}
