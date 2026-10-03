//! Native OpenAI-compatible text transport and the legacy llm-run command surface.
use crate::{
    args::Args,
    config::{Config, ResolvedRequest},
    runtime::{Attempt, OutputContext},
};
use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use futures_util::{StreamExt, future::join_all};
use regex::Regex;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fmt,
    io::Read,
    time::{Duration, Instant},
};
use tokio::time::{Instant as TokioInstant, sleep, timeout_at};

#[derive(Debug)]
pub struct CompletionError {
    pub code: &'static str,
    pub message: String,
    pub retryable: bool,
}
impl fmt::Display for CompletionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Error [{}]: {}", self.code, self.message)
    }
}
impl std::error::Error for CompletionError {}
fn error(code: &'static str, message: impl Into<String>, retryable: bool) -> anyhow::Error {
    CompletionError {
        code,
        message: message.into(),
        retryable,
    }
    .into()
}
fn code_of(error: &anyhow::Error) -> &'static str {
    error
        .downcast_ref::<CompletionError>()
        .map(|e| e.code)
        .unwrap_or("error")
}
fn redact(text: &str, key: &str) -> String {
    let mut text = if key.is_empty() {
        text.to_owned()
    } else {
        text.replace(key, "[REDACTED]")
    };
    static PATTERNS: std::sync::OnceLock<Vec<Regex>> = std::sync::OnceLock::new();
    let patterns = PATTERNS.get_or_init(|| {
        [
            r"\bsk-[A-Za-z0-9_-]{16,}\b",
            r"\bAKIA[A-Z0-9]{16}\b",
            r"\bgh[pousr]_[A-Za-z0-9_]{20,}\b",
            r"\bgithub_pat_[A-Za-z0-9_]{40,}\b",
            r"\bxox[baprs]-[A-Za-z0-9-]{20,}\b",
        ]
        .into_iter()
        .map(|pattern| Regex::new(pattern).expect("constant regex"))
        .collect()
    });
    for pattern in patterns {
        text = pattern
            .replace_all(&text, "[REDACTED_API_KEY]")
            .into_owned();
    }
    text
}
fn assert_safe(config: &Config, text: &str) -> Result<()> {
    if config
        .environment()
        .get("LLM_RUN_ALLOW_CLIENT_DATA")
        .map(String::as_str)
        == Some("1")
    {
        return Ok(());
    }
    let hit = if text.contains("统一社会信用代码")
        && Regex::new(r"[0-9A-HJ-NPQRTUWXY]{2}\d{6}[0-9A-HJ-NPQRTUWXY]{10}")?.is_match(text)
    {
        Some("unified-social-credit")
    } else if text.contains("合同金额") && Regex::new(r"[¥￥]?\s*[\d,]{6,}")?.is_match(text) {
        Some("contract-amount")
    } else {
        None
    };
    if let Some(hit) = hit {
        bail!(
            "Refusing to send likely client data ({hit}) to an API model. For explicitly authorized material, set LLM_RUN_ALLOW_CLIENT_DATA=1 for this call only; no separate desensitization audit is required."
        );
    }
    Ok(())
}

/// Holds a possible credential prefix across HTTP chunks before writing raw bytes.
struct RawRedactor {
    key: Vec<u8>,
    pending: Vec<u8>,
}
impl RawRedactor {
    fn new(key: &str) -> Self {
        Self {
            key: key.as_bytes().to_vec(),
            pending: vec![],
        }
    }
    fn push(&mut self, bytes: &[u8], attempt: &Attempt) -> Result<()> {
        if self.key.is_empty() {
            return attempt.append("response.raw", bytes);
        }
        self.pending.extend_from_slice(bytes);
        let mut out = Vec::new();
        let mut at = 0;
        while self.pending.len() - at >= self.key.len() {
            if self.pending[at..].starts_with(&self.key) {
                out.extend_from_slice(b"[REDACTED]");
                at += self.key.len();
            } else {
                out.push(self.pending[at]);
                at += 1;
            }
        }
        self.pending.drain(..at);
        if !out.is_empty() {
            attempt.append("response.raw", &out)?;
        }
        Ok(())
    }
    fn finish(&mut self, attempt: &Attempt) -> Result<()> {
        if !self.pending.is_empty() {
            attempt.append("response.raw", &self.pending)?;
            self.pending.clear();
        }
        Ok(())
    }
}
#[derive(Default)]
struct Metrics {
    first_data: Option<u64>,
    first_content: Option<u64>,
    last_data: Option<u64>,
    bytes: u64,
    headers: u64,
}
struct ProgressNotice {
    enabled: bool,
    last_data_ms: u64,
}
impl ProgressNotice {
    fn emit(&mut self, kind: &str, extra: Value, metrics: &Metrics, start: Instant) {
        let now = elapsed(start);
        if !self.enabled || kind == "data" && now.saturating_sub(self.last_data_ms) < 5000 {
            return;
        }
        self.last_data_ms = now;
        let mut row = json!({"type":kind,"elapsedMs":now,"bytes":metrics.bytes,"firstDataMs":metrics.first_data,"firstContentMs":metrics.first_content,"lastDataMs":metrics.last_data});
        if let Some(extra) = extra.as_object() {
            for (key, value) in extra {
                row[key] = value.clone();
            }
        }
        eprintln!("[llm-run-progress] {row}");
    }
}

fn elapsed(start: Instant) -> u64 {
    start.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
}
fn timings(metrics: &Metrics, start: Instant) -> Value {
    json!({"headersMs":metrics.headers,"firstDataMs":metrics.first_data,"firstContentMs":metrics.first_content,"lastDataMs":metrics.last_data,"totalMs":elapsed(start)})
}

