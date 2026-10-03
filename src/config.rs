use crate::args::Args;
use anyhow::{Context, Result, bail};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Route {
    pub id: String,
    pub legacy_id: String,
    pub pool: String,
    pub family: String,
    pub label: String,
    pub agentic: bool,
    pub cost: String,
    pub rank: u32,
    pub argv: Vec<String>,
    pub extract: String,
    pub default_timeout: u64,
    #[serde(default)]
    pub api_alias: Option<String>,
}

#[derive(Clone)]
pub struct Config {
    pub profiles: Value,
    pub routes: Vec<Route>,
    environment: HashMap<String, String>,
}

/// Credentials are execution-only. Debug deliberately excludes key, URL and request messages.
#[derive(Clone)]
pub struct ResolvedRequest {
    pub provider: String,
    pub model: String,
    pub model_source: String,
    pub key: String,
    pub url: String,
    pub body: Value,
    pub timeout_ms: u64,
    pub headers_timeout_ms: u64,
    pub idle_timeout_ms: u64,
    pub retries: u32,
    pub stream: bool,
    pub thinking: String,
    pub effort: Option<String>,
}

impl fmt::Debug for ResolvedRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResolvedRequest")
            .field("provider", &self.provider)
            .field("model", &self.model)
            .field("model_source", &self.model_source)
            .field("key", &"[REDACTED]")
            .field("timeout_ms", &self.timeout_ms)
            .field("headers_timeout_ms", &self.headers_timeout_ms)
            .field("idle_timeout_ms", &self.idle_timeout_ms)
            .field("retries", &self.retries)
            .field("stream", &self.stream)
            .field("thinking", &self.thinking)
            .field("effort", &self.effort)
            .finish_non_exhaustive()
    }
}

impl Config {
    /// Capture env before spawning requests. Child commands receive environment(), rather than
    /// mutating Rust's process-wide environment while Tokio worker threads may be active.
    pub fn load() -> Result<Self> {
        let mut environment: HashMap<String, String> = std::env::vars().collect();
        let cwd = std::env::current_dir().context("Cannot determine current directory")?;
        let home = environment
            .get("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        let config_dir = environment
            .get("MULTI_AI_CONFIG_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config/multi-ai"));
        load_dotenv(&cwd.join(".env"), &mut environment)?;
        load_dotenv(&config_dir.join(".env"), &mut environment)?;
        load_dotenv(
            &home.join(".config/multi-llm-pipeline/.env"),
            &mut environment,
        )?;
        let profiles = read_or_default(
            &config_dir.join("profiles.json"),
            include_str!("../config/profiles.json"),
        )?;
        let routes_value = read_or_default(
            &config_dir.join("routes.json"),
            include_str!("../config/routes.json"),
        )?;
        let mut routes: Vec<Route> =
            serde_json::from_value(routes_value).context("Invalid routes configuration")?;
        for route in &mut routes {
            for arg in &mut route.argv {
                *arg = arg.replace("{home}", &home.to_string_lossy());
            }
        }
        let config = Self {
            profiles,
            routes,
            environment,
        };
        config.validate()?;
        Ok(config)
    }

    pub fn environment(&self) -> &HashMap<String, String> {
        &self.environment
    }

    fn validate(&self) -> Result<()> {
        let providers = self.profiles["providers"]
            .as_object()
            .context("profiles.providers must be an object")?;
        let aliases = self.profiles["aliases"]
            .as_object()
            .context("profiles.aliases must be an object")?;
        let mut ids = HashSet::new();
        let mut legacy_ids = HashSet::new();
        for route in &self.routes {
            if !valid_route_id(&route.id) || !ids.insert(&route.id) {
                bail!("Route id is unsafe or duplicated: {}", route.id);
            }
            if !valid_route_id(&route.legacy_id) || !legacy_ids.insert(&route.legacy_id) {
                bail!(
                    "Legacy route id is unsafe or duplicated: {}",
                    route.legacy_id
                );
            }
            if route.pool.is_empty() || route.family.is_empty() || route.default_timeout == 0 {
                bail!(
                    "Route {} needs a pool, family and positive timeout",
                    route.id
                );
            }
            if !["sub", "metered"].contains(&route.cost.as_str()) {
                bail!("Route {} has invalid cost class", route.id);
            }
            if !["stdout", "last-message-file", "jsonl-assistant", "llm-json"]
                .contains(&route.extract.as_str())
            {
                bail!("Route {} has unknown extractor", route.id);
            }
            if let Some(alias) = &route.api_alias {
                if !aliases.contains_key(alias) {
                    bail!("Route {} refers to unknown API alias {alias}", route.id);
                }
                if aliases[alias]["dead"].as_bool().unwrap_or(false) {
                    bail!("Route {} refers to a dead API alias", route.id);
                }
            } else if route.argv.is_empty() {
                bail!("CLI route {} needs argv", route.id);
            }
        }
        for (name, provider) in providers {
            if !provider["models"].is_object() || !provider["keyEnv"].is_array() {
                bail!("Provider {name} needs models and keyEnv");
            }
            for group in ["alwaysThinkingModels", "visionModels"] {
                for pattern in provider[group].as_array().into_iter().flatten() {
                    Regex::new(
                        pattern
                            .as_str()
                            .context("Model capability pattern must be a string")?,
                    )
                    .with_context(|| format!("Invalid {group} pattern for {name}"))?;
                }
            }
            for key in ["requirePattern", "rejectPattern"] {
                if let Some(pattern) = provider["modelGuard"][key].as_str() {
                    Regex::new(pattern)
                        .with_context(|| format!("Invalid model guard for {name}"))?;
                }
            }
        }
        Ok(())
    }

