//! Offline agreement table across review reports (no model calls).
//! Findings are aligned by `path:line` (path suffix, line ±3), the same text matching the
//! canaries use; it is not a semantic judge. Disagreements are listed first: opposite
//! verdicts on one location are what the host should check before anything else.
use crate::{args::Args, canary, runtime};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{collections::BTreeSet, fs, path::Path};

const LINE_TOLERANCE: u64 = 3;

struct Finding {
    seat: String,
    path: String,
    line: u64,
    verdict: Option<bool>,
    summary: String,
}

fn summary(scope: &str) -> String {
    let text = scope
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .trim_matches('|')
        .trim();
    text.chars().take(120).collect()
}
fn same_path(a: &str, b: &str) -> bool {
    a == b || a.ends_with(&format!("/{b}")) || b.ends_with(&format!("/{a}"))
}

pub fn command(args: &Args) -> Result<i32> {
    if args.positional.len() < 2 {
        bail!("consensus requires at least two report files");
    }
    let mut findings = Vec::new();
    let mut unaligned = Vec::new();
    let mut seats = Vec::new();
    for file in &args.positional {
        let path = Path::new(file);
        let text = fs::read_to_string(path).with_context(|| format!("read {file}"))?;
        let seat = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(file)
            .trim_end_matches(".md")
            .to_owned();
        if seats.contains(&seat) {
            bail!("duplicate report name: {seat}");
        }
        seats.push(seat.clone());
        for scope in canary::finding_scopes(&text) {
            let verdict = canary::positive(&scope);
            let mut located = false;
            let mut seen = BTreeSet::new();
            for capture in canary::LOCATIONS.captures_iter(&scope) {
                let Ok(line) = capture["line"].parse::<u64>() else {
                    continue;
                };
                let path = capture["path"]
                    .replace('\\', "/")
                    .trim_start_matches("./")
                    .to_owned();
                if !seen.insert((path.clone(), line)) {
                    continue;
                }
                located = true;
                findings.push(Finding {
                    seat: seat.clone(),
                    path,
                    line,
                    verdict,
                    summary: summary(&scope),
                });
            }
            // Unlocated prose without a verdict is narration, not a finding.
            if !located && verdict.is_some() {
                unaligned.push(json!({"seat":seat,"summary":summary(&scope)}));
            }
        }
    }
    // Greedy clustering on sorted locations; a finding joins the first matching cluster.
    findings.sort_by(|a, b| (&a.path, a.line).cmp(&(&b.path, b.line)));
    let mut clusters: Vec<Vec<Finding>> = Vec::new();
    for finding in findings {
        match clusters.iter_mut().find(|c| {
            c.iter().any(|f| {
                same_path(&f.path, &finding.path) && f.line.abs_diff(finding.line) <= LINE_TOLERANCE
            })
        }) {
            Some(cluster) => cluster.push(finding),
            None => clusters.push(vec![finding]),
        }
    }
    let total = seats.len();
    let mut rows: Vec<Value> = clusters
        .iter()
        .map(|cluster| {
            let mut by_seat = std::collections::BTreeMap::<&str, Option<bool>>::new();
            for f in cluster {
                let entry = by_seat.entry(&f.seat).or_insert(None);
                if f.verdict.is_some() {
                    *entry = f.verdict;
                }
            }
            let verdicts: BTreeSet<bool> = by_seat.values().flatten().copied().collect();
            let kind = if verdicts.len() > 1 {
                "结论相反"
            } else if by_seat.len() == 1 {
                "仅一家"
            } else if by_seat.len() == total {
                "全部提到"
            } else {
                "多家提到"
            };
            let first = &cluster[0];
            json!({"kind":kind,"location":format!("{}:{}", first.path, first.line),
                "seats":by_seat.keys().collect::<Vec<_>>(),
                "verdicts":by_seat.iter().map(|(s,v)|(s.to_string(),json!(v))).collect::<serde_json::Map<_,_>>(),
                "summary":first.summary})
        })
        .collect();
    let order = |k: &str| match k {
        "结论相反" => 0,
        "仅一家" => 1,
        "多家提到" => 2,
        _ => 3,
    };
    rows.sort_by_key(|r| order(r["kind"].as_str().unwrap_or("")));
    let report = json!({"seats":seats,"rows":rows,"unaligned":unaligned,
        "note":"按 path:line（路径后缀、行号±3）文本对齐，不做语义判断；未写位置的发现列在 unaligned。"});
    let text = if args.has("json") {
        serde_json::to_string_pretty(&report)?
    } else {
        markdown(&report)
    };
    match args.get("out") {
        Some(out) => fs::write(out, text + "\n")?,
        None => runtime::stdout_line(format_args!("{text}"))?,
    }
    Ok(0)
}

fn markdown(report: &Value) -> String {
    let verdict = |v: &Value| match v.as_bool() {
        Some(true) => "成立",
        Some(false) => "不成立",
        None => "未判",
    };
    let mut out = format!(
        "# 复核一致/分歧对照\n\n席位：{}\n\n| 类别 | 位置 | 各家结论 | 摘要 |\n|---|---|---|---|\n",
        report["seats"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join("、")
    );
    for row in report["rows"].as_array().unwrap() {
        let verdicts = row["verdicts"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(seat, v)| format!("{seat}:{}", verdict(v)))
            .collect::<Vec<_>>()
            .join("；");
        out.push_str(&format!(
            "| {} | `{}` | {} | {} |\n",
            row["kind"].as_str().unwrap(),
            row["location"].as_str().unwrap(),
            verdicts,
            row["summary"].as_str().unwrap().replace('|', "\\|")
        ));
    }
    let unaligned = report["unaligned"].as_array().unwrap();
    if !unaligned.is_empty() {
        out.push_str("\n## 未对齐（没写 文件:行号）\n\n");
        for item in unaligned {
            out.push_str(&format!(
                "- {}：{}\n",
                item["seat"].as_str().unwrap(),
                item["summary"].as_str().unwrap()
            ));
        }
    }
    out.push_str(&format!("\n{}\n", report["note"].as_str().unwrap()));
    out
}