pub fn validate_completion(
    content: &str,
    reasoning_chars: usize,
    finish_reason: Option<&str>,
    terminal: bool,
) -> Result<()> {
    if finish_reason == Some("length") {
        return Err(error(
            "truncated",
            "Output budget exhausted (finish_reason=length). Split the material or increase --max-tokens.",
            false,
        ));
    }
    if let Some(reason) = finish_reason
        && reason != "stop"
    {
        return Err(error(
            "incomplete",
            format!("Unexpected finish_reason={reason}"),
            false,
        ));
    }
    if !terminal {
        return Err(error(
            "incomplete",
            "Response ended without a completion marker; partial output was preserved.",
            false,
        ));
    }
    if content.trim().is_empty() {
        return Err(error(
            "empty",
            format!(
                "No final content ({reasoning_chars} reasoning chars). Split the material or increase --max-tokens."
            ),
            false,
        ));
    }
    Ok(())
}
#[derive(Default)]
struct Completion {
    content: String,
    reasoning_chars: usize,
    usage: Value,
    model: String,
    finish_reason: Option<String>,
    done: bool,
    pending: Vec<u8>,
}
impl Completion {
    fn line(
        &mut self,
        line: &[u8],
        attempt: &Attempt,
        metrics: &mut Metrics,
        start: Instant,
        key: &str,
    ) -> Result<()> {
        let line = std::str::from_utf8(line)
            .map_err(|_| error("incomplete", "Malformed UTF-8 in SSE data", false))?
            .trim();
        let Some(payload) = line.strip_prefix("data:") else {
            return Ok(());
        };
        let payload = payload.trim();
        if payload.is_empty() {
            return Ok(());
        }
        if payload == "[DONE]" {
            self.done = true;
            return Ok(());
        }
        let item: Value = serde_json::from_str(payload).map_err(|_| {
            error(
                "incomplete",
                "Malformed SSE data; partial output was preserved",
                false,
            )
        })?;
        if !item["error"].is_null() {
            return Err(error(
                "incomplete",
                "Provider reported an error inside the stream",
                false,
            ));
        }
        if !item["usage"].is_null() {
            self.usage = item["usage"].clone();
        }
        if let Some(model) = item["model"].as_str() {
            self.model = model.to_owned();
        }
        if let Some(choice) = item["choices"].as_array().and_then(|v| v.first()) {
            if let Some(reason) = choice["finish_reason"].as_str() {
                self.finish_reason = Some(reason.to_owned());
            }
            if let Some(reasoning) = choice["delta"]["reasoning_content"].as_str() {
                self.reasoning_chars += reasoning.encode_utf16().count();
            }
            if let Some(content) = choice["delta"]["content"].as_str()
                && !content.is_empty()
            {
                metrics.first_content.get_or_insert(elapsed(start));
                self.content.push_str(content);
                attempt.write("answer.partial.txt", redact(&self.content, key).as_bytes())?;
            }
        }
        attempt.update(json!({"contentChars":self.content.encode_utf16().count(),"reasoningChars":self.reasoning_chars,"usage":self.usage,"finishReason":self.finish_reason,"streamDone":self.done,"firstContentMs":metrics.first_content}))?;
        Ok(())
    }
    fn bytes(
        &mut self,
        bytes: &[u8],
        attempt: &Attempt,
        metrics: &mut Metrics,
        start: Instant,
        key: &str,
    ) -> Result<()> {
        self.pending.extend_from_slice(bytes);
        while let Some(n) = self.pending.iter().position(|b| *b == b'\n') {
            let line = self.pending.drain(..=n).collect::<Vec<_>>();
            self.line(&line, attempt, metrics, start, key)?;
        }
        Ok(())
    }
}
#[allow(clippy::too_many_arguments)] // Carries the one request deadline and durable progress state.
async fn read_response<F>(
    response: reqwest::Response,
    deadline: TokioInstant,
    idle_ms: u64,
    attempt: &Attempt,
    metrics: &mut Metrics,
    start: Instant,
    redactor: &mut RawRedactor,
    notice: &mut ProgressNotice,
    mut on_bytes: F,
) -> Result<()>
where
    F: FnMut(&[u8], &mut Metrics) -> Result<()>,
{
    let mut stream = response.bytes_stream();
    loop {
        if TokioInstant::now() >= deadline {
            return Err(error(
                "total_timeout",
                "Total request budget exhausted, including retries",
                false,
            ));
        }
        let idle_deadline = TokioInstant::now() + Duration::from_millis(idle_ms);
        let limit = deadline.min(idle_deadline);
        let next = timeout_at(limit, stream.next()).await.map_err(|_| {
            if TokioInstant::now() >= deadline {
                error(
                    "total_timeout",
                    "Total request budget exhausted, including retries",
                    false,
                )
            } else {
                error(
                    "idle_timeout",
                    format!("No response data for {idle_ms}ms"),
                    false,
                )
            }
        })?;
        let Some(bytes) = next else {
            break;
        };
        let bytes = bytes.map_err(|_| {
            error(
                "transport",
                "Response transport failed; partial output was preserved",
                false,
            )
        })?;
        metrics.bytes += bytes.len() as u64;
        let now = elapsed(start);
        metrics.first_data.get_or_insert(now);
        metrics.last_data = Some(now);
        redactor.push(&bytes, attempt)?;
        on_bytes(&bytes, metrics)?;
        attempt.update(json!({"bytes":metrics.bytes,"firstDataMs":metrics.first_data,"lastDataMs":metrics.last_data}))?;
        attempt.event(
            "data",
            json!({"bytes":metrics.bytes,"lastDataMs":metrics.last_data}),
        )?;
        notice.emit("data", json!({}), metrics, start);
    }
    Ok(())
}