    pub fn route(&self, name: &str, legacy: bool) -> Result<Route> {
        self.routes
            .iter()
            .find(|route| route.id == name || (legacy && route.legacy_id == name))
            .cloned()
            .with_context(|| {
                format!("Unknown route '{name}'. Run multi-ai routes for explicit route names")
            })
    }

    pub fn provider(&self, name: &str) -> Result<&Value> {
        self.profiles["providers"]
            .get(name)
            .with_context(|| format!("Unknown provider: {name}"))
    }

    pub fn first_key(&self, provider: &Value) -> (String, String) {
        let names = provider["keyEnv"].as_array();
        for name in names.into_iter().flatten().filter_map(Value::as_str) {
            if let Some(value) = self.environment.get(name).filter(|value| !value.is_empty()) {
                return (name.to_owned(), value.clone());
            }
        }
        let first = names
            .and_then(|names| names.first())
            .and_then(Value::as_str)
            .unwrap_or("API_KEY");
        (first.to_owned(), String::new())
    }

    pub fn system_prompt(&self, key: Option<&str>, seat: Option<&str>) -> String {
        let prompts = &self.profiles["prompts"];
        let role = seat
            .and_then(|seat| prompts["seats"][seat].as_str())
            .or_else(|| key.and_then(|key| prompts["system"][key].as_str()))
            .unwrap_or("你是严谨的分析助手。用中文。材料没有的不要补。不知道就写不知道。");
        if key == Some("verify") {
            return role.to_owned();
        }
        match prompts["outputSkeleton"]
            .as_str()
            .filter(|skeleton| !skeleton.is_empty())
        {
            Some(skeleton) => format!("{role}\n\n{skeleton}"),
            None => role.to_owned(),
        }
    }

    pub fn public_routes(&self) -> Result<Value> {
        let mut entries = Vec::new();
        let mut routes = self.routes.clone();
        routes.sort_by(|a, b| (a.rank, &a.id).cmp(&(b.rank, &b.id)));
        for route in routes {
            let mut entry = json!({
                "id": route.id, "legacy_id": route.legacy_id, "pool": route.pool,
                "family": route.family, "label": route.label, "agentic": route.agentic,
                "cost": route.cost, "rank": route.rank, "default_timeout": route.default_timeout,
                "transport": if route.api_alias.is_some() { "api" } else { "cli" },
                "capabilities": { "tools": route.agentic, "text": true }
            });
            if let Some(alias) = &route.api_alias {
                let req = self.resolve_alias_internal(alias, &Args::default(), json!([]), false)?;
                let provider = self.provider(&req.provider)?;
                entry["api_alias"] = json!(alias);
                entry["provider"] = json!(req.provider);
                entry["model"] = json!(req.model);
                entry["model_source"] = json!(req.model_source);
                entry["thinking"] = json!(req.thinking);
                entry["effort"] = json!(req.effort);
                entry["stream"] = json!(req.stream);
                entry["capabilities"]["vision"] =
                    json!(matches_patterns(&provider["visionModels"], &req.model)?);
            } else {
                let fixed_model = route
                    .argv
                    .windows(2)
                    .find(|args| args[0] == "-m" || args[0] == "--model")
                    .map(|args| args[1].clone());
                entry["model"] = json!(fixed_model);
                entry["model_source"] = json!(if fixed_model.is_some() {
                    "route"
                } else {
                    "cli-default-unknown"
                });
                entry["command"] = json!(route.argv.first());
                entry["capabilities"]["vision"] = Value::Null;
            }
            entries.push(entry);
        }
        Ok(json!(entries))
    }

