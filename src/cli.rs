use crate::{args::Args, config::Config, runtime};
use anyhow::{Context, Result, bail};
use clap::{Arg, Command};
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

#[derive(Debug)]
pub struct UsageError(pub String);
impl std::fmt::Display for UsageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for UsageError {}
fn parse_args(tokens: &[String]) -> Result<Args> {
    Args::parse(tokens).map_err(|error| UsageError(error.to_string()).into())
}
fn validate_review_args(command: &str, args: &Args) -> Result<()> {
    let common = &["help", "json", "quiet"];
    let specific: &[&str] = match command {
        "batch" => &[
            "only",
            "exclude",
            "jobs",
            "outdir",
            "pool-limit",
            "rate-limit-retries",
            "retry-delay-ms",
            "timeout",
            "resume",
            "source-id",
            "task-type",
        ],
        "run" => &[
            "prompt-file",
            "prompt",
            "cwd",
            "out",
            "timeout",
            "input",
            "text-ready",
            "idle-timeout",
            "headers-timeout",
            "max-tokens",
            "resume",
            "source-id",
            "ignore-cooldown",
            "fallback",
            "rate-limit-retries",
            "retry-delay-ms",
            "task-type",
            "canary-file",
        ],
        "review" | "dispatch" => &[
            "task-type",
            "rate-limit-retries",
            "retry-delay-ms",
            "prompt-file",
            "prompt",
            "kind",
            "seats",
            "only",
            "exclude",
            "agentic",
            "no-metered",
            "dry-run",
            "cwd",
            "outdir",
            "timeout",
            "input",
            "text-ready",
            "idle-timeout",
            "headers-timeout",
            "max-tokens",
            "resume",
            "source-id",
            "no-quota",
        ],
        "verify" => &[
            "task-type",
            "rate-limit-retries",
            "retry-delay-ms",
            "findings",
            "by",
            "exclude",
            "cwd",
            "out",
            "timeout",
            "input",
            "text-ready",
            "idle-timeout",
            "headers-timeout",
            "max-tokens",
            "resume",
            "source-id",
            "no-metered",
            "no-quota",
        ],
        "probe" => &["only", "timeout"],
        "stats" => &["state-dir", "task-type"],
        "scorecard" => &["state-dir", "task-type", "since"],
        "recommend" => &["task-type", "jobs", "only", "exclude", "cwd"],
        "usage" | "status" => &[],
        "reset" => &["all"],
        "routes" => &[],
        "doctor" => &["live", "only", "timeout"],
        "outcome" => &["run", "report", "into", "all", "apply"],
        "feedback" => &[
            "batch",
            "from",
            "run",
            "report",
            "source",
            "human-confirmed",
            "accepted",
            "false-positive",
            "duplicate",
            "uncertain",
            "screen-min",
            "rework-min",
            "outcome",
            "rework-items",
            "rework-rounds",
            "defect-tags",
            "evidence",
        ],
        "install-config" => &["from"],
        "pack" => &["root", "input", "scope", "outdir"],
        "delegate" if args.positional.first().is_some_and(|v| v == "implement") => &[
            "task-type",
            "cwd",
            "out",
            "prompt-file",
            "timeout",
            "path-prepend",
            "watch",
            "allow-dirty",
            "network",
        ],
        "delegate" => &[
            "task-type",
            "packet",
            "prompt-file",
            "outdir",
            "fallback",
            "timeout",
            "resume",
        ],
        "_broker" => &["packet", "audit"],
        _ => return Ok(()),
    };
    if command == "delegate" && args.values.contains_key("network") {
        return Err(UsageError("--network is a flag and does not accept a value".into()).into());
    }
    for key in args.values.keys().chain(args.flags.iter()) {
        if !(common.contains(&key.as_str())
            || specific.contains(&key.as_str())
            || key == "card" && crate::ledger::tracked(command))
        {
            return Err(UsageError(format!("Unknown argument --{key} for {command}")).into());
        }
    }
    if command == "run" && args.positional.is_empty() {
        return Err(UsageError("run requires a route".into()).into());
    }
    if command == "verify" && !args.has("findings") {
        return Err(UsageError("verify requires --findings FILE".into()).into());
    }
    Ok(())
}

