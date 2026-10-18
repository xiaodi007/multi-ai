# Route traits from real batches

Measured on 2026-10-17/18 in one Keel session: about 60 calls over three rounds (summarising 102 commits, classifying 46 issues, replaying 12 blind walkthroughs, judging 12 results). This reflects one account's quotas at that time, not vendor guarantees. Remeasure whenever a plan, CLI version or default model changes.

Implementation update on 2026-10-18 (offline, no new model measurements): native `batch` now handles per-pool concurrency, explicit fallback, backoff and exact-result resume. `stats` summarizes local history; `delegate implement codex-cli` provides the controlled code-writing route. The measurements below retain their original dates and workload.

## Which route for which job

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