pub async fn request(spec: &ResolvedRequest, attempt: &Attempt) -> Result<Value> {
    request_impl(
        spec,
        attempt,
        std::env::var("LLM_RUN_PROGRESS").as_deref() == Ok("1"),
    )
    .await
}
async fn configured_request(
    config: &Config,
    spec: &ResolvedRequest,
    attempt: &Attempt,
) -> Result<Value> {
    request_impl(
        spec,
        attempt,
        config
            .environment()
            .get("LLM_RUN_PROGRESS")
            .map(String::as_str)
            == Some("1"),
    )
    .await
}
async fn request_impl(
    spec: &ResolvedRequest,
    attempt: &Attempt,
    progress_enabled: bool,
) -> Result<Value> {
    let start = Instant::now();
    let deadline = TokioInstant::now() + Duration::from_millis(spec.timeout_ms);
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let input = serde_json::to_vec(&spec.body["messages"])?;
    attempt.update(json!({"provider":spec.provider,"model":spec.model,"modelSource":spec.model_source,"inputSha256":hex::encode(Sha256::digest(&input)),"inputBytes":input.len(),"maxTokens":spec.body["max_tokens"],"stream":spec.stream,"thinking":spec.thinking,"effort":spec.effort,"timeoutMs":spec.timeout_ms,"headersTimeoutMs":spec.headers_timeout_ms,"idleTimeoutMs":spec.idle_timeout_ms,"status":"running"}))?;
    let mut metrics = Metrics::default();
    let mut redactor = RawRedactor::new(&spec.key);
    let mut notice = ProgressNotice {
        enabled: progress_enabled,
        last_data_ms: 0,
    };
    notice.emit(
        "started",
        json!({"provider":spec.provider,"model":redact(&spec.model,&spec.key)}),
        &metrics,
        start,
    );
    let result = {
        let operation = async {
            for index in 0..=spec.retries {
                if TokioInstant::now() >= deadline {
                    return Err(error(
                        "total_timeout",
                        "Total request budget exhausted, including retries",
                        false,
                    ));
                }
                attempt.event("request", json!({"attempt":index+1}))?;
                notice.emit("request", json!({"attempt":index+1}), &metrics, start);
                let sent = Instant::now();
                let header_deadline = (TokioInstant::now()
                    + Duration::from_millis(spec.headers_timeout_ms))
                .min(deadline);
                let mut success_body = false;
                let result=async {
                let response=timeout_at(header_deadline,client.post(&spec.url).bearer_auth(&spec.key).json(&spec.body).send()).await
                    .map_err(|_| if TokioInstant::now()>=deadline {error("total_timeout","Total request budget exhausted, including retries",false)} else {error("headers_timeout",format!("No HTTP response headers within {}ms",spec.headers_timeout_ms),true)})?
                    .map_err(|_|error("transport","Request failed before response headers",true))?;
                metrics.headers=elapsed(sent);let status=response.status();
                attempt.event("headers",json!({"attempt":index+1,"headersMs":metrics.headers,"httpStatus":status.as_u16()}))?;
                notice.emit("headers",json!({"attempt":index+1,"headersMs":metrics.headers,"httpStatus":status.as_u16()}),&metrics,start);
                if !status.is_success() {
                    let mut text=Vec::new();
                    read_response(response,deadline,spec.idle_timeout_ms,attempt,&mut metrics,start,&mut redactor,&mut notice,|b,_|{ if text.len()<8192 {text.extend_from_slice(&b[..b.len().min(8192-text.len())]);}Ok(()) }).await?;
                    let code=match status.as_u16() {401|403=>"auth",429|402=>"quota",500..=599=>"transport",_=>"http_error"};
                    let message=redact(&String::from_utf8_lossy(&text),&spec.key).chars().take(1200).collect::<String>();
                    return Err(error(code,format!("HTTP {status}: {message}"),status.as_u16()==429||status.is_server_error()));
                }
                let mut complete=Completion {model:spec.model.clone(),..Default::default()};
                if spec.stream {
                    read_response(response,deadline,spec.idle_timeout_ms,attempt,&mut metrics,start,&mut redactor,&mut notice,|b,m|{
                        success_body=true;complete.bytes(b,attempt,m,start,&spec.key)
                    }).await?;
                    if !complete.pending.is_empty() {let tail=std::mem::take(&mut complete.pending);complete.line(&tail,attempt,&mut metrics,start,&spec.key)?;}
                    validate_completion(&complete.content,complete.reasoning_chars,complete.finish_reason.as_deref(),complete.done||matches!(complete.finish_reason.as_deref(),Some("stop"|"length")))?;
                } else {
                    let mut bytes=Vec::new();
                    read_response(response,deadline,spec.idle_timeout_ms,attempt,&mut metrics,start,&mut redactor,&mut notice,|b,_|{success_body=true;bytes.extend_from_slice(b);Ok(())}).await?;
                    let item:Value=serde_json::from_slice(&bytes).map_err(|_|error("incomplete","Provider returned invalid JSON",false))?;
                    if !item["error"].is_null() {return Err(error("incomplete","Provider returned an error payload",false));}
                    let choice=&item["choices"][0];
                    complete.content=choice["message"]["content"].as_str().ok_or_else(||error("incomplete","Provider returned non-text content",false))?.to_owned();
                    complete.reasoning_chars=choice["message"]["reasoning_content"].as_str().map(|s|s.encode_utf16().count()).unwrap_or(0);
                    complete.usage=item["usage"].clone();complete.model=item["model"].as_str().unwrap_or(&spec.model).to_owned();
                    complete.finish_reason=choice["finish_reason"].as_str().map(str::to_owned);
                    if !complete.content.is_empty() {metrics.first_content=Some(elapsed(start));attempt.write("answer.partial.txt",redact(&complete.content,&spec.key).as_bytes())?;}
                    validate_completion(&complete.content,complete.reasoning_chars,complete.finish_reason.as_deref(),complete.finish_reason.is_some())?;
                }
                let content=redact(&complete.content,&spec.key);
                let result=json!({"provider":spec.provider,"providerName":spec.provider,"model":redact(&complete.model,&spec.key),"modelSource":spec.model_source,"content":content,"usage":complete.usage,"thinking":spec.thinking,"effort":spec.effort,"seat":null,"reasoningChars":complete.reasoning_chars,"finishReason":complete.finish_reason,"complete":true,"progressDir":attempt.dir,"timings":timings(&metrics,start),"runId":attempt.run_id,"attemptId":attempt.id,"run_id":attempt.run_id,"attempt_id":attempt.id});
                Ok(result)
            }.await;
                match result {
                    Ok(result) => return Ok(result),
                    Err(failure) => {
                        let retryable = failure
                            .downcast_ref::<CompletionError>()
                            .is_some_and(|e| e.retryable);
                        if retryable && !success_body && TokioInstant::now() >= deadline {
                            return Err(error(
                                "total_timeout",
                                "Total request budget exhausted, including retries",
                                false,
                            ));
                        }
                        if retryable
                            && !success_body
                            && index < spec.retries
                            && TokioInstant::now() < deadline
                        {
                            let delay = Duration::from_millis(
                                1000u64.saturating_mul(2u64.saturating_pow(index)),
                            )
                            .min(deadline.saturating_duration_since(TokioInstant::now()));
                            attempt.event("retry",json!({"attempt":index+1,"reason":code_of(&failure),"delayMs":delay.as_millis()}))?;
                            notice.emit("retry",json!({"attempt":index+1,"reason":code_of(&failure),"delayMs":delay.as_millis()}),&metrics,start);
                            sleep(delay).await;
                        } else {
                            return Err(failure);
                        }
                    }
                }
            }
            Err(error("transport", "Request attempts exhausted", false))
        };
        let mut interrupt =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
        let mut termination =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! {
            result=operation=>result,
            _=interrupt.recv()=>Err(error("cancelled","Request cancelled by SIGINT; partial output was preserved",false)),
            _=termination.recv()=>Err(error("cancelled","Request cancelled by SIGTERM; partial output was preserved",false)),
        }
    };
    redactor.finish(attempt)?;
    match result {
        Ok(result) => {
            notice.emit(
                "complete",
                json!({"finishReason":result["finishReason"],"totalMs":elapsed(start)}),
                &metrics,
                start,
            );
            attempt.finish("complete", result.clone())?;
            Ok(result)
        }
        Err(failure) => {
            let code = code_of(&failure);
            let message = redact(&failure.to_string(), &spec.key);
            notice.emit(
                code,
                json!({"error":message,"totalMs":elapsed(start)}),
                &metrics,
                start,
            );
            attempt.finish(
                code,
                json!({"error":message,"complete":false,"timings":timings(&metrics,start)}),
            )?;
            Err(error(
                code,
                format!("{message} (progress: {})", attempt.dir.display()),
                false,
            ))
        }
    }
}

