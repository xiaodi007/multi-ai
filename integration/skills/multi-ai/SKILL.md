---
name: multi-ai
description: Delegate authorized repository investigation, material extraction, cross-model review, or Codex implementation through multi-ai. Use for requested external AI help or multiple perspectives; small local edits stay on the host.
---

# multi-ai

Use the Rust program through this skill's `scripts/run.sh`, which forwards arguments unchanged. Resolve that script relative to the location of this `SKILL.md`; it also works when the agent's PATH omits `~/.local/bin`. The API client needs no Python, Node, uv, or Cargo. CLI routes require their own installed, authenticated vendor tool.

Before choosing a route, run `scripts/run.sh routes --json` and `scripts/run.sh doctor`. These are offline: configured credentials and an executable do not establish login, quota, or service health. Use the user's route preference. `doctor --live`, `probe`, and `text list-models` make network requests; `review --dry-run` can collect live quota too. Before dispatching after a vendor rename or CLI upgrade, `doctor --models` (lists models, no model call) and `doctor --updates` catch retired ids and unvalidated delegation versions. A pool shown as 欠费停用 in `status` needs a recharge and `reset`, not waiting. End one of your own running jobs with `stop ID` (`stop --list` shows them); never pkill vendor CLIs. `delegate implement` also takes any `claude-*-ro-cli` route (restricted Claude Code sandbox, .git write-denied, no --network).

For authorized delegation, read [references/delegation.md](references/delegation.md). Before dispatching a batch, read [references/route-traits.md](references/route-traits.md) for measured speed, quota and size limits per route. Let the worker read large materials before you do: define a small goal and explicit files/directories, then read its short handoff and verify the evidence that affects your decision. For snapshot investigation, extraction and challenge, use at most two parallel independent tasks. For native batch, set each quota pool's concurrency explicitly. Keep simple searches, edits and tests on the host; do not start a roundtable for every step. This workflow uses synchronous CLI calls, not a native host sub-agent panel.

## 怎么选模型、怎么给分

每次派活前读 [route-traits](references/route-traits.md)，运行 `scripts/run.sh scorecard --task-type X --json`，再用 `scripts/run.sh recommend --task-type X --jobs N --cwd WORKTREE --only AUTHORIZED_ROUTES…` 看分配建议。先服从用户已授权的材料与通路范围，再选模型。`run/review/batch/delegate --task-type X` 使用自由字符串（建议 triage/review/investigate/extract/implement/walkthrough），batch 可逐项写 task_type；类型应描述实际任务。

宿主事先确认的事实才可用作金丝雀：run 的 `--canary-file` 读 JSON 数组，batch 每项写 `canaries: [{"match":"src/a.rs:42","expect":"真问题"}]`；其他 match 为正则，有歧义可加 `regex:` 前缀。答案不传给模型。要求报告按 `位置 | 判定 | 证据` 输出；核对路径后缀、行号±3，同表格行/发现段落内判定（明确判定标题可承接后续证据列表），否定/待定不算命中。自动 source=canary 只测召回，测不了精度。不要把模型自己的结论做真值；召回低时先复查提示判据是否漏了缺陷类别。状态的 canary_results 可供抽查。

每次模型交付后尽量独立核验、录分，未知数量和时间留空。用 `feedback --run RUN --report ATTEMPT --accepted N --false-positive N --screen-min MIN --evidence '实际核验证据'`；批量用 `feedback --batch OUTDIR --from verdicts.tsv`，TSV 列 `id accepted false_positive uncertain evidence`（Tab分隔，可再加 screen_min/rework_min）。batch 结束在 stderr 打印含 run_id/attempt_id/canary 的表，stdout JSON 保持兼容。更正按字段取最后已知值，缓存不重复计样本；默认 source=agent，human 仍需要用户明确决定。

发现类成绩单按通路×任务类型给精度、金丝雀召回/漏报、中位/P90秒、quota/rate_limit/timeout失败率、每条采纳的外部秒数与宿主核验分钟，并列有效样本覆盖。少于5次调用不下结论；主力还要求5份成对精度核验、5份金丝雀报告。recommend 以精度×召回选主力，约20%探索证据不足的通路；冷却或本地缺依赖/凭据的通路排除，CLI/模型身份变化后重新积累。小批次按历史补探索欠额。普通 run/review 不变，batch 只有显式 `route:"auto"` 才用建议；对 auto 加 `--only/--exclude` 限制到本次授权路线。API 仍需 text_ready 材料声明。