    pub fn alias_metadata(&self, alias: &str, args: &Args) -> Result<Value> {
        let request = self.resolve_alias_internal(alias, args, json!([]), false)?;
        Ok(request_metadata(&request))
    }

    pub fn request_metadata(
        &self,
        provider: &str,
        model_role_or_name: &str,
        args: &Args,
        defaults: &Value,
    ) -> Result<Value> {
        let request = self.resolve_request_internal(
            provider,
            model_role_or_name,
            args,
            json!([]),
            defaults,
            false,
        )?;
        Ok(request_metadata(&request))
    }

    pub fn resolve_alias(
        &self,
        alias: &str,
        args: &Args,
        messages: Value,
    ) -> Result<ResolvedRequest> {
        self.resolve_alias_internal(alias, args, messages, true)
    }

    fn resolve_alias_internal(
        &self,
        name: &str,
        args: &Args,
        messages: Value,
        require_key: bool,
    ) -> Result<ResolvedRequest> {
        let mut current = name;
        let mut visited = HashSet::new();
        loop {
            if !visited.insert(current) {
                bail!("Alias successor cycle at '{current}'");
            }
            let alias = self.profiles["aliases"]
                .get(current)
                .with_context(|| format!("Unknown alias: {current}"))?;
            if alias["dead"].as_bool().unwrap_or(false) {
                let successor = alias["successor"]
                    .as_str()
                    .context("Dead alias has no successor")?;
                if !args.has("legacy") {
                    bail!("Alias '{current}' is DEAD. Use '{successor}' (or pass --legacy)");
                }
                current = successor;
                continue;
            }
            let provider = alias["provider"]
                .as_str()
                .context("Alias has no provider")?;
            let selector = alias["model"]
                .as_str()
                .or_else(|| alias["modelRole"].as_str())
                .unwrap_or("");
            let mut request = self.resolve_request_internal(
                provider,
                selector,
                args,
                messages,
                alias,
                require_key,
            )?;
            if args.get("model").is_none() && alias["model"].is_string() {
                request.model_source = format!("alias:{current}");
            }
            return Ok(request);
        }
    }

    pub fn resolve_request(
        &self,
        provider: &str,
        model_role_or_name: &str,
        args: &Args,
        messages: Value,
        defaults: &Value,
    ) -> Result<ResolvedRequest> {
        self.resolve_request_internal(provider, model_role_or_name, args, messages, defaults, true)
    }

