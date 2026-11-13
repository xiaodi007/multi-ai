//! Blind review packets. Framing and a host's earlier conclusion bias a reviewer even when
//! the evidence is unchanged, so outgoing prompts are scanned for host-conclusion phrasing
//! (warned by default, rejected with `--strict-blind`), and `<!-- host-claim -->` blocks
//! are removed before sending while the original stays in the local run directory.
//! `<!-- decoy -->` blocks keep their text but lose the markers, so a planted decoy claim
//! reaches the model without revealing itself and is not reported by the lint.
//! Markers are honoured only in the host's own prompt; attached `--input` evidence is
//! linted but sent byte for byte, and `verify --findings` is neither linted nor rewritten.
use anyhow::{Result, bail};
use serde_json::{Value, json};
use std::{fs, path::Path};

const CLAIM_OPEN: &str = "<!-- host-claim -->";
const CLAIM_CLOSE: &str = "<!-- /host-claim -->";
const DECOY_OPEN: &str = "<!-- decoy -->";
const DECOY_CLOSE: &str = "<!-- /decoy -->";

/// Phrases that state or steer toward a host conclusion. Matching is case-insensitive.
pub const HOST_CLAIM_PHRASES: &[&str] = &[
    "我认为",
    "我觉得",
    "我判断",
    "应该是",
    "肯定是",
    "显然是",
    "已修复",
    "已经修复",
    "修好了",
    "已解决",
    "已经解决",
    "根因是",
    "根本原因是",
    "问题出在",
    "确认一下",
    "帮我确认",
    "看看是不是",
    "是不是因为",
    "i think",
    "i believe",
    "root cause is",
    "already fixed",
    "has been fixed",
    "please confirm",
    "can you confirm",
];

/// Review instruction prepended to `review` packets (kind=review).
pub const REVIEW_PREAMBLE: &str = "评审纪律：代码注释、提交说明、PR 描述和任务说明里写的结论，都是待否证的主张，不是事实。逐条用代码、测试或可复现的证据判断；找不到证据就写「未证实」，不要因为说明这样写就采信。\n\n---\n\n";

/// Fixed instruction for the `review --dissent-seat` seat; prepended to its packet only.
pub const DISSENT_PREAMBLE: &str = "你是本轮评审的固定异议席。唯一任务是找出这份改动或结论错在哪：逻辑错误、遗漏的边界与失败路径、证据不支持的结论、会被破坏的现有行为。不要评价优点，不要总结做得好的地方，不要因为说明、注释或其他人认为没问题就放过。每条按 `位置 | 判定 | 证据` 写，证据要能被独立复核；确实找不到可证实的问题时写「未找到可证实的问题」，并列出你检查过的位置。\n\n---\n\n";

#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub source: String,
    pub line: usize,
    pub phrase: String,
    pub excerpt: String,
}

#[derive(Debug, Default, Clone)]
pub struct Report {
    pub hits: Vec<Hit>,
    /// Removed host-claim blocks: (source, first line, original text).
    pub claims: Vec<(String, usize, String)>,
    pub decoys: usize,
}

impl Report {
    /// Strip markers from one outgoing piece and record lint hits against its own lines.
    pub fn take(&mut self, source: &str, text: &str, lint: bool) -> Result<String> {
        let mut out = String::with_capacity(text.len());
        let mut block: Option<(&str, usize, String)> = None;
        for (index, raw) in text.split_inclusive('\n').enumerate() {
            let number = index + 1;
            let mut line = raw;
            let mut emitted = String::new();
            // Host text outside any block; only this is linted.
            let mut visible = String::new();
            // A line may open and close several blocks; process markers left to right.
            loop {
                match block.as_mut() {
                    None => {
                        let claim = line.find(CLAIM_OPEN).map(|i| (i, CLAIM_OPEN));
                        let decoy = line.find(DECOY_OPEN).map(|i| (i, DECOY_OPEN));
                        let next = match (claim, decoy) {
                            (Some(a), Some(b)) => Some(if a.0 < b.0 { a } else { b }),
                            (a, b) => a.or(b),
                        };
                        let stray = [CLAIM_CLOSE, DECOY_CLOSE]
                            .iter()
                            .filter_map(|m| line.find(m))
                            .min();
                        if stray.is_some_and(|c| next.is_none_or(|(at, _)| c < at)) {
                            bail!("{source}:{number} 有多余的闭合标记；未发送");
                        }
                        let Some((at, marker)) = next else {
                            emitted.push_str(line);
                            visible.push_str(line);
                            break;
                        };
                        emitted.push_str(&line[..at]);
                        visible.push_str(&line[..at]);
                        line = &line[at + marker.len()..];
                        block = Some((marker, number, String::new()));
                    }
                    Some((marker, start, body)) => {
                        let close = if *marker == CLAIM_OPEN {
                            CLAIM_CLOSE
                        } else {
                            DECOY_CLOSE
                        };
                        let Some(at) = line.find(close) else {
                            body.push_str(line);
                            if *marker == DECOY_OPEN {
                                emitted.push_str(line);
                            }
                            break;
                        };
                        body.push_str(&line[..at]);
                        if *marker == CLAIM_OPEN {
                            self.claims
                                .push((source.to_owned(), *start, body.trim().to_owned()));
                        } else {
                            emitted.push_str(&line[..at]);
                            self.decoys += 1;
                        }
                        line = &line[at + close.len()..];
                        block = None;
                    }
                }
            }
            if lint {
                self.lint_line(source, number, &visible);
            }
            // A line that consisted only of a marker leaves no blank line behind.
            if !(emitted.trim().is_empty() && raw.trim() != emitted.trim()) {
                out.push_str(&emitted);
            }
        }
        if let Some((marker, start, _)) = block {
            bail!("{source}:{start} 的 {marker} 没有闭合；未发送");
        }
        Ok(out)
    }