fn help_app() -> Command {
    let mut app=Command::new("multi-ai").version(env!("CARGO_PKG_VERSION"))
        .disable_help_subcommand(true)
        .about("Rust 原生多 AI 工具：明确通路、并行复核、可追溯报告")
        .after_help("API 本体不依赖 Python/Node；CLI 通路按需安装登录。\n旧 ai-review / llm-run 二进制别名已弃用，暂保留分派语义。help/routes/doctor 默认离线。");
    for (name, about) in [
        ("run", "指定通路；--timeout 单位秒"),
        (
            "batch",
            "批量任务；--jobs FILE --outdir DIR --pool-limit POOL=N；--resume",
        ),
        (
            "pack",
            "离线材料快照；--root DIR --input FILE/--scope DIR --outdir 新目录",
        ),
        (
            "delegate",
            "受限委派；ROLE TARGET --packet DIR --prompt-file FILE --outdir DIR",
        ),
        (
            "review",
            "并行复核；--kind review|explore|verify；按额度池备援",
        ),
        ("dispatch", "review 的兼容同义入口"),
        ("verify", "复核 --findings FILE"),
        ("text", "llm-run 原生文本功能；--timeout-ms 单位毫秒"),
        ("routes", "离线通路与模型来源；--json"),
        ("doctor", "离线配置与依赖检查；--live 才探测"),
        (
            "feedback",
            "按报告代录反馈；--run ID --report ID --evidence TEXT",
        ),
        (
            "outcome",
            "git 推断实施 outcome；suggest --run RUN --report ATTEMPT --into REF [--all] [--apply]",
        ),
        ("probe", "显式探测通路并调用模型"),
        ("usage", "查看已有账户额度"),
        ("status", "查看冷却，不代表已经验证健康"),
        ("stats", "离线汇总本地运行记录；--json / --state-dir DIR"),
        (
            "scorecard",
            "通路 × 任务类型成绩单；--task-type X --since DATE --json",
        ),
        (
            "recommend",
            "按成绩单建议分配；--task-type X --jobs N --json",
        ),
        ("reset", "清指定额度池冷却"),
        ("install-config", "导入配置，不覆盖已有文件、不复制凭据"),
        ("help", "离线帮助"),
    ] {
        app = app.subcommand(
            Command::new(name).about(about).arg(
                Arg::new("arguments")
                    .num_args(0..)
                    .trailing_var_arg(true)
                    .allow_hyphen_values(true),
            ),
        );
    }
    app
}
fn print_help(topic: Option<&str>, legacy: Option<&str>) -> Result<()> {
    let mut app = help_app();
    if let Some(topic) = topic {
        let topic = if topic == "dispatch" { "review" } else { topic };
        if let Some(sub) = app.find_subcommand_mut(topic) {
            sub.print_long_help()?;
        } else {
            app.print_long_help()?;
        }
    } else {
        app.print_long_help()?;
    }
    println!();
    if topic == Some("batch") {
        println!(
            "multi-ai batch --jobs jobs.json --outdir results [--pool-limit POOL=N] [--timeout SECONDS] [--resume]\n清单为 {{\"jobs\":[{{\"id\":\"one\",\"prompt_file\":\"task.md\",\"cwd\":\"repo\",\"route\":\"grok-cli\",\"fallback\":[]}}],\"pool_limits\":{{\"xai\":2}}}}；路径相对清单，默认每池并发2。\n每项独立 .md / .md.status.json；仅复用身份与正文哈希一致的完整报告。task_type 为自由字符串；canaries 为 [{{\"match\":\"src/a.rs:10\",\"expect\":\"真问题\"}}]；route:auto 按成绩分配，可用 --only/--exclude 收窄。"
        );
    } else if topic == Some("outcome") {
        println!(
            "multi-ai outcome suggest --run RUN --report ATTEMPT --into REF [--json] [--apply]\nmulti-ai outcome suggest --all --into REF [--json] [--apply]\n只读 git 建议；--apply 仅追加确定的 merged/discarded，跳过已有 outcome。unknown 永不写入，不推断 merged-after-rework；旧记录缺分支时 unknown。"
        );
    } else if topic == Some("scorecard") || topic == Some("recommend") {
        println!(
            "multi-ai scorecard [--task-type X] [--since YYYY-MM-DD] [--state-dir DIR] [--json]\nmulti-ai recommend --task-type X [--jobs N] [--cwd DIR] [--only ROUTE…] [--exclude ROUTE…] [--json]\n按真实调用、绑定报告的宿主反馈及金丝雀汇总。至少5次调用、5份成对核验和5份金丝雀报告才可比较；否则不下结论。约20%探索；冷却/本地缺失通路排除。小批次按历史样本补探索欠额。"
        );
    } else if topic == Some("pack") || topic == Some("delegate") {
        println!(
            "材料快照：\n  multi-ai pack --root DIR --input FILE --scope DIR --outdir NEW-DIR\n委派：\n  multi-ai delegate investigate kimi-cli-default --packet DIR --prompt-file TASK --outdir DIR --fallback grok-cli\n  multi-ai delegate extract deepseek-flash --packet DIR --prompt-file TASK --outdir DIR --resume --json\n角色：investigate|extract|challenge；--timeout 默认300秒（含备援）。\n--input/--scope/--fallback 可重复；默认stdout短交接，完整报告路径随结果返回。\n调查只开放快照的list/search/read；doctor展示隔离与版本可用性。\npack输出必须是新目录，快照immutable；--resume只复用身份完整匹配结果。"
        );
        println!(
            "实施：\n  multi-ai delegate implement codex-cli --cwd WORKTREE --prompt-file BRIEF --out SUMMARY [--timeout SECONDS] [--path-prepend DIR] [--allow-dirty] [--network] [--watch PATH]\n仅macOS；cwd须为Git工作树顶层，默认拒绝未提交修改；SUMMARY置于cwd之外。\n固定受控workspace-write；--network允许命令访问任意网络地址，网络写入无法事后核对。运行前后记录HEAD/status/文件变化；不提交，不支持resume。 "
        );
    } else if legacy == Some("llm-run") || topic == Some("text") {
        println!(
            "文本接口：\n  multi-ai text <alias> [prompt] --input FILE --out FILE [--json]\n  multi-ai text run --task NAME [--domain NAME] --input FILE\n  multi-ai text dispatch --domain NAME [--kind review|map|debate] --input FILE\n  multi-ai text profiles|domains|aliases [--json]\n  multi-ai text list-models [--json]  # 联网，需凭据\n\n--input 可重复，--input - 接收stdin；--image支持png/jpg/webp。\n--thinking auto|on|off，--effort low|high|max，--max-tokens N。\n--timeout-ms / --headers-timeout-ms / --idle-timeout-ms 单位毫秒。\n--stream / --no-stream；--quiet；--legacy。\n--resume 需 --out，同身份完整结果才复用；非Git恢复需 --source-id。\nstdout完整正文或JSON；stderr进度；partial不证明成功。"
        );
    } else {
        println!(
            "示例：\n  multi-ai routes --json\n  multi-ai run kimi-k3-cli --prompt-file evidence.md --cwd /path/to/repo --out review.md\n  multi-ai review --input evidence.md --text-ready --outdir reviews --seats 2\n  multi-ai verify --findings review.md --cwd /path/to/repo --out verify.md\n  multi-ai text glm --input evidence.md --out answer.md\n\nrun/review/verify：--timeout、--headers-timeout、--idle-timeout 单位秒。\n--only ROUTE… / --exclude ROUTE…；--no-metered；--agentic；--resume。\nAPI只读实际附带的正文，不能读取cwd路径；客户原件仅在明确授权范围使用。\n反馈：multi-ai feedback --run UUID --report ATTEMPT-ID --accepted N --evidence TEXT\n  [--false-positive N --duplicate N --uncertain N --screen-min N --rework-min N]\n实施反馈：[--outcome merged|merged-after-rework|discarded --rework-items N --rework-rounds N --defect-tags tag1,tag2]\n批量反馈：multi-ai feedback --batch OUTDIR --from verdicts.tsv（id accepted false_positive uncertain evidence；Tab分隔；可选 screen_min rework_min outcome rework_items rework_rounds defect_tags）\nrun/review/verify/batch/delegate 接受 --task-type X 与可选 --card CARD；run --canary-file FILE 接收本地已知答案 JSON 数组，不发送给模型。\n默认source=agent；人工决定需 --source human --human-confirmed 与明确用户证据。\n顺序复用自动保留历史；同输出并发立即失败；旧引擎缓存失配可能重新请求。\nreview --dry-run 保留额度采集行为；需要纯离线检查使用 routes 或 doctor。"
        );
    }
    Ok(())
}
fn find_executable(command: &str, config: &Config) -> Option<PathBuf> {
    let candidates = if command.contains('/') {
        vec![PathBuf::from(command)]
    } else {
        config
            .environment()
            .get("PATH")
            .map(|s| std::env::split_paths(s).map(|p| p.join(command)).collect())
            .unwrap_or_default()
    };
    candidates
        .into_iter()
        .find(|p| p.is_file() && fs::metadata(p).is_ok_and(|m| m.permissions().mode() & 0o111 != 0))
}
fn doctor(config: &Config, args: &Args) -> Result<i32> {
    let mut rows = Vec::new();
    let only = args.all("only");
    for name in &only {
        config.route(name, false)?;
    }
    for route in &config.routes {
        if !only.is_empty() && !only.contains(&route.id) {
            continue;
        }
        let mut row =
            json!({"route":route.id,"pool":route.pool,"family":route.family,"live_checked":false});
        row["delegation"] = crate::delegate::capability(config, route);
        if let Some(alias) = &route.api_alias {
            let meta = config.alias_metadata(alias, &Args::default())?;
            let provider = meta["provider"].as_str().context("provider missing")?;
            let available = !config.first_key(config.provider(provider)?).1.is_empty();
            row["kind"] = json!("api");
            row["credentials_configured"] = json!(available);
            row["ready_to_attempt"] = json!(available);
        } else {
            let executable = route.argv.first().context("missing route executable")?;
            let available = find_executable(executable, config).is_some();
            row["kind"] = json!("cli");
            row["executable_available"] = json!(available);
            row["login_verified"] = json!(false);
            row["ready_to_attempt"] = json!(available);
        }
        rows.push(row);
    }
    let value = json!({"engine":"multi-ai-rust","version":env!("CARGO_PKG_VERSION"),"platform":format!("{}-{}",std::env::consts::OS,std::env::consts::ARCH),
        "offline":true,"python_node_required":false,"routes":rows});
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(0)
}
fn install_config(args: &Args) -> Result<i32> {
    let directory = std::env::var_os("MULTI_AI_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| runtime::home().join(".config/multi-ai"));
    runtime::private_dir(&directory)?;
    let profiles = if let Some(path) = args.get("from") {
        let value =
            fs::read_to_string(path).with_context(|| format!("read configuration {path}"))?;
        let parsed: Value = serde_json::from_str(&value)?;
        if !parsed["providers"].is_object() || !parsed["aliases"].is_object() {
            bail!("source is not a model profile configuration");
        }
        value
    } else {
        include_str!("../config/profiles.json").into()
    };
    let mut created = Vec::new();
    let mut skipped = Vec::new();
    for (name, content) in [
        ("profiles.json", profiles.as_str()),
        ("routes.json", include_str!("../config/routes.json")),
    ] {
        let path = directory.join(name);
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
        {
            Ok(mut file) => {
                file.write_all(content.as_bytes())?;
                file.sync_all()?;
                created.push(path);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => skipped.push(path),
            Err(error) => return Err(error.into()),
        }
    }
    println!(
        "{}",
        serde_json::to_string_pretty(
            &json!({"created":created,"preserved":skipped,"credentials_copied":false})
        )?
    );
    Ok(0)
}

pub async fn main_entry(argv: Vec<String>) -> Result<i32> {
    let ledger = crate::ledger::Ledger::start(&argv);
    let result = main_entry_inner(argv).await;
    if let Some(ledger) = ledger {
        ledger.finish(match &result {
            Ok(code) => *code,
            Err(error) if error.downcast_ref::<UsageError>().is_some() => 2,
            Err(_) => 1,
        });
    }
    result
}

async fn main_entry_inner(argv: Vec<String>) -> Result<i32> {
    let executable = argv
        .first()
        .and_then(|p| Path::new(p).file_name())
        .and_then(|p| p.to_str())
        .unwrap_or("multi-ai");
    let legacy = match executable {
        "ai-review" => Some("ai-review"),
        "llm-run" => Some("llm-run"),
        _ => None,
    };
    let tokens = &argv[1..];
    if tokens
        .first()
        .is_some_and(|s| s == "--version" || s == "-V")
    {
        println!(
            "multi-ai {} (Rust native, macOS arm64 first release)",
            env!("CARGO_PKG_VERSION")
        );
        return Ok(0);
    }
    if tokens.is_empty() || tokens[0] == "help" || tokens[0] == "--help" || tokens[0] == "-h" {
        print_help(tokens.get(1).map(String::as_str), legacy)?;
        return Ok(0);
    }
    if legacy == Some("llm-run") {
        let command = &tokens[0];
        let raw = &tokens[1..];
        let args = if ["codex", "grok"].contains(&command.as_str()) {
            Args {
                raw: raw.to_vec(),
                ..Args::default()
            }
        } else {
            parse_args(raw)?
        };
        if args.has("help") {
            print_help(Some("text"), legacy)?;
            return Ok(0);
        }
        return crate::api::text_command(&Config::load()?, command, &args).await;
    }
    let command = &tokens[0];
    if command == "text" {
        let sub = tokens.get(1).map(String::as_str).unwrap_or("help");
        let raw = if tokens.len() > 1 { &tokens[2..] } else { &[] };
        let args = if ["codex", "grok"].contains(&sub) {
            Args {
                raw: raw.to_vec(),
                ..Args::default()
            }
        } else {
            parse_args(raw)?
        };
        if sub == "help" || args.has("help") {
            print_help(Some("text"), None)?;
            return Ok(0);
        }
        return crate::api::text_command(&Config::load()?, sub, &args).await;
    }
    let args = parse_args(&tokens[1..])?;
    if args.has("help") {
        print_help(Some(command), legacy)?;
        return Ok(0);
    }
    validate_review_args(command, &args)?;
    if command == "stats" {
        return crate::stats::command(&args);
    }
    if command == "scorecard" {
        return crate::scorecard::command(&args);
    }
    if command == "install-config" {
        return install_config(&args);
    }
    if command == "outcome" {
        return crate::outcome::command(&args);
    }
    if command == "feedback" {
        return runtime::feedback_command(&args);
    }
    if command == "pack" {
        return crate::delegate::pack_command(&args);
    }
    if command == "_broker" {
        crate::broker::serve(
            Path::new(args.get("packet").context("missing --packet")?),
            Path::new(args.get("audit").context("missing --audit")?),
        )
        .await?;
        return Ok(0);
    }
    if command == "delegate" && args.positional.first().is_some_and(|v| v == "implement") {
        crate::hooks::pre_delegate_implement(&args).await?;
    }
    let config = Config::load()?;
    match command.as_str() {
        "recommend" => crate::scorecard::recommend_command(&config, &args),
        "routes" => {
            let mut value = config.public_routes()?;
            for entry in value.as_array_mut().context("routes not array")? {
                let route = config.route(entry["id"].as_str().unwrap(), false)?;
                entry["capabilities"]["delegation"] = crate::delegate::capability(&config, &route);
            }
            if args.has("json") {
                runtime::stdout_line(format_args!("{}", serde_json::to_string_pretty(&value)?))?;
            } else {
                runtime::stdout_line(format_args!(
                    "{:<24} {:<17} {:<13} MODEL / SOURCE",
                    "ROUTE", "POOL", "FAMILY"
                ))?;
                for route in value.as_array().context("route list is not an array")? {
                    runtime::stdout_line(format_args!(
                        "{:<24} {:<17} {:<13} {} / {}",
                        route["id"].as_str().unwrap_or("?"),
                        route["pool"].as_str().unwrap_or("?"),
                        route["family"].as_str().unwrap_or("?"),
                        route["model"].as_str().unwrap_or("unknown"),
                        route["model_source"].as_str().unwrap_or("CLI default")
                    ))?;
                }
            }
            Ok(0)
        }
        "doctor" => {
            if args.has("live") {
                crate::review::command(&config, "probe", &args, legacy.is_some()).await
            } else {
                doctor(&config, &args)
            }
        }
        "delegate" if args.positional.first().is_some_and(|v| v == "implement") => {
            crate::implement::command(&config, &args).await
        }
        "delegate" => crate::delegate::command(&config, &args).await,
        "batch" => crate::batch::command(&config, &args).await,
        "run" | "review" | "dispatch" | "verify" | "probe" | "usage" | "status" | "reset" => {
            crate::review::command(
                &config,
                if command == "review" {
                    "dispatch"
                } else {
                    command
                },
                &args,
                legacy.is_some(),
            )
            .await
        }
        _ => bail!("Unknown command: {command}. Use multi-ai help."),
    }
}