    fn resolve_request_internal(
        &self,
        initial_provider: &str,
        selector: &str,
        args: &Args,
        messages: Value,
        defaults: &Value,
        require_key: bool,
    ) -> Result<ResolvedRequest> {
        let original = self.provider(initial_provider)?;
        let selected = args.get("model").unwrap_or(selector);
        let explicit = args.get("model").is_some()
            || defaults["model"]
                .as_str()
                .is_some_and(|model| model == selected)
            || !original["models"][selected].is_string() && !selected.is_empty();
        let (initial_model, mut model_source) = if explicit {
            (selected.to_owned(), "explicit-model".to_owned())
        } else {
            self.model_for(original, selected)?
        };
        if args.get("model").is_some() {
            model_source = "argument".to_owned();
        }
        let provider_name = self.guarded_provider(initial_provider, &initial_model)?;
        let provider = self.provider(&provider_name)?;
        // Legacy chat() reapplies a role against the guarded provider; pinned models stay pinned.
        let model = if explicit || provider_name == initial_provider {
            initial_model
        } else {
            let (model, source) = self.model_for(provider, selected)?;
            model_source = source;
            model
        };
        if messages_have_images(&messages) && !matches_patterns(&provider["visionModels"], &model)?
        {
            bail!(
                "Provider {provider_name} model {model} does not accept --image. Use glm-flash or kimi-k3"
            );
        }
        let style = provider["reasoningStyle"].as_str().unwrap_or("legacy");
        let always = style == "k3" || matches_patterns(&provider["alwaysThinkingModels"], &model)?;
        let requested_thinking = option_string(args, "thinking", defaults, "thinking")
            .or_else(|| provider["thinking"]["default"].as_str().map(str::to_owned))
            .unwrap_or_else(|| "auto".to_owned());
        // Invalid thinking options remain errors even for always-thinking models.
        let normalized = normalize_thinking(&requested_thinking)?;
        let thinking = if always {
            "always"
        } else {
            normalized.as_str()
        }
        .to_owned();
        let sends_effort = style == "k3"
            || (always
                && (provider["thinking"]["effortSupported"]
                    .as_bool()
                    .unwrap_or(false)
                    || provider["reasoning"]["effortTopLevel"]
                        .as_bool()
                        .unwrap_or(false)))
            || (normalized == "on"
                && provider["thinking"]["effortSupported"]
                    .as_bool()
                    .unwrap_or(false));
        if args.has("effort") && args.get("effort").is_none() {
            bail!("--effort needs a value");
        }
        let effort = if sends_effort {
            let section = if style == "k3" {
                &provider["reasoning"]
            } else {
                &provider["thinking"]
            };
            let effort = option_string(args, "effort", defaults, "effort")
                .filter(|v| !v.is_empty())
                .or_else(|| section["defaultEffort"].as_str().map(str::to_owned))
                .unwrap_or_else(|| if style == "k3" { "max" } else { "high" }.to_owned())
                .to_lowercase();
            if let Some(allowed) = section["effortValues"].as_array()
                && !allowed.iter().any(|value| value.as_str() == Some(&effort))
            {
                bail!("Provider {provider_name} does not accept effort '{effort}'");
            }
            Some(effort)
        } else {
            None
        };
        let max_tokens = positive_number(
            args,
            "max-tokens",
            defaults
                .get("maxTokens")
                .filter(|value| !value.is_null())
                .or_else(|| provider.get("maxTokens")),
            4000,
        )?;
        let default_timeout = if style == "k3" {
            900_000
        } else if normalized == "on" || always {
            420_000
        } else {
            120_000
        };
        let timeout_ms = positive_number(
            args,
            "timeout-ms",
            defaults.get("timeoutMs"),
            default_timeout,
        )?;
        let headers_timeout_ms = positive_number(
            args,
            "headers-timeout-ms",
            defaults.get("headersTimeoutMs"),
            timeout_ms.min(300_000),
        )?;
        let idle_timeout_ms = positive_number(
            args,
            "idle-timeout-ms",
            defaults.get("idleTimeoutMs"),
            120_000,
        )?;
        let retries = args
            .get("retries")
            .map(str::to_owned)
            .or_else(|| {
                defaults
                    .get("retries")
                    .filter(|value| !value.is_null())
                    .map(value_text)
            })
            .or_else(|| self.environment.get("LLM_RUN_RETRIES").cloned())
            .unwrap_or_else(|| "2".to_owned())
            .parse::<u32>()
            .context("LLM_RUN_RETRIES / --retries must be a non-negative integer")?;
        let stream_default = provider_name == "glm"
            || (style == "k3"
                && provider["reasoning"]["streamForEfforts"]
                    .as_array()
                    .is_some_and(|values| {
                        values
                            .iter()
                            .any(|value| value.as_str() == effort.as_deref())
                    }));
        let stream = if args.has("no-stream") {
            false
        } else if let Some(raw) = args.get("stream") {
            parse_bool(raw)?
        } else if args.has("stream") {
            true
        } else {
            defaults["stream"].as_bool().unwrap_or(stream_default)
        };
        let mut body = json!({ "model": model, "messages": messages, "max_tokens": max_tokens });
        let temperature = if let Some(raw) = args.get("temperature") {
            Some(raw.parse::<f64>().context("Invalid --temperature")?)
        } else if provider["temperature"].is_null() {
            None
        } else {
            defaults["temperature"]
                .as_f64()
                .or_else(|| provider["temperature"].as_f64())
        };
        if let Some(temperature) = temperature {
            if !temperature.is_finite() {
                bail!("--temperature must be finite");
            }
            body["temperature"] = json!(temperature);
        }
        if style == "k3" {
            body["reasoning_effort"] = json!(effort);
        } else if always {
            body["thinking"] = json!({ "type": "enabled" });
            if sends_effort {
                body["reasoning_effort"] = json!(effort);
            }
        } else if provider["thinking"]["supported"].as_bool().unwrap_or(false)
            && normalized != "auto"
        {
            body["thinking"] =
                json!({ "type": if normalized == "on" { "enabled" } else { "disabled" } });
            if sends_effort {
                body["reasoning_effort"] = json!(effort);
            }
        }
        if stream {
            body["stream"] = json!(true);
            body["stream_options"] = json!({ "include_usage": true });
        }
        let (key_name, key) = self.first_key(provider);
        if require_key && key.is_empty() {
            bail!("Provider {provider_name} API key is missing: set {key_name}");
        }
        let base_url = provider["baseUrl"]
            .as_str()
            .context("Provider has no baseUrl")?;
        let chat_path = provider["chatPath"].as_str().unwrap_or("/chat/completions");
        let url = format!("{}{chat_path}", base_url.trim_end_matches('/'));
        Ok(ResolvedRequest {
            provider: provider_name,
            model,
            model_source,
            key,
            url,
            body,
            timeout_ms,
            headers_timeout_ms,
            idle_timeout_ms,
            retries,
            stream,
            thinking,
            effort,
        })
    }