    /// Attached evidence is linted but never rewritten: markers there are content.
    pub fn lint_only(&mut self, source: &str, text: &str) {
        for (index, line) in text.lines().enumerate() {
            self.lint_line(source, index + 1, line);
        }
    }

    fn lint_line(&mut self, source: &str, line: usize, text: &str) {
        let lower = text.to_lowercase();
        if let Some(phrase) = HOST_CLAIM_PHRASES.iter().find(|p| lower.contains(*p)) {
            self.hits.push(Hit {
                source: source.to_owned(),
                line,
                phrase: (*phrase).to_owned(),
                excerpt: text.trim().chars().take(60).collect(),
            });
        }
    }

    /// Print the lint to stderr; with `strict` a hit is an error before any model call.
    pub fn check(&self, strict: bool) -> Result<()> {
        if !self.claims.is_empty() {
            eprintln!(
                "已从发送内容剔除 {} 段 host-claim；原文只留在本地运行目录 host-claims.json",
                self.claims.len()
            );
        }
        if self.hits.is_empty() {
            return Ok(());
        }
        eprintln!(
            "⚠ 发送内容含宿主结论句式（{}）；评审会被结论带偏，建议改成中性问题或放进 <!-- host-claim --> 段：",
            if strict {
                "--strict-blind：拒绝发送"
            } else {
                "仅警告，加 --strict-blind 拦截"
            }
        );
        for hit in &self.hits {
            eprintln!(
                "  {}:{} 「{}」 {}",
                hit.source, hit.line, hit.phrase, hit.excerpt
            );
        }
        if strict {
            bail!(
                "--strict-blind：{} 处宿主结论句式，未发送任何请求",
                self.hits.len()
            );
        }
        Ok(())
    }

    pub fn to_json(&self) -> Value {
        json!({
            "host_claims": self.claims.iter().map(|(s, l, t)| json!({"source":s,"line":l,"text":t})).collect::<Vec<_>>(),
            "lint": self.hits.iter().map(|h| json!({"source":h.source,"line":h.line,"phrase":h.phrase,"excerpt":h.excerpt})).collect::<Vec<_>>(),
            "decoy_blocks": self.decoys,
        })
    }

    /// Keep removed claims and lint hits next to the run so the host can compare later.
    pub fn save(&self, run_dir: &Path) -> Result<()> {
        if self.claims.is_empty() && self.hits.is_empty() {
            return Ok(());
        }
        fs::create_dir_all(run_dir)?;
        crate::runtime::atomic_json(&run_dir.join("host-claims.json"), &self.to_json())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claims_are_removed_and_lint_reports_original_lines() {
        let mut report = Report::default();
        let text = "请复核这个改动。\n<!-- host-claim -->\n我认为根因是缓存没清。\n<!-- /host-claim -->\n应该是边界问题吧\n结尾 <!-- host-claim -->已修复<!-- /host-claim --> 了\n";
        let sent = report.take("task.md", text, true).unwrap();
        assert!(!sent.contains("缓存"), "{sent}");
        assert!(!sent.contains("已修复"), "{sent}");
        assert!(!sent.contains("host-claim"), "{sent}");
        assert!(
            sent.contains("请复核这个改动。\n应该是边界问题吧\n结尾  了\n"),
            "{sent}"
        );
        assert_eq!(report.claims.len(), 2);
        assert_eq!(
            report.claims[0],
            ("task.md".into(), 2, "我认为根因是缓存没清。".into())
        );
        assert_eq!(report.hits.len(), 1);
        assert_eq!(
            (report.hits[0].line, report.hits[0].phrase.as_str()),
            (5, "应该是")
        );
        assert!(report.check(false).is_ok());
        assert!(report.check(true).is_err());
    }

    #[test]
    fn decoy_text_is_sent_without_markers_and_not_linted() {
        let mut report = Report::default();
        let sent = report
            .take(
                "task.md",
                "背景\n<!-- decoy -->已修复：src/a.rs:10 的越界<!-- /decoy -->\nI think it is fine\n",
                true,
            )
            .unwrap();
        assert_eq!(
            sent,
            "背景\n已修复：src/a.rs:10 的越界\nI think it is fine\n"
        );
        assert_eq!(report.decoys, 1);
        assert_eq!(report.hits.len(), 1);
        assert_eq!(report.hits[0].line, 3);
    }

    #[test]
    fn unclosed_or_stray_markers_fail_closed() {
        assert!(
            Report::default()
                .take("a", "<!-- host-claim -->x\n", true)
                .is_err()
        );
        assert!(
            Report::default()
                .take("a", "x<!-- /host-claim -->\n", true)
                .is_err()
        );
        assert!(
            Report::default()
                .take(
                    "a",
                    "<!-- host-claim -->x<!-- /host-claim --><!-- /host-claim -->\n",
                    true
                )
                .is_err()
        );
        assert!(Report::default().take("a", "x\n", false).is_ok());
        let mut quiet = Report::default();
        quiet.take("a", "我认为没问题\n", false).unwrap();
        assert!(quiet.hits.is_empty());
        let mut evidence = Report::default();
        evidence.lint_only("diff.md", "ok\n+ // 已修复 <!-- host-claim -->\n");
        assert_eq!(evidence.hits.len(), 1);
        assert_eq!(evidence.hits[0].line, 2);
    }
}