外包划算的判据：`宿主核验成本＋漏报率×返工成本 < 宿主自己做的成本`。工具只给核验分钟、漏报率和外部耗时的实测，返工成本和自己做的成本由宿主估计；不把速度、模型一致或运行成功当正确性/token节省。

Choose the smallest useful operation:

| Request | Command |
|---|---|
| One explicit route | `run ROUTE --prompt-file evidence.md --cwd /path/to/work --out answer.md` |
| Batch of independent jobs | `batch --jobs jobs.json --outdir results --resume` |
| Local history statistics | `stats` or `stats --json` |
| Explicitly authorized code implementation | `delegate implement codex-cli --cwd /path/to/clean-worktree --prompt-file brief.md --out /outside/worktree/summary.md` |
| Parallel review | `review --input evidence.md --text-ready --seats 2 --outdir reviews` |
| Verify a report | `verify --findings reviews/report.md --cwd /path/to/work --out verified.md` |
| Text or images through API profiles | `text glm --input evidence.md --out answer.md` or `text run --task vision-review --image image.png --out vision.md` |
| Inspect aliases, tasks, domains | `text aliases`, `text profiles`, `text domains` |

Use explicit route names such as `glm-api`, `kimi-k3-cli`, and `codex-cli`; `routes` supplies the full configured list. `--only ROUTE…` selects review candidates. The review scheduler deduplicates quota pools and can add backup routes after failure; inspect the resulting manifest to see what actually ran. Respect exclusions, cost preferences, and the requested scope. A single text request does not need a roundtable. API routes only see the supplied text/images; they cannot inspect the path passed in `--cwd`. CLI routes can use vendor tools in their working directory, so choose an appropriately scoped directory.

For existing `run/review/verify` workflows inside Codex, preserve the routing preference: do not automatically select Grok/xAI unless requested; add `--exclude grok-cli` to scheduler calls when it was not requested. Do not fill an extra seat by nesting Codex CLI from a Codex host; exclude `codex-cli`. The new delegation workflow permits Kimi and Grok when both were authorized for this task, with Kimi → Grok investigation fallback. It does not broaden older review permissions or override a narrower project/session preference.

Send only material authorized for this request. Explicitly requested external processing authorizes those specified materials; a general request to inspect a file does not authorize sending unrelated customer originals or credentials. Prepare a small relevant packet rather than uploading a repository by default. Never print or copy keys into prompts, reports, or installation material.

`run/review/verify --timeout`, `--headers-timeout`, and `--idle-timeout` use **seconds**. `text` and legacy `llm-run --timeout-ms`, `--headers-timeout-ms`, and `--idle-timeout-ms` use **milliseconds**. The text interface supports repeated `--input` and `--input -` for multiple files and stdin; `run/review/verify --input` accepts file paths, so do not pass `-` there. With `text --out`, the selected text/JSON format is written into that file; stdout prints `Wrote <path>`, so parse the output file when requesting JSON.

Choose an output owned by this task; overlapping outputs fail before sending. Sequential replacement keeps history, and a failed attempt preserves the last complete report. `--resume` only reuses complete results whose identity and body hash match; a changed input, model, implementation, or corrupted cache causes another request. For a non-Git workspace, provide a meaningful `--source-id` and change it when external context changes. Unknown CLI default model identities cannot be safely cached. Use run/attempt IDs in the manifest to distinguish new calls and reused reports.

## 实施

实施委派使用本技能 runner 和 multi-ai，须在用户已授权的通路与材料范围内；不要另写厂商执行命令。评审席位规则继续适用。固定使用 `codex exec --json` 与原生 workspace-write，不提供 danger-full-access，不套外层 macOS sandbox；默认不覆盖网络配置。

使用 `scripts/run.sh delegate implement codex-cli --cwd WORKTREE --prompt-file BRIEF --out SUMMARY`；默认 task-type=implement。cwd 必须为 Git 工作树顶层，默认拒绝 dirty，已有修改显式 --allow-dirty；out 放在 cwd 外。--path-prepend DIR 仅影响子进程 PATH，--timeout 秒；--network 允许任意网络地址，远端写入不能事后核对。不自动提交，不支持实施 resume。长任务或换过模型 id 后加 --preflight：先几秒探针，通路坏了直接以 model/auth/quota 等状态结束、不碰工作树。