    fn model_for(&self, provider: &Value, selector: &str) -> Result<(String, String)> {
        if selector.is_empty() {
            return Ok((
                provider["models"]["fast"]
                    .as_str()
                    .context("Provider has no fast model")?
                    .to_owned(),
                "profile:fast".to_owned(),
            ));
        }
        if let Some(model) = provider["models"][selector].as_str() {
            if let Some(name) = provider["modelEnv"][selector].as_str()
                && let Some(value) = self.environment.get(name).filter(|value| !value.is_empty())
            {
                return Ok((value.clone(), format!("environment:{name}")));
            }
            return Ok((model.to_owned(), format!("profile:{selector}")));
        }
        Ok((selector.to_owned(), "explicit-model".to_owned()))
    }

    fn guarded_provider(&self, name: &str, model: &str) -> Result<String> {
        let guard = &self.provider(name)?["modelGuard"];
        let rejected = match guard["rejectPattern"].as_str() {
            Some(pattern) => Regex::new(pattern)?.is_match(model),
            None => false,
        };
        let missing = match guard["requirePattern"].as_str() {
            Some(pattern) => !Regex::new(pattern)?.is_match(model),
            None => false,
        };
        if !rejected && !missing {
            return Ok(name.to_owned());
        }
        let target = guard["mismatchProvider"]
            .as_str()
            .with_context(|| format!("Provider {name} cannot drive model {model}"))?;
        self.provider(target)?;
        Ok(target.to_owned())
    }
}

fn valid_route_id(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || ['-', '_', '.'].contains(&character)
        })
}

fn request_metadata(request: &ResolvedRequest) -> Value {
    let mut options = request.body.clone();
    if let Some(object) = options.as_object_mut() {
        object.remove("messages");
    }
    json!({
        "provider": request.provider, "model": request.model, "model_source": request.model_source,
        "thinking": request.thinking, "effort": request.effort, "stream": request.stream,
        "timeout_ms": request.timeout_ms, "headers_timeout_ms": request.headers_timeout_ms,
        "idle_timeout_ms": request.idle_timeout_ms, "retries": request.retries, "options": options
    })
}

fn read_or_default(path: &Path, embedded: &str) -> Result<Value> {
    match fs::read_to_string(path) {
        Ok(contents) => serde_json::from_str(&contents)
            .with_context(|| format!("Invalid JSON configuration: {}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(serde_json::from_str(embedded)?)
        }
        Err(error) => {
            Err(error).with_context(|| format!("Cannot read configuration: {}", path.display()))
        }
    }
}

fn load_dotenv(path: &Path, environment: &mut HashMap<String, String>) -> Result<()> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| format!("Cannot read dotenv: {}", path.display()));
        }
    };
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((raw_key, raw_value)) = line.split_once('=') else {
            continue;
        };
        let key = raw_key
            .strip_prefix("export")
            .filter(|rest| rest.chars().next().is_some_and(char::is_whitespace))
            .unwrap_or(raw_key)
            .trim();
        if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            continue;
        }
        let mut value = strip_inline_comment(raw_value.trim());
        if value.len() >= 2
            && ((value.starts_with('"') && value.ends_with('"'))
                || (value.starts_with('\'') && value.ends_with('\'')))
        {
            value = &value[1..value.len() - 1];
        }
        if environment.get(key).is_none_or(|value| value.is_empty()) {
            environment.insert(key.to_owned(), value.to_owned());
        }
    }
    Ok(())
}