fn cache_setup(
    config: &Config,
    args: &Args,
    ctx: &OutputContext,
    request_identities: Value,
) -> Result<Option<(std::path::PathBuf, Value)>> {
    if args.has("resume") && ctx.output.is_none() {
        bail!("text --resume requires --out");
    }
    let Some(output) = &ctx.output else {
        return Ok(None);
    };
    let cwd = std::env::current_dir()?;
    let workspace =
        match crate::runtime::workspace_identity(&cwd, args.get("source-id"), &ctx.excludes) {
            Ok(workspace) => workspace,
            Err(failure) if !args.has("resume") => {
                note(
                    config,
                    args,
                    &format!(
                        "source identity unavailable; this call will not be cached: {failure}"
                    ),
                );
                None
            }
            Err(failure) => return Err(failure),
        };
    if workspace.is_none() {
        if args.has("resume") {
            bail!("text --resume outside Git requires --source-id");
        }
        return Ok(None);
    }
    let executable = std::env::current_exe()?;
    let engine_sha256 = crate::runtime::sha256(&std::fs::read(executable)?);
    let identity = json!({"engineVersion":env!("CARGO_PKG_VERSION"),"engineSha256":engine_sha256,"workspace":workspace,"requests":request_identities,
        "configSha256":crate::runtime::sha256(&serde_json::to_vec(&json!({"profiles":config.profiles,"routes":config.routes}))?)});
    Ok(Some((
        std::path::PathBuf::from(format!("{}.cache.json", output.display())),
        identity,
    )))
}
fn request_identity(spec: &ResolvedRequest) -> Result<Value> {
    Ok(
        json!({"provider":spec.provider,"model":spec.model,"modelSource":spec.model_source,"bodySha256":crate::runtime::sha256(&serde_json::to_vec(&spec.body)?),
        "endpointSha256":crate::runtime::sha256(spec.url.as_bytes()),"timeoutMs":spec.timeout_ms,"headersTimeoutMs":spec.headers_timeout_ms,"idleTimeoutMs":spec.idle_timeout_ms,"retries":spec.retries}),
    )
}
fn bind_invocation(result: &mut Value, ctx: &OutputContext, attempt: &Attempt) {
    result["runId"] = json!(ctx.run_id);
    result["run_id"] = json!(ctx.run_id);
    result["attemptId"] = json!(attempt.id);
    result["attempt_id"] = json!(attempt.id);
}

async fn managed_request(
    config: &Config,
    args: &Args,
    ctx: &OutputContext,
    spec: &ResolvedRequest,
    attempt: &Attempt,
) -> Result<Value> {
    let cache = cache_setup(config, args, ctx, request_identity(spec)?)?;
    if args.has("resume")
        && let Some((path, identity)) = &cache
        && let Some(mut result) = crate::runtime::cache_get(path, identity)?
    {
        bind_invocation(&mut result, ctx, attempt);
        attempt.finish("complete",json!({"cached":true,"complete":true,"model_calls":0,"reused_from":result["reused_from"]}))?;
        note(
            config,
            args,
            "resume matched complete report; no model request",
        );
        return Ok(result);
    }
    if args.has("resume") {
        note(
            config,
            args,
            "resume identity did not match; a new model request may be billed",
        );
    }
    let result = configured_request(config, spec, attempt).await?;
    if let Some((path, identity)) = &cache {
        crate::runtime::cache_put(
            path,
            identity,
            &result,
            result["content"].as_str().unwrap_or(""),
        )?;
    }
    Ok(result)
}

