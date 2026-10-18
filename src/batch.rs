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
fn cancelled(route: &str) -> Value {
    json!({"route":route,"status":"cancelled","complete":false,"model_calls":0,"error":"batch cancelled before adapter invocation"})
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
    for job in jobs.jobs {
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
    // Preflight every job before the first adapter invocation.
    let mut work = Vec::new();
    for (job, options, prompt) in prepared {
        work.push((ctx.job_output(&job.id)?, job, options, prompt));
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
                if let Some(result) =
                    review::resume_candidate(config, &routes, &options, &prompt, &child, false)
                        .await?
                {
                    return Ok(if *cancellation.borrow() {
                        cancelled(&job.route)
                    } else {
                        result
                    });
                }
                let mut history = Vec::new();
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
                    if !matches!(result["status"].as_str(), Some("quota" | "cooldown")) {
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
    println!(
        "{}",
        serde_json::to_string_pretty(
            &json!({"run_id":ctx.run_id,"status":status,"complete":complete,"results":results})
        )?
    );
    Ok(rc)
}
