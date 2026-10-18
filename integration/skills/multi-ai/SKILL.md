---
name: multi-ai
description: Delegate authorized repository investigation, material extraction, cross-model review, or Codex implementation through multi-ai. Use for requested external AI help or multiple perspectives; small local edits stay on the host.
---

# multi-ai

Use the Rust program through this skill's `scripts/run.sh`, which forwards arguments unchanged. Resolve that script relative to the location of this `SKILL.md`; it also works when the agent's PATH omits `~/.local/bin`. The API client needs no Python, Node, uv, or Cargo. CLI routes require their own installed, authenticated vendor tool.

Before choosing a route, run `scripts/run.sh routes --json` and `scripts/run.sh doctor`. These are offline: configured credentials and an executable do not establish login, quota, or service health. Use the user's route preference. `doctor --live`, `probe`, and `text list-models` make network requests; `review --dry-run` can collect live quota too.

For authorized delegation, read [references/delegation.md](references/delegation.md). Before dispatching a batch, read [references/route-traits.md](references/route-traits.md) for measured speed, quota and size limits per route. Let the worker read large materials before you do: define a small goal and explicit files/directories, then read its short handoff and verify the evidence that affects your decision. For snapshot investigation, extraction and challenge, use at most two parallel independent tasks. For native batch, set each quota pool's concurrency explicitly. Keep simple searches, edits and tests on the host; do not start a roundtable for every step. This workflow uses synchronous CLI calls, not a native host sub-agent panel.

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

When the user authorizes Codex to implement code, use this skill's runner and the controlled entry:

```sh
scripts/run.sh delegate implement codex-cli \
  --cwd /absolute/path/to/clean-worktree \
  --prompt-file /absolute/path/to/brief.md \
  --out /absolute/path/outside-worktree/summary.md \
  --timeout 1800 \
  --path-prepend ~/.nvm/versions/node/v24.19.0/bin
```

Use a Git worktree root and place the summary outside it. The default refuses any uncommitted changes; only add `--allow-dirty` when intentionally continuing on those changes. Repeated `--path-prepend DIR` affects the child PATH only. Timeout is in seconds. This entry fixes `codex exec -C <cwd> -s workspace-write --skip-git-repo-check -o <staged-summary>`, without an outer macOS sandbox or an extra network configuration override. The write boundary relies on Codex's native `workspace-write` and checks after execution.

The runner records before/after HEAD, `git status --porcelain`, Git-visible file changes with preexisting edits identified, and recursive mtime snapshots of sibling Git worktrees outside cwd, `~/.claude`, and `~/.agents` (excluding this attempt’s managed artifacts; nested symlink targets are not followed). Detected changes outside the boundary produce `boundary_violation` with paths in the status; report them without rolling back. These snapshots cannot detect remote writes over the network, writes outside monitored paths, or writes that disappear or restore mtime between snapshots. Concurrent processes can also change monitored paths, so a difference alone cannot prove Codex caused it.

This entry does not commit or reuse implementation results. After a failed or timed out implementation, inspect the actual worktree as well as the status, since edits may already exist; the previous complete summary is preserved.

Use this entry for implementation delegation. Keep review-seat selection rules above; an implementation task is a separately authorized operation. All external model work goes through `multi-ai`; old `ai-review`/`multi-llm-pipeline` skills are no longer installed or updated, and `ai-review`/`llm-run` binary aliases are deprecated. Do not write a separate vendor execution command.

For a batch, use an explicit `route` and `fallback` list per job in the JSON manifest. Limit concurrency by quota pool with `--pool-limit POOL=N`. Quota errors switch pool; rate limits retry with backoff. Read each `.md.status.json` and the batch `dispatch.json`; a prior complete report can remain after this run fails. Use `stats --json` to refresh local observations without making a model request; unknown historical fields remain null. Existing measurement guidance in route-traits is dated evidence, not a new benchmark.

Read and independently verify returned findings against the material, requirements, or actual tests. Model agreement and `CONFIRMED` are evidence to check, not acceptance. After actual verification, record known report-level counts:

```sh
scripts/run.sh feedback --run RUN-UUID --report ATTEMPT-ID \
  --accepted 1 --evidence 'Verified against the supplied requirement and test …'
```

Leave unknown counts and time savings unset. Feedback binds the report hash and appends events; corrections retain the earlier record. Default source is `agent`. Use `--source human --human-confirmed` only for an explicit user decision, with that decision in the evidence. Passing tests do not imply human acceptance. Feedback does not change route ranking.