async fn read_inputs(config: &Config, args: &Args) -> Result<String> {
    let mut chunks = Vec::new();
    let mut stdin = None;
    for input in args.all("input") {
        let (label, text) = if input == "-" {
            if stdin.is_none() {
                let mut text = String::new();
                std::io::stdin().read_to_string(&mut text)?;
                stdin = Some(text);
            }
            ("STDIN".to_owned(), stdin.as_ref().unwrap().clone())
        } else {
            let path = std::path::absolute(&input)?;
            (
                format!("FILE: {}", path.display()),
                tokio::fs::read_to_string(&path)
                    .await
                    .with_context(|| format!("Cannot read input {}", path.display()))?,
            )
        };
        if text.trim().is_empty() {
            bail!("Input {label} is empty");
        }
        chunks.push(format!("--- {label} ---\n{}", redact(&text, "")));
    }
    let content = chunks.join("\n\n");
    assert_safe(config, &content)?;
    Ok(content)
}
async fn read_images(args: &Args) -> Result<Vec<Value>> {
    let mut images = Vec::new();
    for name in args.all("image") {
        let path = std::path::absolute(&name)?;
        let bytes = tokio::fs::read(&path).await?;
        if bytes.len() > 5 * 1024 * 1024 {
            bail!("Image {} exceeds 5MB", path.display());
        }
        let extension = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();
        let mime = match extension.as_str() {
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "webp" => "image/webp",
            _ => bail!(
                "Unsupported image type for {} — use png, jpg, or webp",
                path.display()
            ),
        };
        images.push(json!({"type":"image_url","image_url":{"url":format!("data:{mime};base64,{}",STANDARD.encode(bytes))}}));
    }
    Ok(images)
}
fn messages(question: &str, content: &str, system: String, images: Vec<Value>) -> Value {
    let question = redact(question, "");
    let text = if content.trim().is_empty() {
        question.clone()
    } else {
        format!("{question}\n\n材料：\n{content}")
    };
    let user = if images.is_empty() {
        json!(text)
    } else {
        let mut parts = vec![json!({"type":"text","text":text})];
        parts.extend(images);
        json!(parts)
    };
    json!([{"role":"system","content":system},{"role":"user","content":user}])
}
fn kind_task(kind: &str) -> Option<&'static str> {
    match kind {
        "compress" | "summarize" => Some("summarize"),
        "extract" => Some("extract"),
        "review" => Some("review"),
        "think" => Some("think"),
        "debate" | "roundtable" => Some("roundtable"),
        "map" => Some("map"),
        "verify" => Some("verify"),
        "red-team" => Some("red-team"),
        "code-review" => Some("code-review"),
        "vision" => Some("vision-review"),
        _ => None,
    }
}
fn task_name(args: &Args, domain: &Value) -> String {
    args.get("task")
        .or_else(|| args.get("kind").and_then(kind_task))
        .or_else(|| domain["task"].as_str())
        .unwrap_or("summarize")
        .to_owned()
}
fn domain(config: &Config, args: &Args) -> Result<Value> {
    if let Some(name) = args.get("domain") {
        let value = &config.profiles["domains"][name];
        if value.is_null() {
            bail!("Unknown domain: {name}");
        }
        Ok(value.clone())
    } else {
        Ok(Value::Null)
    }
}
fn note(config: &Config, args: &Args, message: &str) {
    if !args.has("quiet")
        && config
            .environment()
            .get("LLM_RUN_QUIET")
            .map(String::as_str)
            != Some("1")
    {
        eprintln!("[llm-run] {message}");
    }
}
fn alias_name(config: &Config, alias: &str, args: &Args) -> Result<String> {
    let mut name = alias.to_owned();
    let mut seen = std::collections::HashSet::new();
    loop {
        let profile = &config.profiles["aliases"][&name];
        if profile.is_null() {
            bail!("Unknown alias: {name}");
        }
        if !seen.insert(name.clone()) {
            bail!("Alias successor cycle: {name}");
        }
        if profile["dead"] != true {
            return Ok(name);
        }
        let successor = profile["successor"]
            .as_str()
            .context("DEAD alias has no successor")?;
        if !args.has("legacy") {
            bail!("Alias '{name}' is DEAD. Use '{successor}'. (Pass --legacy to override.)");
        }
        eprintln!("[llm-run] WARN DEAD alias {name} -> {successor} via --legacy");
        name = successor.to_owned();
    }
}
fn alias_prompt_key(config: &Config, alias: &Value) -> String {
    if let Some(key) = alias["promptKey"].as_str() {
        return key.to_owned();
    }
    let provider = &config.profiles["providers"][alias["provider"].as_str().unwrap_or("")];
    if provider["reasoningStyle"] == "k3" {
        "think-k3".into()
    } else if alias["thinking"] == "on" {
        "think".into()
    } else {
        "extract".into()
    }
}
pub async fn execute_alias(
    config: &Config,
    alias: &str,
    args: &Args,
    prompt: &str,
    attempt: &Attempt,
) -> Result<Value> {
    let alias = alias_name(config, alias, args)?;
    let profile = &config.profiles["aliases"][&alias];
    let prompt = redact(prompt, "");
    assert_safe(config, &prompt)?;
    let spec = config.resolve_alias(
        &alias,
        args,
        messages(
            &prompt,
            "",
            config.system_prompt(Some(&alias_prompt_key(config, profile)), None),
            vec![],
        ),
    )?;
    note(
        config,
        args,
        &format!(
            "alias={alias} provider={} model={} thinking={} effort={} stream={} timeout={}s",
            spec.provider,
            spec.model,
            spec.thinking,
            spec.effort.as_deref().unwrap_or("—"),
            spec.stream,
            spec.timeout_ms / 1000
        ),
    );
    let mut result = configured_request(config, &spec, attempt).await?;
    result["providerName"] = config.provider(&spec.provider)?["displayName"].clone();
    Ok(result)
}
fn result_format(result: &Value, as_json: bool) -> Result<String> {
    if as_json {
        Ok(serde_json::to_string_pretty(result)?)
    } else {
        Ok(result["content"].as_str().unwrap_or("").to_owned())
    }
}
fn roundtable_format(results: &[Value], question: &str, as_json: bool) -> Result<String> {
    if as_json {
        return Ok(serde_json::to_string_pretty(
            &json!({"question":question,"results":results}),
        )?);
    }
    let mut text = format!(
        "# Multi-LLM Roundtable\n\nGenerated at: {}\n\nQuestion: {question}\n\nThis is R1 (independent seats in parallel). Cross-exam is /round, not this file.\n\n",
        chrono::Utc::now().to_rfc3339()
    );
    for result in results {
        let seat = result["seat"]
            .as_str()
            .map(|s| format!(" seat={s}"))
            .unwrap_or_default();
        text.push_str(&format!(
            "## {} ({}){seat}\n\n{}\n\n",
            result["providerName"].as_str().unwrap_or("unknown"),
            result["model"].as_str().unwrap_or("unknown"),
            result["content"].as_str().unwrap_or("")
        ));
    }
    let metadata=results.iter().map(|r|json!({"provider":r["provider"],"model":r["model"],"seat":r["seat"],"thinking":r["thinking"],"effort":r["effort"],"reasoningChars":r["reasoningChars"],"usage":r["usage"],"error":r["error"].as_bool().unwrap_or(false)})).collect::<Vec<_>>();
    text.push_str(&format!(
        "## Metadata\n\n```json\n{}\n```\n",
        serde_json::to_string_pretty(&metadata)?
    ));
    Ok(text)
}

