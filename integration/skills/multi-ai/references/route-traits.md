# Route traits from real batches

Measured on 2026-10-17/18 in one Keel session: about 60 calls over three rounds (summarising 102 commits, classifying 46 issues, replaying 12 blind walkthroughs, judging 12 results). This reflects one account's quotas at that time, not vendor guarantees. Remeasure whenever a plan, CLI version or default model changes.

Implementation update on 2026-10-18 (offline, no new model measurements): native `batch` now handles per-pool concurrency, explicit fallback, backoff and exact-result resume. `stats` summarizes local history; `delegate implement codex-cli` provides the controlled code-writing route. The measurements below retain their original dates and workload.

## 怎么选模型、怎么给分（每次派活前）

以下历史偏好需要结合任务类型和本机成绩，不能当永久排名。先看 `scorecard --task-type X --json`，再 `recommend --task-type X --jobs N --cwd WORKTREE --only AUTHORIZED_ROUTES…`；始终收窄到用户授权的通路和材料。每次 run/review/delegate 带 `--task-type X`，batch 逐项写 task_type。recommend 不联网，只检查本地依赖、配置和冷却；就绪不代表登录或额度健康。

2026-10-18 宿主甄别反馈（用户提供，未在本轮重新调用模型）：Kimi 判真5条，核实5条属实；Grok 判真2条，核实0条属实。金丝雀0/2来自宿主提示判据漏了“冗余”类别，不能直接归罪模型。这里是发现条数，不能冒充5份独立核验报告；本轮没有将这些文字重复导入本地反馈。它提示了历史“Grok 主力”的结论需要按任务重新验，尚不足以宣布模型普遍优劣。

先从宿主确认的事实制作已知答案，用 `run --canary-file canaries.json` 或 batch 每项 canaries。格式 `[ {"match":"src/a.rs:42","expect":"真问题"} ]`；match 也可为正则（有歧义时加 `regex:` 前缀），位置按路径后缀/行号±3，要求同表格行或同发现段落有判定；明确判定标题可承接后续证据列表。答案留在本地，报告生成后自动写 source=canary。金丝雀只能测召回，测不了精度；不要用模型自己的结论当真值。召回异常先检查提示判据和输出格式，再评模型。

宿主核验后录 `feedback --accepted N --false-positive N --screen-min MIN --evidence TEXT`，未知项留空。批量可 `feedback --batch results --from verdicts.tsv`；TSV 首5列 id/accepted/false_positive/uncertain/evidence，以Tab分隔，可加screen_min/rework_min。更正追加但不重复相加，缓存反馈归回原调用。source=agent 是默认；human 必须有用户明确决定。

发现类 scorecard 按通路×任务类型显示精度、召回、失败率、秒数中位/P90和每采纳发现成本。调用不足5标样本不足；主力比较还需5份成对精度反馈及5份含金丝雀报告。模型/CLI指纹变化后，recommend 只使用新身份样本；以精度×召回选主力，约20%给未测充分通路，无主力则均匀探索，小批次按历史补欠额。冷却、缺CLI或无API凭据的路线排除。batch route:auto 按类型、cwd和text_ready分组，仍可用--only/--exclude限范围。

是否外包由宿主判断：`宿主核验成本＋漏报率×返工成本 < 宿主自己做的成本`。工具给外部秒数/采纳、宿主核验分钟/采纳和漏报率及有效样本数；返工成本、自己做的成本需要宿主估计，不自动宣称省token。

## 2026-10-24 本机累计记录复盘（stats/scorecard，约 800 次运行，未新调用模型）

- **agy-cli**：未标任务类型的 64 次里精度 0.174，宿主核对样本仍少（scorecard 仍判样本不足）。复核/找问题不当主力，只在别的通路不可用时用，并带金丝雀。
- **kimi-k3-cli**：69 次调用里 quota 17 次、因冷却跳过 162 次。只放第二意见席位，最多 2 并发；批量派活加 `--preflight`。
- **grok-cli**：triage 117 次失败率 0.026，仍是批量主力；复核 15 次精度 0.72、金丝雀 11/13。
- **codex-cli**：实施 148 次（gpt-6.1-sol，含按会话日志回填的旧记录）全部合入，约一半需宿主返工；复核时专查撞号、判据写两份、排序无兜底。

## Which route for which job (historical observations)

| Job | First choice | Second choice | Avoid |
|---|---|---|---|
| Read the repository and find problems (needs `--cwd`) | `grok-cli` | `kimi-k3-cli` | API routes, which cannot see `--cwd` |
| Fill a fixed table from a ≤100 KB packet | `grok-cli`, `kimi-k3-cli` | `text glm` | `run glm-api` on large input |
| Judge or merge short outputs (≤40 KB in, one table out) | `text glm` | `grok-cli` | — |
| Second opinion on the same packet | the family that did not write the first answer | — | the same family twice |