fn strip_inline_comment(value: &str) -> &str {
    let mut quote = None;
    let mut previous = None;
    for (index, character) in value.char_indices() {
        if let Some(expected) = quote {
            if character == expected && previous != Some('\\') {
                quote = None;
            }
        } else if character == '"' || character == '\'' {
            quote = Some(character);
        } else if character == '#' && (index == 0 || previous.is_some_and(char::is_whitespace)) {
            return value[..index].trim();
        }
        previous = Some(character);
    }
    value.trim()
}

fn option_string(args: &Args, key: &str, defaults: &Value, default_key: &str) -> Option<String> {
    args.get(key).map(str::to_owned).or_else(|| {
        defaults
            .get(default_key)
            .filter(|value| !value.is_null())
            .map(value_text)
    })
}

fn value_text(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| value.to_string())
}

fn positive_number(args: &Args, key: &str, default: Option<&Value>, fallback: u64) -> Result<u64> {
    let value = if let Some(raw) = args.get(key) {
        raw.parse::<u64>()
            .with_context(|| format!("--{key} must be a positive integer"))?
    } else if let Some(value) = default.filter(|value| !value.is_null()) {
        value
            .as_u64()
            .with_context(|| format!("Default {key} must be a positive integer"))?
    } else {
        fallback
    };
    if value == 0 {
        bail!("--{key} must be a positive integer");
    }
    Ok(value)
}

fn normalize_thinking(raw: &str) -> Result<String> {
    match raw.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" | "enabled" => Ok("on".to_owned()),
        "0" | "false" | "no" | "off" | "disabled" => Ok("off".to_owned()),
        "auto" => Ok("auto".to_owned()),
        _ => bail!("Invalid --thinking value: {raw}"),
    }
}

fn parse_bool(raw: &str) -> Result<bool> {
    match raw.to_ascii_lowercase().as_str() {
        "1" | "true" | "on" => Ok(true),
        "0" | "false" | "off" => Ok(false),
        _ => bail!("Invalid --stream value: {raw}"),
    }
}