async fn run_roundtable(
    config: &Config,
    args: &Args,
    domain: &Value,
    ctx: &OutputContext,
    content: &str,
) -> Result<i32> {
    if args.has("image") {
        bail!("roundtable does not take --image; use run --task vision-review --image FILE");
    }
    let task = &config.profiles["tasks"]["roundtable"];
    let name = args
        .get("set")
        .or_else(|| domain["roundtable"].as_str())
        .or_else(|| task["set"].as_str())
        .unwrap_or("default");
    let set = config.profiles["roundtableSets"][name]
        .as_array()
        .with_context(|| format!("Unknown roundtable set: {name}"))?;
    if set.is_empty() {
        bail!("Roundtable set {name} has no seats");
    }
    let prompt = args.prompt();
    let question = args
        .get("question")
        .filter(|s| !s.is_empty())
        .or_else(|| {
            if prompt.trim().is_empty() {
                None
            } else {
                Some(prompt.as_str())
            }
        })
        .or_else(|| task["defaultQuestion"].as_str())
        .unwrap_or("请独立评审这些材料。");
    note(
        config,
        args,
        &format!("roundtable set={name} seats={} (parallel)", set.len()),
    );
    let mut planned = Vec::new();
    let mut identities = Vec::new();
    for item in set {
        let provider = item["provider"].as_str().unwrap_or("");
        let seat = item["seat"].as_str().unwrap_or("");
        let mut defaults = task.clone();
        if let Some(map) = defaults.as_object_mut()
            && let Some(item) = item.as_object()
        {
            map.extend(item.clone());
        }
        let mut overrides = args.clone();
        // A fixed seat model wins; --model otherwise only applies to opts.provider's seat.
        if item["model"].is_string() || args.get("provider") != Some(provider) {
            overrides.values.remove("model");
        }
        let role = item["model"]
            .as_str()
            .or_else(|| item["modelRole"].as_str())
            .unwrap_or("fast");
        let spec = config.resolve_request(
            provider,
            role,
            &overrides,
            messages(
                question,
                content,
                config.system_prompt(None, Some(seat)),
                vec![],
            ),
            &defaults,
        );
        identities.push(match &spec {Ok(spec)=>request_identity(spec)?,Err(failure)=>json!({"provider":provider,"seat":seat,"resolutionErrorSha256":crate::runtime::sha256(failure.to_string().as_bytes())})});
        planned.push((item.clone(), spec));
    }
    let cache = cache_setup(
        config,
        args,
        ctx,
        json!({"roundtable":identities,"question":question,"json":args.has("json")}),
    )?;
    if args.has("resume")
        && let Some((path, identity)) = &cache
        && let Some(cached) = crate::runtime::cache_get(path, identity)?
    {
        let mut results = cached["results"]
            .as_array()
            .context("Invalid cached roundtable results")?
            .clone();
        for result in &mut results {
            let attempt = ctx.attempt(result["seat"].as_str().unwrap_or("roundtable"))?;
            result["reused_from"] = json!({"run_id":result["run_id"],"attempt_id":result["attempt_id"],"report_sha256":crate::runtime::sha256(result["content"].as_str().unwrap_or("").as_bytes())});
            result["cached"] = json!(true);
            bind_invocation(result, ctx, &attempt);
            ctx.report(&attempt, result["content"].as_str().unwrap_or(""), result)?;
        }
        let output = if args.has("json") {
            roundtable_format(&results, question, true)?
        } else {
            cached["content"]
                .as_str()
                .context("Cached roundtable output missing")?
                .to_owned()
        };
        ctx.publish(&output)?;
        ctx.save_status(json!({"status":"complete","complete":true,"cached":true,"results":results,"reused_from":cached["reused_from"]}))?;
        if ctx.output.is_none() {
            println!("{}", output.trim_end());
        } else {
            println!("Wrote {}", ctx.output.as_ref().unwrap().display());
        }
        note(
            config,
            args,
            "resume matched complete roundtable; no model requests",
        );
        return Ok(0);
    }
    if args.has("resume") {
        note(
            config,
            args,
            "roundtable resume identity did not match; new requests may be billed",
        );
    }
    let jobs=planned.iter().map(|(item,spec)|async move {
        let provider=item["provider"].as_str().unwrap_or("");let seat=item["seat"].as_str().unwrap_or("");let attempt=ctx.attempt(if seat.is_empty(){provider}else{seat})?;
        let result=async {
            let spec=spec.as_ref().map_err(|failure|anyhow::anyhow!("{failure}"))?;
            let mut result=configured_request(config,spec,&attempt).await?;result["seat"]=json!(seat);result["providerName"]=config.provider(&spec.provider)?["displayName"].clone();
            ctx.report(&attempt,result["content"].as_str().unwrap_or(""),&result)?;Ok::<_,anyhow::Error>(result)
        }.await;
        match result {Ok(result)=>Ok(result),Err(failure)=>{let message=redact(&failure.to_string(),"");attempt.finish(code_of(&failure),json!({"error":message,"complete":false}))?;Ok(json!({"provider":provider,"providerName":provider,"model":item["model"].as_str().unwrap_or("unknown"),"seat":seat,"content":format!("ERROR: {message}"),"error":true,"complete":false,"attemptId":attempt.id,"runId":attempt.run_id,"attempt_id":attempt.id,"run_id":attempt.run_id}))}}
    });
    let results = join_all(jobs)
        .await
        .into_iter()
        .collect::<Result<Vec<Value>>>()?;
    let failed = results.iter().any(|r| r["error"] == true);
    let formatted = roundtable_format(&results, question, args.has("json"))?;
    if failed {
        ctx.save_status(json!({"status":"incomplete","complete":false,"results":results,"error":"Some roundtable seats failed; previous complete report was preserved"}))?;
        crate::runtime::atomic_text(&ctx.dir.join("roundtable.partial"), &formatted)?;
        if let Some(path) = &ctx.output {
            ctx.publish_to(
                &std::path::PathBuf::from(format!("{}.partial", path.display())),
                &formatted,
            )?;
        } else {
            println!("{formatted}");
        }
        eprintln!(
            "Error [incomplete]: Some roundtable seats failed; only partial results were delivered"
        );
        Ok(1)
    } else {
        if let Some((path, identity)) = &cache {
            // Aggregate artifact is a cache owner, not an extra model report or model call.
            let aggregate = ctx.attempt("roundtable-aggregate")?;
            aggregate.write("report.md", formatted.as_bytes())?;
            aggregate.finish("complete",json!({"complete":true,"model_calls":0,"reportSha256":crate::runtime::sha256(formatted.as_bytes())}))?;
            crate::runtime::cache_put(
                path,
                identity,
                &json!({"complete":true,"run_id":ctx.run_id,"attempt_id":aggregate.id,"results":results}),
                &formatted,
            )?;
        }
        ctx.publish(&formatted)?;
        if ctx.output.is_none() {
            println!("{}", formatted.trim_end());
        } else {
            println!("Wrote {}", ctx.output.as_ref().unwrap().display());
        }
        ctx.save_status(json!({"status":"complete","complete":true,"results":results}))?;
        Ok(0)
    }
}

