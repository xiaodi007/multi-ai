//! Local recall checks against host-confirmed answers. Never sent to a model.
use crate::args::Args;
use anyhow::{Context, Result, bail};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{fs, sync::LazyLock};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Canary {
    #[serde(rename = "match")]
    pub pattern: String,
    pub expect: String,
}

pub fn load(args: &Args) -> Result<Vec<Canary>> {
    let values: Vec<Canary> = if let Some(path) = args.get("canary-file") {
        serde_json::from_slice(&fs::read(path).context("read --canary-file")?)?
    } else if let Some(value) = args.get("canaries-json") {
        serde_json::from_str(value)?
    } else {
        Vec::new()
    };
    validate(&values)?;
    Ok(values)
}

pub fn validate(values: &[Canary]) -> Result<()> {
    for value in values {
        if value.pattern.trim().is_empty() || value.expect.trim().is_empty() {
            bail!("canary match and expect must not be empty");
        }
        if location(&value.pattern).is_none() {
            Regex::new(regex_pattern(&value.pattern)).context("invalid canary regex")?;
        }
    }
    Ok(())
}

fn regex_pattern(text: &str) -> &str {
    text.strip_prefix("regex:").unwrap_or(text)
}

fn location(text: &str) -> Option<(String, u64)> {
    let (path, line) = text.trim().rsplit_once(':')?;
    let line = line.parse().ok()?;
    if path.is_empty()
        || text.starts_with("regex:")
        || path.starts_with('^')
        || path.contains(".*")
        || path.contains(".+")
        || path.contains('|')
        || path.contains("(?:")
    {
        return None;
    }
    Some((
        path.replace('\\', "/").trim_start_matches("./").into(),
        line,
    ))
}

pub(crate) fn positive(text: &str) -> Option<bool> {
    let lower = text.to_lowercase();
    // Negated labels must be checked before their positive substrings.
    static NEGATIVE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\b(refuted|unclear|false|invalid|unconfirmed|uncertain|rejected)\b").unwrap()
    });
    static POSITIVE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\b(confirmed|accepted|true|true_positive)\b").unwrap());
    if NEGATIVE.is_match(&lower)
        || [
            "误报",
            "假问题",
            "不是真问题",
            "非真问题",
            "不属实",
            "不成立",
            "无问题",
            "不是问题",
            "未确认",
            "待定",
            "存疑",
            "false_positive",
            "false positive",
            "not confirmed",
            "unconfirmed",
            "uncertain",
            "rejected",
        ]
        .iter()
        .any(|word| lower.contains(word))
    {
        return Some(false);
    }
    if POSITIVE.is_match(&lower)
        || ["真问题", "属实", "成立", "确认"]
            .iter()
            .any(|word| lower.contains(word))
    {
        return Some(true);
    }
    None
}

