use crate::{args::Args, config::Config, review, runtime::OutputContext};
use anyhow::{Context, Result, bail};
use futures_util::{StreamExt, stream::FuturesUnordered};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::sync::{Semaphore, watch};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Job {
    id: String,
    prompt_file: PathBuf,
    cwd: PathBuf,
    route: String,
    #[serde(default)]
    fallback: Vec<String>,
    source_id: Option<String>,
    #[serde(default)]
    text_ready: bool,
    task_type: Option<String>,
    #[serde(default)]
    canaries: Vec<crate::canary::Canary>,
    #[serde(skip)]
    resume_routes: Vec<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Jobs {
    jobs: Vec<Job>,
    #[serde(default)]
    pool_limits: HashMap<String, usize>,
}
fn relative(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_owned()
    } else {
        base.join(path)
    }
}
/// Definite route failures; timeout, transport and rate_limit may be transient and stay in.
const PREFLIGHT_FATAL: &[&str] = &[
    "model", "auth", "quota", "cooldown", "capacity", "error", "empty",
];
/// One tiny call per distinct route before any job, so a dead route (rejected model id,
/// arrears, logged out) is skipped instead of failing minutes into each job.
async fn preflight(config: &Config, ids: &[String], args: &Args) -> Result<HashMap<String, Value>> {
    let routes = ids
        .iter()
        .map(|id| config.route(id, false))
        .collect::<Result<Vec<_>>>()?;
    let temp = crate::runtime::state_dir()
        .join("probe")
        .join(uuid::Uuid::new_v4().to_string());
    fs::create_dir_all(&temp)?;
    let mut probe = Args::default();
    probe.set("cwd", temp.display());
    probe.set("timeout", args.get("preflight-timeout").unwrap_or("90"));
    probe.flags.insert("text-ready".into());
    let ctx = OutputContext::new(&probe, "preflight", false)?;
    let mut pending: FuturesUnordered<_> = routes
        .iter()
        .map(|r| review::run_one(config, r, &probe, "只回复两个字：正常", &ctx, false))
        .collect();
    let mut failed = HashMap::new();
    while let Some(result) = pending.next().await {
        let mut result = review::without_output(&result?);
        let status = result["status"].as_str().unwrap_or("error").to_owned();
        let route = result["route"].as_str().unwrap_or("-").to_owned();
        if PREFLIGHT_FATAL.contains(&status.as_str()) {
            eprintln!(
                "preflight: skip {route} for this batch: {status} {}",
                result["error"].as_str().unwrap_or("")
            );
            result["preflight"] = json!(true);
            result["error"] = json!(format!(
                "preflight: {}",
                result["error"].as_str().unwrap_or(&status)
            ));
            failed.insert(route, result);
        }
    }
    drop(pending);
    ctx.finish(0)?;
    let _ = fs::remove_dir_all(&temp);
    Ok(failed)
}
fn cancelled(route: &str) -> Value {
    json!({"route":route,"status":"cancelled","complete":false,"model_calls":0,"error":"batch cancelled before adapter invocation"})
}
/// Earlier routes of a job that fell back, as `route:status(error)`; None without fallback.
fn fallback_chain(row: &Value) -> Option<String> {
    let attempts = row["route_attempts"].as_array()?;
    let (_, earlier) = attempts.split_last()?;
    if earlier.is_empty() {
        return None;
    }
    Some(
        earlier
            .iter()
            .map(|a| {
                let error: String = a["error"].as_str().unwrap_or("").chars().take(80).collect();
                format!(
                    "{}:{}({error})",
                    a["route"].as_str().unwrap_or("-"),
                    a["status"].as_str().unwrap_or("-")
                )
            })
            .collect::<Vec<_>>()
            .join(" → "),
    )
}
pub async fn command(config: &Config, args: &Args) -> Result<i32> {
    let path = Path::new(args.get("jobs").context("batch requires --jobs FILE")?);
    let mut jobs: Jobs = serde_json::from_slice(&fs::read(path)?)?;
    if jobs.jobs.is_empty() {
        bail!("jobs must not be empty");
    }
    for item in args.all("pool-limit") {
        let (pool, n) = item
            .split_once('=')
            .context("--pool-limit requires POOL=N")?;
        jobs.pool_limits.insert(pool.into(), n.parse()?);
    }
    if jobs
        .pool_limits
        .values()
        .any(|n| *n == 0 || *n > Semaphore::MAX_PERMITS)
    {
        bail!("pool limits must be positive and within semaphore capacity");
    }
    let base = path.parent().unwrap_or(Path::new("."));
    let mut ids = HashSet::new();
    let mut prepared = Vec::new();
    let mut pools = HashMap::new();
    let mut auto_routes = HashMap::new();
    for job in &jobs.jobs {
        if job.route == "auto" {
            let task = job
                .task_type
                .as_deref()
                .or(args.get("task-type"))
                .context("route auto requires task_type")?;
            auto_routes
                .entry((
                    task.to_owned(),
                    job.text_ready,
                    fs::canonicalize(relative(base, &job.cwd))?,
                ))
                .or_insert_with(Vec::new);
        }
    }
    for ((task, text_ready, cwd), routes) in &mut auto_routes {
        let count = jobs
            .jobs
            .iter()
            .filter(|j| {
                j.route == "auto"
                    && j.text_ready == *text_ready
                    && j.task_type.as_deref().or(args.get("task-type")) == Some(task.as_str())
                    && fs::canonicalize(relative(base, &j.cwd)).is_ok_and(|p| p == *cwd)
            })
            .count();
        let mut selection = args.clone();
        selection.set("cwd", cwd.to_string_lossy());
        *routes = match crate::scorecard::allocation(config, task, count, *text_ready, &selection) {
            Ok(routes) => routes,
            // Cooldown does not invalidate an intact cached result. A route hint is
            // only used for a strict cache probe below, never as proof of success.
            Err(error)
                if args.has("resume")
                    && error.to_string() == "no eligible routes outside cooldown" =>
            {
                Vec::new()
            }
            Err(error) => return Err(error),
        };
        routes.reverse();
    }
    for mut job in jobs.jobs {
        if job.id.is_empty()
            || job.id == "."
            || job.id == ".."
            || !job
                .id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            || !ids.insert(job.id.clone())
        {
            bail!("invalid or duplicate job id: {}", job.id);
        }
        let automatic = job.route == "auto";
        if automatic {
            if args.has("resume")
                && let Some(outdir) = args.get("outdir")
            {
                let path = Path::new(outdir).join(format!("{}.md.cache.json", job.id));
                let cached = fs::symlink_metadata(&path)
                    .ok()
                    .filter(|m| m.is_file() && m.len() <= 32 * 1024 * 1024)
                    .and_then(|_| fs::read(path).ok())
                    .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
                if let Some(cached) = cached
                    && let Some(actual) = cached["result"]["route"].as_str()
                {
                    let primary = cached["result"]["auto_primary"].as_str().unwrap_or(actual);
                    let allowed = |id: &str| {
                        (args.all("only").is_empty() || args.all("only").iter().any(|v| v == id))
                            && !args.all("exclude").iter().any(|v| v == id)
                            && config
                                .route(id, false)
                                .is_ok_and(|r| r.agentic || job.text_ready)
                    };
                    if allowed(primary)
                        && (allowed(actual) || job.fallback.iter().any(|r| r == actual))
                    {
                        job.resume_routes
                            .extend([primary.to_owned(), actual.to_owned()]);
                    }
                }
            }

            let task_type = job
                .task_type
                .as_deref()
                .or(args.get("task-type"))
                .context("route auto requires task_type")?;
            job.route = auto_routes
                .get_mut(&(
                    task_type.to_owned(),
                    job.text_ready,
                    fs::canonicalize(relative(base, &job.cwd))?,
                ))
                .unwrap()
                .pop()
                .or_else(|| job.resume_routes.first().cloned())
                .context("no eligible auto route or authorized resume candidate")?;
        }
        crate::canary::validate(&job.canaries)?;
        for name in std::iter::once(&job.route).chain(job.fallback.iter()) {
            let route = config.route(name, false)?;
            pools.entry(route.pool.clone()).or_insert_with(|| {
                Arc::new(Semaphore::new(
                    *jobs.pool_limits.get(&route.pool).unwrap_or(&2),
                ))
            });
        }
        let prompt = fs::read_to_string(relative(base, &job.prompt_file))?;
        let cwd = fs::canonicalize(relative(base, &job.cwd))?;
        if !cwd.is_dir() {
            bail!("job cwd must be a directory");
        }
        let mut options = args.clone();
        if automatic {
            options.set("auto-primary", &job.route);
        }
        if let Some(task_type) = &job.task_type {
            options.set("task-type", task_type);
        }
        options.set("canaries-json", serde_json::to_string(&job.canaries)?);
        options.set("cwd", cwd.to_string_lossy());
        if let Some(source) = &job.source_id {
            options.set("source-id", source);
        }
        if job.text_ready {
            options.flags.insert("text-ready".into());
        }
        prepared.push((job, options, prompt));
    }
    args.get("outdir").context("batch requires --outdir DIR")?;
    let ctx = OutputContext::new(args, "batch", true)?;
    let unavailable = if args.has("preflight") {
        let mut routes = Vec::new();
        for (job, _, _) in &prepared {
            for id in std::iter::once(&job.route).chain(job.fallback.iter()) {
                if !routes.contains(id) {
                    routes.push(id.clone());
                }
            }
        }
        preflight(config, &routes, args).await?
    } else {
        HashMap::new()
    };
    let unavailable = &unavailable;
    // Preflight every job before the first adapter invocation.
    let mut work = Vec::new();
    for (job, options, prompt) in prepared {
        let mut child = ctx.job_output(&job.id)?;
        child.task_type = options.get("task-type").map(str::to_owned);
        child.canaries = job.canaries.clone();
        work.push((child, job, options, prompt));
    }
    let (cancel_tx, cancel_rx) = watch::channel(false);
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut pending = FuturesUnordered::new();
    for (child, job, options, prompt) in work {
        let pools = &pools;
        let mut cancellation = cancel_rx.clone();
        pending.push(async move {
            let operation = async {
                if *cancellation.borrow() {
                    return Ok(cancelled(&job.route));
                }
                let routes = std::iter::once(&job.route)
                    .chain(job.fallback.iter())
                    .map(|id| config.route(id, false))
                    .collect::<Result<Vec<_>>>()?;
                let budget: f64 = options
                    .get("timeout")
                    .map(str::parse)
                    .transpose()?
                    .unwrap_or(routes[0].default_timeout as f64);
                if !budget.is_finite() || budget <= 0.0 {
                    bail!("timeout must be finite and positive");
                }
                let budget_duration = std::time::Duration::try_from_secs_f64(budget)
                    .context("timeout exceeds supported duration")?;
                if std::time::Instant::now().checked_add(budget_duration).is_none() {
                    bail!("timeout exceeds supported duration");
                }
                let resume_routes = if job.resume_routes.is_empty() { routes.clone() } else {
                    job.resume_routes.iter().map(|id|config.route(id,false)).collect::<Result<Vec<_>>>()?
                };
                if let Some(result) =
                    review::resume_candidate(config, &resume_routes, &options, &prompt, &child, false)
                        .await?
                {
                    return Ok(if *cancellation.borrow() {
                        cancelled(&job.route)
                    } else {
                        result
                    });
                }
                // Routes that failed preflight are skipped but stay visible in the chain.
                let mut history: Vec<Value> = routes
                    .iter()
                    .filter_map(|r| unavailable.get(&r.id).cloned())
                    .collect();
                let routes: Vec<_> = routes
                    .into_iter()
                    .filter(|r| !unavailable.contains_key(&r.id))
                    .collect();
                if routes.is_empty() {
                    let mut result = history[0].clone();
                    result["model_calls"] = json!(0);
                    result["route_attempts"] = json!(history);
                    return Ok(result);
                }
                let mut result = json!({});
                let mut started: Option<std::time::Instant> = None;
                for route in &routes {
                    if *cancellation.borrow() {
                        result = cancelled(&route.id);
                        break;
                    }
                    let expired = || json!({"route":route.id,"pool":route.pool,"status":"timeout","complete":false,"model_calls":0,"error":"total_timeout: task budget exhausted while waiting for quota pool"});
                    let permit = if let Some(start) = started {
                        let remaining = budget - start.elapsed().as_secs_f64();
                        if remaining <= 0.0 {
                            result = expired();
                            break;
                        }
                        tokio::select! {
                            biased;
                            _ = cancellation.changed() => {
                                result = cancelled(&route.id);
                                break;
                            }
                            permit = tokio::time::timeout(
                                std::time::Duration::from_secs_f64(remaining),
                                pools[&route.pool].acquire(),
                            ) => match permit {
                                Ok(permit) => permit?,
                                Err(_) => {
                                    result = expired();
                                    break;
                                }
                            }
                        }
                    } else {
                        // A job's budget starts when its first pool slot becomes available.
                        tokio::select! {
                            biased;
                            _ = cancellation.changed() => {
                                result = cancelled(&route.id);
                                break;
                            }
                            permit = pools[&route.pool].acquire() => permit?,
                        }
                    };
                    let _permit = permit;
                    if *cancellation.borrow() {
                        result = cancelled(&route.id);
                        break;
                    }
                    let start = started.get_or_insert_with(std::time::Instant::now);
                    let seconds = budget - start.elapsed().as_secs_f64();
                    if seconds <= 0.0 {
                        result = expired();
                        break;
                    }
                    let mut remaining = options.clone();
                    remaining.set("policy-timeout", budget);
                    remaining.set("timeout", seconds);
                    result = review::run_retry(config, route, &remaining, &prompt, &child, false)
                        .await?;
                    history.push(review::without_output(&result));
                    if !matches!(result["status"].as_str(), Some("quota" | "cooldown" | "capacity" | "model")) {
                        break;
                    }
                }
                result["route_attempts"] = json!(history);
                Ok::<Value, anyhow::Error>(result)
            }
            .await;
            let mut result = operation.unwrap_or_else(
                |e| json!({"status":"error","complete":false,"error":format!("{e:#}")}),
            );
            result["id"] = json!(job.id);
            result["task_type"] = json!(child.task_type);
            if result["status"] == "ok"
                && let Err(e) = child.publish(result["output"].as_str().unwrap_or(""))
            {
                result["status"] = json!("output_error");
                result["complete"] = json!(false);
                result["error"] = json!(e.to_string());
            }
            let mut result = review::without_output(&result);
            if let Err(e) = child.save_status(result.clone()) {
                result["status"] = json!("output_error");
                result["complete"] = json!(false);
                result["error"] = json!(e.to_string());
            }
            result
        });
    }
    let mut results = Vec::new();
    while !pending.is_empty() {
        let next = tokio::select! {
            biased;
            _ = interrupt.recv(), if !*cancel_tx.borrow() => {
                cancel_tx.send_replace(true);
                None
            }
            _ = terminate.recv(), if !*cancel_tx.borrow() => {
                cancel_tx.send_replace(true);
                None
            }
            next = pending.next() => next,
        };
        let Some(result) = next else {
            continue;
        };
        if result["status"] == "cancelled" {
            cancel_tx.send_replace(true);
        }
        results.push(result);
        ctx.save_status(json!({"status":"running","results":results}))?;
    }
    results.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
    let stopped = *cancel_tx.borrow();
    let complete = !stopped && results.iter().all(|v| v["status"] == "ok");
    let status = if stopped {
        "cancelled"
    } else if complete {
        "complete"
    } else {
        "incomplete"
    };
    ctx.save_status(json!({"status":status,"complete":complete,"results":results}))?;
    let rc = i32::from(!complete);
    ctx.finish(rc)?;
    eprintln!("id\troute\tstatus\tseconds\trun_id\tattempt_id\tcanary\tfallback");
    for row in &results {
        let text = |key: &str| row[key].as_str().unwrap_or("-");
        eprintln!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            text("id"),
            text("route"),
            text("status"),
            row["seconds"]
                .as_f64()
                .map_or_else(|| "-".into(), |n| format!("{n:.1}")),
            ctx.run_id,
            text("attempt_id"),
            if row["canary_total"].is_number() {
                format!("{}/{}", row["canary_hits"], row["canary_total"])
            } else {
                "-".into()
            },
            fallback_chain(row).unwrap_or_else(|| "-".into())
        );
    }
    // A fallback hides why the planned route failed; say it where the host reads.
    for row in &results {
        if let Some(chain) = fallback_chain(row) {
            eprintln!(
                "⚠️  {} 回落：{chain} → {}",
                row["id"].as_str().unwrap_or("-"),
                row["route"].as_str().unwrap_or("-")
            );
        }
    }
    println!(
        "{}",
        serde_json::to_string_pretty(
            &json!({"run_id":ctx.run_id,"status":status,"complete":complete,"results":results})
        )?
    );
    Ok(rc)
}