async fn execute_text(
    config: &Config,
    command: &str,
    args: &Args,
    ctx: &OutputContext,
) -> Result<i32> {
    let domain = domain(config, args)?;
    let task_name = task_name(args, &domain);
    let content = read_inputs(config, args).await?;
    if (command == "run" || command == "dispatch")
        && (task_name == "roundtable"
            || command == "dispatch" && args.get("kind") == Some("debate"))
    {
        return run_roundtable(config, args, &domain, ctx, &content).await;
    }
    let images = read_images(args).await?;
    let positional = args.prompt();
    let attempt = ctx.attempt(command)?;
    let mut result = if command == "run" || command == "dispatch" {
        let task = &config.profiles["tasks"][&task_name];
        if task.is_null() {
            bail!("Unknown task: {task_name}");
        }
        if task_name == "vision-review" && images.is_empty() {
            bail!("vision-review requires --image FILE (png/jpg/webp)");
        }
        let provider = args
            .get("provider")
            .filter(|p| *p != "auto")
            .or_else(|| domain["provider"].as_str())
            .or_else(|| task["provider"].as_str())
            .with_context(|| format!("Task {task_name} requires explicit --provider"))?;
        let role = domain["model"]
            .as_str()
            .or_else(|| task["modelRole"].as_str())
            .unwrap_or("fast");
        let question = args
            .get("question")
            .filter(|s| !s.is_empty())
            .or_else(|| {
                if positional.trim().is_empty() {
                    None
                } else {
                    Some(positional.as_str())
                }
            })
            .or_else(|| task["defaultQuestion"].as_str())
            .unwrap_or("请处理材料。");
        let mut defaults = task.clone();
        if let Some(map) = defaults.as_object_mut() {
            for key in ["thinking", "effort"] {
                if !domain[key].is_null() {
                    map.insert(key.into(), domain[key].clone());
                }
            }
        }
        let spec = config.resolve_request(
            provider,
            role,
            args,
            messages(
                question,
                &content,
                config.system_prompt(Some(task["promptKey"].as_str().unwrap_or(&task_name)), None),
                images,
            ),
            &defaults,
        )?;
        note(
            config,
            args,
            &format!(
                "task={task_name} provider={} model={} thinking={} stream={} timeout={}s",
                spec.provider,
                spec.model,
                spec.thinking,
                spec.stream,
                spec.timeout_ms / 1000
            ),
        );
        let mut result = managed_request(config, args, ctx, &spec, &attempt).await?;
        result["providerName"] = config.provider(&spec.provider)?["displayName"].clone();
        result
    } else {
        let name = alias_name(config, command, args)?;
        let alias = &config.profiles["aliases"][&name];
        let question = args
            .get("question")
            .filter(|s| !s.is_empty())
            .unwrap_or(&positional);
        if question.trim().is_empty() && content.trim().is_empty() && images.is_empty() {
            bail!("usage: llm-run {command} \"<prompt>\" [--input FILE] [--image FILE]");
        }
        let question = if question.trim().is_empty() {
            "请处理材料。"
        } else {
            question
        };
        let spec = config.resolve_alias(
            &name,
            args,
            messages(
                &redact(question, ""),
                &content,
                config.system_prompt(Some(&alias_prompt_key(config, alias)), None),
                images,
            ),
        )?;
        assert_safe(config, question)?;
        note(
            config,
            args,
            &format!(
                "alias={name} provider={} model={} thinking={} stream={} timeout={}s",
                spec.provider,
                spec.model,
                spec.thinking,
                spec.stream,
                spec.timeout_ms / 1000
            ),
        );
        let mut result = managed_request(config, args, ctx, &spec, &attempt).await?;
        result["providerName"] = config.provider(&spec.provider)?["displayName"].clone();
        result
    };
    bind_invocation(&mut result, ctx, &attempt);
    ctx.report(&attempt, result["content"].as_str().unwrap_or(""), &result)?;
    let output = result_format(&result, args.has("json"))?;
    ctx.publish(&output)?;
    if ctx.output.is_none() {
        println!("{}", output.trim_end());
    } else {
        println!("Wrote {}", ctx.output.as_ref().unwrap().display());
    }
    ctx.save_status(json!({"status":"complete","complete":true,"result":result}))?;
    Ok(0)
}