pub(crate) static LOCATIONS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?P<path>[\p{L}\p{N}_./\\-]+)(?::|#L)(?P<line>\d+)").unwrap());
pub fn evaluate(values: &[Canary], report: &str) -> Value {
    let locations = &*LOCATIONS;
    let scopes = finding_scopes(report);
    evaluate_scopes(values, &scopes, locations)
}
/// One table row or one finding paragraph per scope; a verdict heading owns what follows.
pub(crate) fn finding_scopes(report: &str) -> Vec<String> {
    let locations = &*LOCATIONS;
    let normalized = report.replace("\r\n", "\n");
    let heading = Regex::new(r"^\s*(#{1,6})\s+").unwrap();
    let numbered = Regex::new(r"^\s*[0-9]+[.)]\s+").unwrap();
    let mut scopes = Vec::new();
    let mut scope = String::new();
    let mut verdict_context = false;
    let mut heading_depth = None;
    for line in normalized.lines() {
        let verdict = positive(line);
        let depth = heading.captures(line).map(|c| c[1].len());
        if line.trim_start().starts_with('|') {
            if !scope.is_empty() {
                scopes.push(std::mem::take(&mut scope));
            }
            scopes.push(line.to_owned());
            verdict_context = false;
            heading_depth = None;
            continue;
        }
        if line.trim().is_empty() {
            // A labelled Markdown heading owns its following paragraphs and evidence lists.
            if !verdict_context
                && (positive(&scope).is_none() || locations.is_match(&scope))
                && !scope.is_empty()
            {
                scopes.push(std::mem::take(&mut scope));
            }
            continue;
        }
        let new_heading = depth.is_some_and(|d| {
            !(heading_depth.is_some_and(|old| d > old) && verdict_context && verdict.is_none())
        });
        let new_numbered = numbered.is_match(line) && (!verdict_context || verdict.is_some());
        let new_location = locations.is_match(line)
            && locations.is_match(&scope)
            && (!verdict_context || verdict.is_some());
        if !scope.is_empty() && (new_heading || new_numbered || new_location) {
            scopes.push(std::mem::take(&mut scope));
            verdict_context = false;
            heading_depth = None;
        }
        if let Some(depth) = depth {
            heading_depth = Some(depth);
        }
        if (depth.is_some() || numbered.is_match(line)) && verdict.is_some() {
            verdict_context = true;
        }
        if !scope.is_empty() {
            scope.push('\n');
        }
        scope.push_str(line);
    }
    if !scope.is_empty() {
        scopes.push(scope);
    }
    scopes
}
fn evaluate_scopes(values: &[Canary], scopes: &[String], locations: &Regex) -> Value {
    let details = values
        .iter()
        .map(|value| {
            let literal = location(&value.pattern);
            let regex = literal
                .is_none()
                .then(|| Regex::new(regex_pattern(&value.pattern)).unwrap());
            let mut mentioned = false;
            // A table row or individual finding is the verdict scope, never the report.
            let hit = scopes.iter().any(|scope| {
                let matches = if let Some((path, line)) = &literal {
                    locations.captures_iter(scope).any(|capture| {
                        let actual = capture["path"].replace('\\', "/");
                        let actual = actual.trim_start_matches("./");
                        let suffix = actual == path
                            || actual.ends_with(&format!("/{path}"))
                            || path.ends_with(&format!("/{actual}"));
                        suffix
                            && capture["line"]
                                .parse::<u64>()
                                .is_ok_and(|n| n.abs_diff(*line) <= 3)
                    })
                } else {
                    regex.as_ref().unwrap().is_match(scope)
                };
                mentioned |= matches;
                matches
                    && if positive(&value.expect) == Some(true) {
                        positive(scope) == Some(true)
                    } else {
                        scope.contains(&value.expect)
                    }
            });
            json!({"match":value.pattern,"expect":value.expect,"mentioned":mentioned,"hit":hit})
        })
        .collect::<Vec<_>>();
    json!({"canary_hits":details.iter().filter(|v|v["hit"] == true).count(),"canary_total":values.len(),"canary_results":details})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn suffix_tolerance_regex_and_verdict_scope() {
        let c = |pattern: &str| Canary {
            pattern: pattern.into(),
            expect: "真问题".into(),
        };
        let values = vec![
            c("src/a.rs:10"),
            c("src/b.rs:20"),
            c("redundant.*guard"),
            c("src/c.rs:30"),
        ];
        let result = evaluate(
            &values,
            "| /repo/src/a.rs:13 | 属实 |\n| src/b.rs:20 | 误报 |\n| redundant extra guard | CONFIRMED |\n| src/c.rs:34 | 真问题 |",
        );
        assert_eq!(result["canary_hits"], 2);
        assert_eq!(result["canary_total"], 4);
        assert_eq!(
            evaluate(
                &[c("src/.*:13"), c("regex:src/a.rs:13")],
                "| src/a.rs:13 | CONFIRMED |"
            )["canary_hits"],
            2
        );
        for text in [
            "### 边界问题 — CONFIRMED\n- 证据：src/a.rs:10",
            "### 边界问题 — CONFIRMED\n\n证据：src/a.rs:10",
            "### 边界问题 — CONFIRMED\n\n#### 证据\n- src/a.rs:10",
        ] {
            assert_eq!(
                evaluate(&[c("src/a.rs:10")], text)["canary_hits"],
                1,
                "{text}"
            );
        }
        assert_eq!(
            evaluate(
                &[c("src/b.rs:20")],
                "### A CONFIRMED\n\n- src/a.rs:10\n### B REFUTED\n\n- src/b.rs:20"
            )["canary_hits"],
            0
        );

        assert_eq!(
            evaluate(
                &[c("a.rs:10")],
                "### 1. 边界问题 — CONFIRMED\n证据：src/a.rs:10"
            )["canary_hits"],
            1
        );
        assert_eq!(
            evaluate(&[c("a.rs:10")], "1. CONFIRMED\n证据 src/a.rs:10")["canary_hits"],
            1
        );
        assert_eq!(
            evaluate(&[c("b.rs:20")], "1. a.rs:10 — 误报\n2. b.rs:20 — CONFIRMED")["canary_hits"],
            1
        );
        assert_eq!(
            evaluate(&[c("a.rs:1")], "a.rs:1\nUNCONFIRMED")["canary_hits"],
            0
        );
        assert_eq!(
            evaluate(&[c("a.rs:1")], "a.rs:1\n位置存在")["canary_hits"],
            0
        );
        assert_eq!(
            evaluate(
                &[c("a.rs:1")],
                "| a.rs:1 | REFUTED | 原发现声称已确认 CONFIRMED |"
            )["canary_hits"],
            0
        );
    }
}
