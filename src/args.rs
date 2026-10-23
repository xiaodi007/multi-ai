use anyhow::{Result, bail};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, Default)]
pub struct Args {
    pub raw: Vec<String>,
    pub values: HashMap<String, Vec<String>>,
    pub flags: HashSet<String>,
    pub positional: Vec<String>,
}
impl Args {
    pub fn parse(tokens: &[String]) -> Result<Self> {
        let booleans = [
            "help",
            "json",
            "quiet",
            "text-ready",
            "resume",
            "no-metered",
            "no-quota",
            "dry-run",
            "all",
            "apply",
            "legacy",
            "stream",
            "no-stream",
            "live",
            "force",
            "include-metered",
            "human-confirmed",
            "agentic",
            "ignore-cooldown",
            "allow-dirty",
            "network",
        ];
        let mut result = Self {
            raw: tokens.to_vec(),
            ..Self::default()
        };
        let mut i = 0;
        while i < tokens.len() {
            let token = &tokens[i];
            if token == "--" {
                result.positional.extend_from_slice(&tokens[i + 1..]);
                break;
            }
            if token == "-h" {
                result.flags.insert("help".into());
            } else if token.starts_with("--") || token == "-o" {
                let raw = if token == "-o" { "out" } else { &token[2..] };
                if let Some((key, value)) = raw.split_once('=') {
                    result
                        .values
                        .entry(key.into())
                        .or_default()
                        .push(value.into());
                } else if raw == "stream"
                    && tokens.get(i + 1).is_some_and(|v| {
                        ["true", "false", "on", "off", "yes", "no", "1", "0"].contains(&v.as_str())
                    })
                {
                    i += 1;
                    result
                        .values
                        .insert("stream".into(), vec![tokens[i].clone()]);
                } else if raw == "thinking"
                    && (i + 1 == tokens.len() || tokens[i + 1].starts_with("--"))
                {
                    result.values.insert("thinking".into(), vec!["auto".into()]);
                } else if ["only", "exclude"].contains(&raw) {
                    let mut values = Vec::new();
                    while i + 1 < tokens.len() && !tokens[i + 1].starts_with("--") {
                        i += 1;
                        values.push(tokens[i].clone());
                    }
                    result.values.entry(raw.into()).or_default().extend(values);
                } else if booleans.contains(&raw) {
                    result.flags.insert(raw.into());
                } else {
                    i += 1;
                    if i >= tokens.len() || tokens[i].starts_with("--") {
                        bail!("Missing value for --{raw}");
                    }
                    result
                        .values
                        .entry(raw.into())
                        .or_default()
                        .push(tokens[i].clone());
                }
            } else {
                result.positional.push(token.clone());
            }
            i += 1;
        }
        Ok(result)
    }
    pub fn get(&self, key: &str) -> Option<&str> {
        self.values
            .get(key)
            .and_then(|v| v.last())
            .map(String::as_str)
    }
    pub fn all(&self, key: &str) -> Vec<String> {
        self.values.get(key).cloned().unwrap_or_default()
    }
    pub fn has(&self, key: &str) -> bool {
        self.flags.contains(key) || self.values.contains_key(key)
    }
    pub fn set(&mut self, key: &str, value: impl ToString) {
        self.values.insert(key.into(), vec![value.to_string()]);
    }
    pub fn number(&self, key: &str, default: u64) -> Result<u64> {
        match self.get(key) {
            Some(v) => Ok(v.parse()?),
            None => Ok(default),
        }
    }
    pub fn prompt(&self) -> String {
        self.positional.join(" ")
    }
}