fn usage(config: &Config) -> String {
    let keys = |key: &str| {
        config.profiles[key]
            .as_object()
            .map(|m| m.keys().cloned().collect::<Vec<_>>().join("|"))
            .unwrap_or_default()
    };
    format!(
        "Usage:\n  llm-run dispatch --domain {} [--kind compress|review|debate|map|…] [options]\n  llm-run run --task {} [options]\n  llm-run <alias> \"<prompt>\" [--input FILE]\n  llm-run profiles|domains|aliases [--json]\n  llm-run list-models [--json]\n  llm-run codex|grok <args...>\n\nOptions:\n  --provider auto|{}  --model MODEL  --thinking auto|on|off\n  --effort low|high|max  --input FILE (repeatable, - for stdin)\n  --image FILE (repeatable PNG/JPG/WebP, at most 5MB each)\n  --question TEXT  --out FILE  --max-tokens N  --temperature N\n  --timeout-ms N  --headers-timeout-ms N  --idle-timeout-ms N\n  --progress-dir DIR  --stream|--no-stream  --legacy  --quiet  --json\n\n快速上手：\n  llm-run glm --input evidence.md --out answer.md\n  multi-ai text run --task extract --input evidence.md\n帮助与 profiles/domains/aliases 只读本地配置，不联网、不调用模型。\n实时正文位于 progressDir/answer.partial.txt；只有完整结果才能发布 --out。\nheaders/idle/total timeout 分别表示等响应头、无数据间隔、总预算（含重试）。\n文本 API 看不见仓库，需通过 --input 附正文。\n",
        keys("domains"),
        keys("tasks"),
        keys("providers")
    )
}
fn render_config(config: &Config, command: &str, args: &Args) -> Result<String> {
    let data = match command {
        "profiles" => {
            json!({"tasks":config.profiles["tasks"],"providers":config.profiles["providers"]})
        }
        "domains" => config.profiles["domains"].clone(),
        _ => config.profiles["aliases"].clone(),
    };
    if args.has("json") {
        return Ok(serde_json::to_string_pretty(&data)?);
    }
    let mut lines = vec![
        match command {
            "profiles" => "Task profiles:",
            "domains" => "Domains (llm-run dispatch --domain NAME):",
            _ => "One-shot aliases — llm-run <alias> <prompt>:",
        }
        .to_owned(),
        String::new(),
    ];
    let table = if command == "profiles" {
        &config.profiles["tasks"]
    } else {
        &data
    };
    for (name, entry) in table.as_object().into_iter().flatten() {
        if command == "domains" {
            let class = entry["class"].as_str().unwrap_or("—");
            let cap = &config.profiles["capabilityClasses"][class];
            let via = if cap["llmRun"] == true {
                format!("task={}", entry["task"].as_str().unwrap_or("—"))
            } else {
                format!("ADAPTER={}", cap["adapter"].as_str().unwrap_or("unknown"))
            };
            lines.push(format!(
                "  {name:16} {class:18} {via:30} {}",
                entry["description"].as_str().unwrap_or("")
            ));
        } else if entry["dead"] == true {
            lines.push(format!(
                "  {name:16} DEAD → {} {}",
                entry["successor"].as_str().unwrap_or("unknown"),
                entry["desc"].as_str().unwrap_or("")
            ));
        } else if name == "roundtable" && command == "profiles" {
            lines.push(format!(
                "  {name:16} set={} {}",
                entry["set"].as_str().unwrap_or("default"),
                entry["description"].as_str().unwrap_or("")
            ));
        } else {
            let provider = entry["provider"].as_str().unwrap_or("unknown");
            let role = entry["model"]
                .as_str()
                .or_else(|| entry["modelRole"].as_str())
                .unwrap_or("fast");
            let metadata = if command == "aliases" {
                config.alias_metadata(name, &Args::default())?
            } else {
                config.request_metadata(provider, role, &Args::default(), entry)?
            };
            let model = metadata["model"].as_str().unwrap_or("unknown");
            let provider = metadata["provider"].as_str().unwrap_or(provider);
            lines.push(format!(
                "  {name:16} {provider:10} {model:28} {}",
                entry["description"]
                    .as_str()
                    .or_else(|| entry["desc"].as_str())
                    .unwrap_or("")
            ));
        }
    }
    Ok(lines.join("\n"))
}
async fn list_models(config: &Config, args: &Args) -> Result<String> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_millis(args.number("timeout-ms", 120000)?))
        .build()?;
    let mut providers = serde_json::Map::new();
    for (name, provider) in config.profiles["providers"]
        .as_object()
        .context("providers must be an object")?
    {
        if name == "moonshot" || name == "kimi-k3" {
            continue;
        }
        let (key_name, key) = config.first_key(provider);
        let result = if key.is_empty() {
            json!({"ok":false,"error":format!("missing {key_name}"),"models":[]})
        } else {
            let endpoint = format!(
                "{}{}",
                provider["baseUrl"]
                    .as_str()
                    .unwrap_or("")
                    .trim_end_matches('/'),
                provider["modelsPath"].as_str().unwrap_or("/models")
            );
            match client.get(endpoint).bearer_auth(&key).send().await {
                Err(_) => json!({"ok":false,"error":"Request failed or timed out","models":[]}),
                Ok(response) => {
                    let status = response.status();
                    let bytes = response.bytes().await?;
                    let text = redact(&String::from_utf8_lossy(&bytes), &key);
                    match serde_json::from_str::<Value>(&text) {
                        Ok(value) if status.is_success() => {
                            json!({"ok":true,"status":status.as_u16(),"models":value["data"].as_array().map(|a|a.iter().filter_map(|v|v["id"].as_str()).collect::<Vec<_>>()).unwrap_or_default()})
                        }
                        _ => {
                            json!({"ok":false,"status":status.as_u16(),"error":text.chars().take(800).collect::<String>(),"models":[]})
                        }
                    }
                }
            }
        };
        providers.insert(name.clone(), result);
    }
    let result = json!({"generatedAt":chrono::Utc::now().to_rfc3339(),"note":"Providers kimi, kimi-k3 and moonshot share one endpoint; its models are listed once under kimi.","providers":providers});
    if args.has("json") {
        return Ok(serde_json::to_string_pretty(&result)?);
    }
    let mut lines = vec![];
    for (name, info) in result["providers"].as_object().unwrap() {
        lines.push(format!(
            "{}:",
            config.profiles["providers"][name]["displayName"]
                .as_str()
                .unwrap_or(name)
        ));
        if info["ok"] == true {
            for model in info["models"].as_array().unwrap() {
                lines.push(format!("  {}", model.as_str().unwrap_or("")));
            }
        } else {
            lines.push(format!(
                "  error: {}",
                info["error"].as_str().unwrap_or("unknown")
            ));
        }
        lines.push(String::new());
    }
    lines.push(format!("Note: {}", result["note"].as_str().unwrap()));
    Ok(lines.join("\n"))
}

pub async fn text_command(config: &Config, command: &str, args: &Args) -> Result<i32> {
    if command.is_empty()
        || ["help", "--help", "-h"].contains(&command)
        || args.has("help") && !matches!(command, "codex" | "grok")
    {
        print!("{}", usage(config));
        return Ok(0);
    }
    if matches!(command, "codex" | "grok") {
        return crate::runtime::passthrough(command, &args.raw).await;
    }
    if matches!(command, "profiles" | "domains" | "aliases") {
        println!("{}", render_config(config, command, args)?);
        return Ok(0);
    }
    if command == "list-models" {
        println!("{}", list_models(config, args).await?);
        return Ok(0);
    }
    if command == "dispatch" {
        let domain = domain(config, args)?;
        if domain.is_null() {
            bail!("dispatch requires --domain. Run llm-run domains for the table.");
        }
        let class = domain["class"].as_str().context("Domain has no class")?;
        let cap = &config.profiles["capabilityClasses"][class];
        if cap.is_null() {
            bail!("Domain has unknown class {class}");
        }
        if cap["llmRun"] != true {
            let follow = domain["followupTask"]
                .as_str()
                .map(|task| {
                    format!("\nAfter the evidence packet: llm-run run --task {task} --input PACKET")
                })
                .unwrap_or_default();
            let text = format!(
                "[llm-run] ADAPTER={}\ndomain={} class={class}\n\n{}{follow}\n",
                cap["adapter"].as_str().unwrap_or("unknown"),
                args.get("domain").unwrap_or(""),
                cap["instruction"].as_str().unwrap_or("")
            );
            crate::runtime::offline_publish(args, &text)?;
            if !args.has("out") {
                print!("{text}");
            }
            return Ok(0);
        }
    }
    if !matches!(command, "run" | "dispatch") && config.profiles["aliases"][command].is_null() {
        bail!("Unknown command: {command}\n{}", usage(config));
    }
    let ctx = OutputContext::new(args, &format!("text:{command}"), false)?;
    let result = execute_text(config, command, args, &ctx).await;
    match result {
        Ok(rc) => {
            ctx.finish(rc)?;
            Ok(rc)
        }
        Err(failure) => {
            ctx.save_status(
                json!({"status":code_of(&failure),"complete":false,"error":failure.to_string()}),
            )?;
            ctx.finish(1)?;
            Err(failure)
        }
    }
}
