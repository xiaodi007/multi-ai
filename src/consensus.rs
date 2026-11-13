//! Offline agreement table across review reports (no model calls).
//! Findings are aligned by `path:line` (path suffix, line ±3), the same text matching the
//! canaries use; it is not a semantic judge. Disagreements are listed first: opposite
//! verdicts on one location are what the host should check before anything else.
//! Each opposite-verdict row is also listed as pending verification: the host adds an
//! executable check (test, command or reproduction) or hands it to the maintainer. There is
//! deliberately no "feed A's answer to B" second round: debate gains come from the vote,
//! and a second round is where one model's error becomes the group's.
use crate::{args::Args, canary, runtime, scorecard};
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
    let pending: Vec<Value> = rows
        .iter()
        .filter(|r| r["kind"] == "结论相反")
        .map(|r| {
            json!({"location":r["location"],"verdicts":r["verdicts"],"summary":r["summary"],
                "verification":null,"result":null,"decided_by":null})
        })
        .collect();
    if !pending.is_empty() {
        eprintln!(
            "{} 处结论相反：每处补一个可执行验证（测试、命令或复现）或交维护者裁决；不要把一方答案交给另一方再讨论。",
            pending.len()
        );
    }
    let trust = seat_trust(&seats, args);
    let report = json!({"seats":seats,"rows":rows,"unaligned":unaligned,"pending_verification":pending,"seat_trust":trust,
        "note":"按 path:line（路径后缀、行号±3）文本对齐，不做语义判断；未写位置的发现列在 unaligned。",
        "pending_rule":"结论相反的行不交模型讨论第二轮：宿主补可执行验证（测试、命令或复现）并记结果，或交维护者裁决。"});
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

/// Sycophancy rate per seat from local decoy canaries; a seat is a report file stem, which
/// `review --outdir` names after the route's legacy id. Missing state means "无记录".
fn seat_trust(seats: &[String], args: &Args) -> Value {
    let state = args
        .get("state-dir")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(runtime::state_dir);
    let rates = match scorecard::sycophancy_by_route(&state) {
        Ok(rates) => rates,
        Err(error) => {
            eprintln!("⚠ 读不到附和率（{}）：{error:#}", state.display());
            Default::default()
        }
    };
    let routes = crate::config::Config::load()
        .map(|c| c.routes)
        .unwrap_or_default();
    let mut trust = serde_json::Map::new();
    for seat in seats {
        let route = routes
            .iter()
            .find(|r| &r.legacy_id == seat || &r.id == seat)
            .map_or_else(|| seat.clone(), |r| r.id.clone());
        let (agreed, judged) = rates.get(&route).copied().unwrap_or((0, 0));
        let rate =
            (judged >= scorecard::SYCOPHANCY_MIN_SAMPLES).then(|| agreed as f64 / judged as f64);
        let level = match rate {
            _ if judged == 0 => "无记录",
            None => "样本不足",
            Some(r) if r >= scorecard::LOW_TRUST_SYCOPHANCY => "低信任",
            Some(_) => "正常",
        };
        trust.insert(
            seat.clone(),
            json!({"route":route,"decoy_agreed":agreed,"decoy_judged":judged,"sycophancy_rate":rate,"trust":level}),
        );
    }
    Value::Object(trust)
}

fn markdown(report: &Value) -> String {
    let verdict = |v: &Value| match v.as_bool() {
        Some(true) => "成立",
        Some(false) => "不成立",
        None => "未判",
    };
    let trust = &report["seat_trust"];
    let low = |seat: &str| trust[seat]["trust"] == "低信任";
    let verdict_cell = |verdicts: &Value| {
        verdicts
            .as_object()
            .unwrap()
            .iter()
            .map(|(seat, v)| {
                let mark = if low(seat) { "（低信任）" } else { "" };
                format!("{seat}:{}{mark}", verdict(v))
            })
            .collect::<Vec<_>>()
            .join("；")
    };
    let seat_label = |seat: &str| {
        let t = &trust[seat];
        match (t["trust"].as_str(), t["sycophancy_rate"].as_f64()) {
            (Some("无记录") | None, _) => format!("{seat}（附和率无记录）"),
            (Some(level), Some(rate)) => format!(
                "{seat}（附和率 {rate:.2}={}/{}，{level}）",
                t["decoy_agreed"], t["decoy_judged"]
            ),
            (Some(_), None) => format!(
                "{seat}（附和率样本不足 {}/{}）",
                t["decoy_judged"],
                scorecard::SYCOPHANCY_MIN_SAMPLES
            ),
        }
    };
    let mut out = format!(
        "# 复核一致/分歧对照\n\n席位：{}\n\n| 类别 | 位置 | 各家结论 | 摘要 |\n|---|---|---|---|\n",
        report["seats"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .map(seat_label)
            .collect::<Vec<_>>()
            .join("、")
    );
    for row in report["rows"].as_array().unwrap() {
        let verdicts = verdict_cell(&row["verdicts"]);
        out.push_str(&format!(
            "| {} | `{}` | {} | {} |\n",
            row["kind"].as_str().unwrap(),
            row["location"].as_str().unwrap(),
            verdicts,
            row["summary"].as_str().unwrap().replace('|', "\\|")
        ));
    }
    let pending = report["pending_verification"].as_array().unwrap();
    out.push_str("\n## 待执行验证（结论相反）\n\n");
    if pending.is_empty() {
        out.push_str("没有结论相反的位置。\n");
    } else {
        out.push_str(&format!(
            "{}\n\n| # | 位置 | 各家结论 | 摘要 | 可执行验证（测试/命令/复现） | 结果或裁决 |\n|---|---|---|---|---|---|\n",
            report["pending_rule"].as_str().unwrap()
        ));
        for (index, item) in pending.iter().enumerate() {
            let verdicts = verdict_cell(&item["verdicts"]);
            out.push_str(&format!(
                "| {} | `{}` | {} | {} | 待补 | 待跑／交维护者裁决 |\n",
                index + 1,
                item["location"].as_str().unwrap(),
                verdicts,
                item["summary"].as_str().unwrap().replace('|', "\\|")
            ));
        }
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
    out.push_str(&format!(
        "\n{}\n附和率来自诱饵金丝雀（宿主植入的错误主张被判为真问题的比例），至少 {} 次判定才计算；不低于 {:.0}% 标为低信任，其结论需额外独立验证。\n",
        report["note"].as_str().unwrap(),
        scorecard::SYCOPHANCY_MIN_SAMPLES,
        scorecard::LOW_TRUST_SYCOPHANCY * 100.0
    ));
    out
}