## Per route

**grok-cli** is the workhorse: 15 parallel jobs and no quota hits.
- Timing: a 100 KB table batch takes 7–9 min. A repository walkthrough takes 11–21 min with 4 running at once.
- Output: 3–16 KB.
- Limits: four at once was fine. Seven hours of continuous use did not hit a limit.
- It writes progress narration before the numbered list, so parse from the first `1.`.

**kimi-k3-cli** is slower, and its 5-hour window runs out.
- Timing: 2–14 min per job.
- Limits: after about 10 large jobs in one window it returned `403 You've reached your 5-hour usage limit`. Starting 4 at once then made all 14 remaining jobs fail within seconds.
- Run at most 2 at once, and budget about 8 large jobs per window. Use it for the second-opinion seat, not the bulk.
- In the 10-17/18 build, the status file's `error` could hold unrelated git status lines. The 10-18 implementation selects the matching diagnostic line; old records still need the `.log` tail.

**kimi-k3-api** returned `429 insufficient balance`. Treat it as unavailable until it is topped up.

**glm-api / `text glm`** does not read the repository and fails on long output.
- `run glm-api` with about 130 KB input ended with `finish_reason=length`. `text glm --effort low --max-tokens 32000 --timeout-ms 1800000` succeeded on 100 KB.
- Ask for one short table. Long free-form answers get cut.
- With 6 at once, 2 of 12 got `429 code 1302` (rate limit). Run at most 4 at once, and retry failures one at a time.
- The client-data guard refuses text that looks like contract amounts. Replacing the numbers did not help. Pass `LLM_RUN_ALLOW_CLIENT_DATA=1` per call, only when the user has authorised that material.
- Text routes need `--text-ready`, otherwise they fail with `missing_input`.

**codex** (the MCP server) was disconnected that day. Untested via `codex-cli`.

## Sizing rules

- Split input into packets of ≤100 KB. Cap each item in a packet at about 3 KB of diff and 3 KB of task text, so one huge item cannot crowd out the others.
- Ask for a fixed table with a fixed first column, such as a commit hash. Then merge and count with a script, not with the host reading every answer.
- Size the output, not only the input. For GLM, ≤8 KB of output is reliable.
- For slow routes, give fewer and larger jobs instead of many parallel ones.

## Saving host tokens

- The host writes prompts and scripts, and reads only final tables and 2–3 spot checks. Raw packets and per-batch answers stay on disk.
- Have one model produce and a different one judge. The host then checks a couple of verdicts against the source: in the replay, 2 of 12 GLM verdicts needed correcting.
- Use `multi-ai batch --jobs jobs.json --outdir results --resume`, with one prompt per job and explicit per-pool limits. Complete results are reused only when identity and body hash still match; unknown model identities are rerun. The historical batch used a shell wrapper and per-route `xargs -P` limits.
- On a quota error, configure another pool in the job's `fallback` list (or `run --fallback ROUTE`). The 10-18 scheduler switches immediately and skips a cooled pool. Rate-limit errors such as 1302 retry with backoff. The historical run required the wrapper to perform this switch.
- The historical zsh `xargs` wrapper could fail with "command line cannot be assembled" when embedding long strings. Native `batch` reads prompt files and avoids that shell assembly path.


## 实施结果

implement 默认类型不必重复传参。派活后先检查 attribution、boundary_warnings、summary_kind；快照警告只表示观测到外部变化，不能归因给 Codex。完整归因越界诊断报告仍可 feedback，执行失败/partial 不算完整报告。

用 --outcome merged|merged-after-rework|discarded、--rework-items N、--rework-rounds N、--defect-tags tag1,tag2 和 --evidence 录宿主结论，未知数留空，沿用 --screen-min/--rework-min（分钟）。建议标签 criterion-duplicated/id-collision/comment-format-churn/pagination-clears-data/sort-tiebreak/performance/browser-only。TSV 增加同名下划线列；更正按字段最后已知值，标签整体替换。

实施成绩单按历史 model 区分身份，以已知 outcome 算合入率、返工后合入率、弃用率，按已知数计算平均返工条数/轮数，给缺陷直方图及中位/P90 秒，不计算 precision。警告/归因单列，不计执行失败；越界同时执行失败仍按内层原因计失败。主力比较需至少5次调用、5份 outcome 和5份返工条数，合入率优先、低返工次序。换模型、身份未知或样本不足继续探索。旧快照误报可离线重判后补录；只认完整内层 response exit 0 和与 last-message 对应的完整摘要，不猜历史 model。