fn matches_patterns(patterns: &Value, model: &str) -> Result<bool> {
    for pattern in patterns.as_array().into_iter().flatten() {
        let pattern = pattern.as_str().context("Model pattern must be a string")?;
        if Regex::new(pattern)?.is_match(model) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn messages_have_images(messages: &Value) -> bool {
    messages.as_array().is_some_and(|messages| {
        messages.iter().any(|message| {
            message["content"]
                .as_array()
                .is_some_and(|parts| parts.iter().any(|part| part["type"] == "image_url"))
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Config {
        let profiles = serde_json::from_str(include_str!("../config/profiles.json")).unwrap();
        let routes = serde_json::from_str(include_str!("../config/routes.json")).unwrap();
        Config {
            profiles,
            routes,
            environment: HashMap::from([
                (
                    "DEEPSEEK_API_KEY".to_owned(),
                    "test-deepseek-key".to_owned(),
                ),
                ("KIMI_API_KEY".to_owned(), "test-kimi-key".to_owned()),
                ("ZHIPU_API_KEY".to_owned(), "test-glm-key".to_owned()),
            ]),
        }
    }
    fn args(values: &[(&str, &str)]) -> Args {
        let mut args = Args::default();
        for (key, value) in values {
            args.set(key, value);
        }
        args
    }
    fn messages() -> Value {
        json!([{ "role": "user", "content": "fixture" }])
    }

    #[test]
    fn routes_preserve_ranks_pools_and_legacy_names() {
        let config = config();
        config.validate().unwrap();
        assert_eq!(config.routes.len(), 9);
        assert_eq!(config.route("agy", true).unwrap().rank, 10);
        assert_eq!(
            config.route("kimi", true).unwrap().pool,
            config.route("kimi-k3", true).unwrap().pool
        );
        assert_eq!(
            config.route("deepseek", true).unwrap().pool,
            config.route("claude-ds", true).unwrap().pool
        );
        assert!(config.route("kimi", false).is_err());
        assert!(config.route("kimi-cli-default", false).is_ok());
    }

    #[test]
    fn route_ids_cannot_escape_output_and_cache_directories() {
        for invalid in [
            "",
            ".",
            "..",
            "../escaped",
            "/absolute",
            "a/b",
            "a\\b",
            "a b",
            "汉字",
            "a\nb",
        ] {
            let mut config = config();
            config.routes[0].id = invalid.to_owned();
            assert!(
                config.validate().is_err(),
                "unsafe id accepted: {invalid:?}"
            );
            config.routes[0].id = "grok-cli".to_owned();
            config.routes[0].legacy_id = invalid.to_owned();
            assert!(
                config.validate().is_err(),
                "unsafe legacy id accepted: {invalid:?}"
            );
        }
        let mut config = config();
        config.routes[0].id = "grok.v4_cli-1".to_owned();
        config.routes[0].legacy_id = "grok.v4_legacy-1".to_owned();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn public_routes_are_offline_and_never_expose_credential_fields() {
        let mut config = config();
        config.environment.clear();
        let routes = config.public_routes().unwrap();
        let serialized = routes.to_string();
        for forbidden in [
            "keyEnv",
            "baseUrl",
            "https://",
            "test-deepseek-key",
            "API_KEY",
        ] {
            assert!(!serialized.contains(forbidden));
        }
        let kimi = routes
            .as_array()
            .unwrap()
            .iter()
            .find(|route| route["id"] == "kimi-cli-default")
            .unwrap();
        assert!(kimi["model"].is_null());
        assert_eq!(kimi["model_source"], "cli-default-unknown");
        assert!(
            config
                .resolve_alias("glm", &Args::default(), messages())
                .is_err()
        );
    }

    #[test]
    fn api_env_resolution_is_shared_with_display_and_cache_identity() {
        let mut config = config();
        config
            .environment
            .insert("GLM_MODEL".into(), "glm-5.3-flash".into());
        let request = config
            .resolve_alias("glm", &Args::default(), messages())
            .unwrap();
        let metadata = config.alias_metadata("glm", &Args::default()).unwrap();
        let display = config.public_routes().unwrap();
        let display_glm = display
            .as_array()
            .unwrap()
            .iter()
            .find(|route| route["id"] == "glm-api")
            .unwrap();
        assert_eq!(request.model, "glm-5.3-flash");
        assert_eq!(metadata["model"], request.model);
        assert_eq!(display_glm["model"], request.model);
        assert_eq!(request.model_source, "environment:GLM_MODEL");
        config
            .environment
            .insert("DEEPSEEK_REASONER_MODEL".into(), "overridden".into());
        assert_eq!(
            config
                .resolve_alias("deepseek-pro", &Args::default(), messages())
                .unwrap()
                .model,
            "deepseek-v4-pro"
        );
        assert_eq!(
            config
                .resolve_request("deepseek", "pro", &Args::default(), messages(), &json!({}))
                .unwrap()
                .model,
            "overridden"
        );
        assert_eq!(
            config
                .resolve_alias("glm", &args(&[("model", "glm-5.3")]), messages())
                .unwrap()
                .model,
            "glm-5.3"
        );
        // --model is always a wire model name, even if it happens to match a profile role.
        assert_eq!(
            config
                .resolve_alias("deepseek", &args(&[("model", "pro")]), messages())
                .unwrap()
                .model,
            "pro"
        );
    }

    #[test]
    fn k3_effort_and_stream_remain_distinct_from_k2_thinking() {
        let config = config();
        let k3 = config
            .resolve_alias("kimi-k3", &Args::default(), messages())
            .unwrap();
        assert_eq!(k3.body["reasoning_effort"], "max");
        assert!(k3.body.get("thinking").is_none());
        assert!(k3.stream);
        assert_eq!(k3.timeout_ms, 900_000);
        assert_eq!(k3.thinking, "always");
        let low = config
            .resolve_alias("kimi-k3-low", &Args::default(), messages())
            .unwrap();
        assert!(!low.stream);
        let k2 = config
            .resolve_alias(
                "kimi-code",
                &args(&[("thinking", "off"), ("effort", "max")]),
                messages(),
            )
            .unwrap();
        assert_eq!(k2.body["thinking"]["type"], "enabled");
        assert!(k2.body.get("reasoning_effort").is_none());
        let glm = config
            .resolve_alias("glm", &args(&[("thinking", "off")]), messages())
            .unwrap();
        assert_eq!(glm.body["thinking"]["type"], "enabled");
        assert_eq!(glm.body["reasoning_effort"], "high");
        assert!(glm.stream);
    }

    #[test]
    fn model_guards_use_correct_protocol_and_explicit_model() {
        let config = config();
        let k3 = config
            .resolve_request("kimi", "kimi-k3", &Args::default(), messages(), &json!({}))
            .unwrap();
        assert_eq!(k3.provider, "kimi-k3");
        assert!(k3.body.get("thinking").is_none());
        let k2 = config
            .resolve_request(
                "kimi-k3",
                "kimi-k2.7-code",
                &Args::default(),
                messages(),
                &json!({}),
            )
            .unwrap();
        assert_eq!(k2.provider, "kimi");
        assert!(k2.body.get("reasoning_effort").is_none());
        let mut config = config;
        config
            .environment
            .insert("KIMI_K3_MODEL".into(), "kimi-k2.7-code".into());
        let request = config
            .resolve_alias("kimi-k3", &Args::default(), messages())
            .unwrap();
        assert_eq!(request.provider, "kimi");
        assert_eq!(request.model, "kimi-k2.7-code");
    }

    #[test]
    fn vision_and_invalid_options_fail_before_execution() {
        let config = config();
        let image = json!([{ "role": "user", "content": [{ "type": "image_url", "image_url": { "url": "data:image/png;base64,Zg==" } }] }]);
        assert!(
            config
                .resolve_alias("deepseek-pro", &Args::default(), image.clone())
                .is_err()
        );
        assert!(
            config
                .resolve_alias("glm-flash", &Args::default(), image.clone())
                .is_ok()
        );
        assert!(
            config
                .resolve_alias("kimi-k3", &Args::default(), image)
                .is_ok()
        );
        assert!(
            config
                .resolve_alias("kimi-k3", &args(&[("effort", "medium")]), messages())
                .is_err()
        );
        assert!(
            config
                .resolve_alias("glm", &args(&[("timeout-ms", "0")]), messages())
                .is_err()
        );
        assert!(
            config
                .resolve_alias("glm", &args(&[("max-tokens", "1.5")]), messages())
                .is_err()
        );
        assert!(
            config
                .resolve_alias("glm", &args(&[("retries", "-1")]), messages())
                .is_err()
        );
    }

    #[test]
    fn transport_and_temperature_options_preserve_precedence() {
        let config = config();
        let req = config
            .resolve_alias(
                "glm",
                &args(&[
                    ("timeout-ms", "1500"),
                    ("idle-timeout-ms", "300"),
                    ("headers-timeout-ms", "500"),
                    ("stream", "false"),
                    ("temperature", "0"),
                ]),
                messages(),
            )
            .unwrap();
        assert_eq!(req.timeout_ms, 1500);
        assert_eq!(req.headers_timeout_ms, 500);
        assert_eq!(req.idle_timeout_ms, 300);
        assert!(!req.stream);
        assert_eq!(req.body["temperature"], 0.0);
        assert!(
            config
                .resolve_alias("kimi-k3", &Args::default(), messages())
                .unwrap()
                .body
                .get("temperature")
                .is_none()
        );
    }

    #[test]
    fn dead_aliases_require_legacy_and_follow_successor() {
        let config = config();
        assert!(
            config
                .resolve_alias("kimi-thinking", &Args::default(), messages())
                .is_err()
        );
        let mut args = Args::default();
        args.flags.insert("legacy".into());
        let req = config
            .resolve_alias("kimi-thinking", &args, messages())
            .unwrap();
        assert_eq!(req.provider, "kimi-k3");
        assert_eq!(req.effort.as_deref(), Some("high"));
    }

    #[test]
    fn dotenv_priority_and_comment_quotes_match_legacy_loader() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first.env");
        let second = dir.path().join("second.env");
        fs::write(&first, "A=cwd\nexport B='quoted # value' # ignored\nC=value#literal\nD=\"double\"\nEMPTY=filled\n").unwrap();
        fs::write(&second, "A=global\nB=global\nNEW=new\n").unwrap();
        let mut env = HashMap::from([
            ("A".into(), "process".into()),
            ("EMPTY".into(), String::new()),
        ]);
        load_dotenv(&first, &mut env).unwrap();
        load_dotenv(&second, &mut env).unwrap();
        assert_eq!(env["A"], "process");
        assert_eq!(env["B"], "quoted # value");
        assert_eq!(env["C"], "value#literal");
        assert_eq!(env["D"], "double");
        assert_eq!(env["EMPTY"], "filled");
        assert_eq!(env["NEW"], "new");
    }

    #[test]
    fn debug_and_metadata_do_not_contain_keys_or_messages() {
        let config = config();
        let req = config
            .resolve_alias("deepseek-pro", &Args::default(), messages())
            .unwrap();
        let debug = format!("{req:?}");
        assert!(!debug.contains("test-deepseek-key"));
        assert!(!debug.contains("fixture"));
        assert!(!debug.contains("https://"));
        let metadata = config
            .alias_metadata("deepseek-pro", &Args::default())
            .unwrap()
            .to_string();
        assert!(!metadata.contains("test-deepseek-key"));
        assert!(!metadata.contains("messages"));
    }
}