JSONL 文件编辑事件用于归因；cwd 外编辑或沙箱可写到的 Claude skills/settings/CLAUDE.md（含 .agents）变化为 boundary_violation，不回滚；沙箱写不到的（宿主派活期间自己改技能/设置）只进 boundary_warnings.claude_config_outside_sandbox。注册工作树的 ignored 产物、Git index/logs/refs、同伴修改等未归因差异只进入 boundary_warnings，完整成功仍 status=ok/complete=true。attribution=unavailable 表示事件流无法完整解析；不能据快照断言是谁改的。--watch PATH 可追加范围。status 每桶最多 20 样例，完整列表在 boundary_details；summary_kind=final/diagnostic 区分摘要。完整诊断摘要也可按 run/attempt 反馈，实际执行失败则不登记空或 partial 报告。

宿主复核后录：

```sh
scripts/run.sh feedback --run RUN --report ATTEMPT \
  --outcome merged-after-rework --rework-items 2 --rework-rounds 1 \
  --defect-tags criterion-duplicated,id-collision --screen-min 8 --rework-min 15 \
  --evidence '实际复核与门禁证据'
```

outcome 是 merged/merged-after-rework/discarded；返工条数和另派轮数未知留空。建议缺陷标签：criterion-duplicated、id-collision、comment-format-churn、pagination-clears-data、sort-tiebreak、performance、browser-only，自由字符串可扩展。TSV 首5列保持 id/accepted/false_positive/uncertain/evidence，实施时发现数量可留空，追加 outcome/rework_items/rework_rounds/defect_tags/screen_min/rework_min。更正逐字段追加，标签整体替换，默认 source=agent。

实施成绩看合入率、返工后合入率、弃用率、平均返工条数和轮数、缺陷分布、中位/P90 秒；不给 precision。警告与归因越界单列，失败率只计执行失败。recommend 按合入率优先、较少返工次序，至少5份 outcome 和5份返工条数才比较；未知模型不获主力结论。旧误报补录前，可由宿主在停止写入后用发布包 reclassify-implement.py --state-dir STATE --status SUMMARY.md.status.json 预演，加 --apply 重判并登记；完整性判据、备份与中断恢复见仓库 README，不能把失败或损坏摘要改成成功。

For a batch, use an explicit `route` and `fallback` list per job, or authorized `route:"auto"` with task_type in the JSON manifest. Limit concurrency by quota pool with `--pool-limit POOL=N`. For long or unattended batches add `--preflight`: one tiny call per route first, and routes failing with model/auth/quota/cooldown/capacity/error are skipped for the whole batch (jobs start at their fallback). Quota errors switch pool; rate limits retry with backoff. With two or more review reports, run `consensus REPORT... --out table.md` (offline) and check the 结论相反 rows first; disagreement is where judgment criteria or real defects hide. After host review, fill the generated `OUTDIR/verdicts.template.tsv` (blank rows are skipped) and import it with `feedback --batch OUTDIR --from FILE`; this feedback is what makes scorecard comparable. Read each `.md.status.json` and the batch `dispatch.json`; a prior complete report can remain after this run fails. Use `stats --json` to refresh local observations without making a model request; unknown historical fields remain null. Existing measurement guidance in route-traits is dated evidence, not a new benchmark.

Read and independently verify returned findings against the material, requirements, or actual tests. Model agreement and `CONFIRMED` are evidence to check, not acceptance. After actual verification, record known report-level counts:

```sh
scripts/run.sh feedback --run RUN-UUID --report ATTEMPT-ID \
  --accepted 1 --evidence 'Verified against the supplied requirement and test …'
```

Leave unknown counts and time savings unset. Feedback binds the report hash and appends events; corrections retain the earlier record. Default source is `agent`. Use `--source human --human-confirmed` only for an explicit user decision, with that decision in the evidence. Passing tests do not imply human acceptance. Feedback informs scorecard and recommend; ordinary run/review ranking stays unchanged.
