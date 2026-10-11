---
name: multi-ai
description: Use the installed multi-ai CLI for requested multi-model reviews, report verification, or DeepSeek/Kimi/GLM text and image processing. Use when the user asks for multiple AI opinions or names multi-ai; ordinary local edits stay on the host.
---

# multi-ai

Use the Rust program through this skill's `scripts/run.sh`, which forwards arguments unchanged. Resolve that script relative to the location of this `SKILL.md`; it also works when the agent's PATH omits `~/.local/bin`. The API client needs no Python, Node, uv, or Cargo. CLI routes require their own installed, authenticated vendor tool.

Before choosing a route, run `scripts/run.sh routes --json` and `scripts/run.sh doctor`. These are offline: configured credentials and an executable do not establish login, quota, or service health. Use the user's route preference when given. Read local `--help` for unfamiliar parameters; `doctor --live`, `probe`, and `text list-models` make network requests. `review --dry-run` can collect live quota too.

Choose the smallest useful operation:

| Request | Command |
|---|---|
| One explicit route | `run ROUTE --prompt-file evidence.md --cwd /path/to/work --out answer.md` |
| Parallel review | `review --input evidence.md --text-ready --seats 2 --outdir reviews` |
| Verify a report | `verify --findings reviews/report.md --cwd /path/to/work --out verified.md` |
| Text or images through API profiles | `text glm --input evidence.md --out answer.md` or `text run --task vision-review --image image.png --out vision.md` |
| Inspect aliases, tasks, domains | `text aliases`, `text profiles`, `text domains` |

Use explicit route names such as `glm-api`, `kimi-k3-cli`, and `codex-cli`; `routes` supplies the full configured list. `--only ROUTE…` selects review candidates. The review scheduler deduplicates quota pools and can add backup routes after failure; inspect the resulting manifest to see what actually ran. Respect exclusions, cost preferences, and the requested scope. A single text request does not need a roundtable. API routes only see the supplied text/images; they cannot inspect the path passed in `--cwd`. CLI routes can use vendor tools in their working directory, so choose an appropriately scoped directory.

When this skill runs inside Codex, preserve the existing routing preference: do not automatically select Grok/xAI unless the user explicitly requested it; add `--exclude grok-cli` for a scheduler call when it was not requested. Do not automatically fill an extra seat by nesting Codex CLI from a Codex host; exclude `codex-cli` in that case. An explicit request to exercise those routes authorizes the selected routes. Preserve any more specific host or project routing preferences already in the session.

Send only material authorized for this request. Explicitly requested external processing authorizes those specified materials; a general request to inspect a file does not authorize sending unrelated customer originals or credentials. Prepare a small relevant packet rather than uploading a repository by default. Never print or copy keys into prompts, reports, or installation material.

`run/review/verify --timeout`, `--headers-timeout`, and `--idle-timeout` use **seconds**. `text` and legacy `llm-run --timeout-ms`, `--headers-timeout-ms`, and `--idle-timeout-ms` use **milliseconds**. The text interface supports repeated `--input` and `--input -` for multiple files and stdin; `run/review/verify --input` accepts file paths, so do not pass `-` there. With `text --out`, the selected text/JSON format is written into that file; stdout prints `Wrote <path>`, so parse the output file when requesting JSON.

Choose an output owned by this task; overlapping outputs fail before sending. Sequential replacement keeps history, and a failed attempt preserves the last complete report. `--resume` only reuses complete results whose identity and body hash match; a changed input, model, implementation, or corrupted cache causes another request. For a non-Git workspace, provide a meaningful `--source-id` and change it when external context changes. Unknown CLI default model identities cannot be safely cached. Use run/attempt IDs in the manifest to distinguish new calls and reused reports.

Read and independently verify returned findings against the material, requirements, or actual tests. Model agreement and `CONFIRMED` are evidence to check, not acceptance. After actual verification, record known report-level counts:

```sh
scripts/run.sh feedback --run RUN-UUID --report ATTEMPT-ID \
  --accepted 1 --evidence 'Verified against the supplied requirement and test …'
```

Leave unknown counts and time savings unset. Feedback binds the report hash and appends events; corrections retain the earlier record. Default source is `agent`. Use `--source human --human-confirmed` only for an explicit user decision, with that decision in the evidence. Passing tests do not imply human acceptance. Feedback does not change route ranking.
