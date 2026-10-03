---
name: multi-llm-pipeline
description: Delegate text-in/text-out work to DeepSeek, Kimi, or GLM via llm-run. Use when the user names those models, asks for a cheap second opinion, roundtable, or dispatch by domain. Do not use for host-native web search, repo exploration, or work the current model should do itself.
---

# Multi-LLM Pipeline

`multi-ai text` is the Rust native **text-in/text-out** channel (DeepSeek / Kimi / GLM). `llm-run` and the existing `bin/run.sh` are compatibility entries to the same binary; no Node/Python is needed for API requests. Three hosts: Claude Code, Codex, Grok TUI. It is not a general agent. Repo exploration, live web, and writing code stay on **whatever host is driving**.

Canonical tree: `~/.claude/skills/multi-llm-pipeline/` (this file + `bin/run.sh`). The Codex `bin/run.sh` forwards here; its documentation is navigation, not a second maintained implementation. Legacy scripts remain compatibility-only.

Every call prints the resolved route on stderr:

```
[llm-run] dispatch domain=extract class=text-fast adapter=llm-run
[llm-run] task=extract -> provider=deepseek model=deepseek-v4-flash thinking=off ...
```

Without `--out`, stdout is the completed answer (or JSON with `--json`). With `--out`, that selected format is saved to the file and stdout prints `Wrote <path>`, preserving the legacy interface. Tokens are saved incrementally to progress files rather than printed on stdout. `--quiet` / `LLM_RUN_QUIET=1` silences the banner. Aliases also honor `--out`, `--json`, `--effort`, and transport options.

Live tables beat this file: `llm-run profiles` · `llm-run domains` · `llm-run aliases`.

First use: `llm-run help` gives the offline quick start, current options, progress paths and failure rules. `llm-run glm --help` and other text-command help flags also exit before model dispatch, without credentials. For cross-vendor scheduling, start with `ai-review help`; `ai-review help run|dispatch|verify` shows current subcommand parameters. Full workflow and material policy stay in these canonical skills.

## Need failover across vendors? Use `ai-review`

The text command retries eligible failures **inside one provider**, within the total time budget and only before receiving answer content. For cross-vendor failover use `multi-ai review` or the compatible `ai-review dispatch`; both use the same Rust execution core. It schedules by **credit pool** across Grok / Codex / Kimi CLI /
DeepSeek / GLM, cools a dead pool down and moves to the next one. Report count, configured model families, and quota pools are separate measures; multiple pools do not establish cognitive independence. Note `claude-ds` and this file's
`deepseek` share one pool. In `ai-review`, `kimi` and `kimi-k3` both use the Kimi Code CLI pool; `kimi-k3-api` uses the separate Moonshot API pool. This runner's `llm-run kimi-k3` is that API route, not the CLI.

## When to use

- User requests DeepSeek / Kimi / GLM processing or an external second opinion; mentioning a model or saving tokens alone does not require a call
- Long doc to compress or extract (`--domain extract`)
- Evidence-based review / red-team when it can add a distinct, verifiable perspective
- An **explore evidence packet** already exists and needs judgment (`--task map` / `review` / `roundtable`)

Do **not** use when:

- The host can already search the web — do that yourself (see host routing)
- The task is repo exploration — use host tools directly, or available read-only subagents for large separable searches
- You would be calling the host's own model over MCP (Grok→Grok, Claude→Claude)
- A single in-session edit the current model can finish

## Who drives (Claude / Codex / Grok)

Same `bin/run.sh`. The host does exploration and synthesis; `llm-run` only judges text it is fed.

| Need | Claude Code | Codex | Grok TUI | Never |
|---|---|---|---|---|
| Live facts | Host web; if dead, Codex MCP | You search (you have web) | Host `web_search` / `open_page` | DeepSeek / Kimi |
| Repo explore | Host tools; available read-only subagents for large searches | Host tools; available read-only subagents for large searches. Do not nest `codex exec` | Host tools; available read-only explore subagent | `llm-run`; Grok MCP from Grok |
| Judge a packet | **This session** runs `llm-run` via Bash | **This session** runs `llm-run` via shell | **This session** runs `llm-run` via Bash | Spawn a sibling agent just to wrap the shell |
| Write code / DOM | This session | This session | This session | Flash as sole implementer |
| Customer data | Local by default; explicit material authorization below | Same | Same | Unspecified customer originals; credentials |

Use host tools for an evidence packet. Any project-specific slash command or Grok workflow is optional and must be installed separately; the multi-ai package does not ship those commands.

DeepSeek/Kimi/GLM are not host subagents. Do not register them as Grok `[model.*]`. From Codex, do not auto-pick Grok; only if the user names Grok/xAI.

## Dispatch (preferred entry)

```bash
RUN=~/.claude/skills/multi-llm-pipeline/bin/run.sh   # or llm-run on PATH

$RUN dispatch --domain extract --input doc.md
$RUN dispatch --domain product --input plan.md
$RUN dispatch --domain product --kind debate --input plan.md --out /tmp/r1.md
$RUN dispatch --domain snippet --input patch.diff
$RUN dispatch --domain explore          # prints ADAPTER=host-explore; does not call an API
$RUN run --task vision-review --image shot.png --question "挑布局问题"
$RUN domains
```

`--domain` picks the capability class and default model. `--kind compress|review|debate|map|verify` overlays the task. Classes `agentic-explore` / `web` / `local-private` / `host` print an adapter instruction and exit — follow it, do not retry with Flash. A printed `After the evidence packet` command is an optional follow-up suggestion, not an automatic call.

## Explore packet (the only way to "explore" with cheap models)

1. Host tools answer small searches directly. For large separable searches, available read-only subagents return a packet (paths + excerpts + open questions + coverage limits), not the whole repo.
2. Stop when the evidence answers the task. Only call `map` for needed extraction or `review` for a useful second opinion; no automatic post-exploration call or default roundtable.
3. Host verifies citations against source excerpts and relevant context; unresolved claims remain 未核实. Review output includes findings, sources, impact and coverage limits; no findings is valid.

Do not reread or resummarize material already understood just to delegate it. Keep one writer per file; high-risk review starts from original requirements, diff and acceptance scenarios, not only the implementer's summary. Different vendors are optional; if independent review is unavailable, disclose that limitation. Unconfirmed business semantics go to the maintainer, not a stronger model.

No extra slash command is required for this protocol. Attach the resulting packet explicitly when requesting an external review.

## Tasks (text channel)

| Task | Default | Notes |
|---|---|---|
| `summarize` / `extract` | DeepSeek flash, thinking off | Faithful; no extra facts |
| `map` | flash | Packet → modules / call chains. No invented paths |
| `review` / `explore-review` | DeepSeek pro, thinking on | Finding = severity + quote + suggestion |
| `code-review` | GLM-5.3 | Snippet domain → `kimi-k2.7-code` |
| `think` | DeepSeek pro max | |
| `think-k3` / `red-team` | Kimi K3 | Perspective, not savings. Minutes |
| `third-opinion` | GLM-5.3 (always thinks) | Independent seat |
| `verify` | flash | First line CONFIRMED\|REFUTED\|UNKNOWN. Not for live web claims |
| `vision-review` | GLM-5.3-Flash | Requires `--image` (png/jpg/webp). GLM-5.3-Flash and K3 only |
| `roundtable` | parallel seats | First opinions only. Host coordinates cross-examination separately |

Roundtable is **parallel** and **seat-split** (`--set default|product|code`). It is optional, not the default for important work or a vote-based acceptance gate. It is not a debate.

## Progress, completeness, and timeouts

GLM streams by default; `--stream` explicitly enables it and `--no-stream` disables it. Streaming helps receive data early but does not guarantee that queueing or network failure disappears. It does not fix an exhausted output budget: split large material by account/report/inventory and leave enough `--max-tokens` for thinking plus the final answer. GLM-5.3 cannot disable thinking.

Every request has its own private run directory: `--out FILE` defaults to `FILE.progress/run-*`; otherwise `~/.local/state/multi-ai/progress/run-*`; each API attempt is a private child directory. `--progress-dir DIR` overrides the base. `answer.partial.txt`, `response.raw`, `events.jsonl`, and `metadata.json` persist during execution and on failure. The metadata records headers wait, first data/content, last data, total duration, usage, and completion status. Treat the raw response as sensitive material. `LLM_RUN_PROGRESS=1` also emits bounded progress notices on stderr.

Only nonempty content with a valid terminal marker succeeds. `finish_reason=length`, unexpected finish reasons, malformed/truncated streams, or thinking-only output exit nonzero and retain partial files; they never replace an existing complete `--out`. An incomplete roundtable writes `FILE.partial` and fails the round.

`--timeout-ms` covers the entire request including retries (profile defaults: 900s for K3, 420s for thinking-on). `--headers-timeout-ms` bounds waiting for HTTP headers (default up to 300s); `--idle-timeout-ms` bounds gaps between response bytes (default 120s). Errors distinguish headers/idle/total timeout. **Your host tool timeout can still kill the process first**; use a yielded/background shell and a caller budget that covers the request. A streaming API does not make a collecting outer tool return tokens.

| Call | Caller timeout |
|---|---|
| flash, k3-low, thinking off | default |
| review, third-opinion, k3-high, glm-5.3 | `timeout: 300000` |
| think, think-k3, roundtable | `timeout: 600000` + `--out FILE`, or background |

A killed Moonshot API call may already have incurred token charges. Kimi Code CLI uses its separate subscription quota.

## Safety

- Do not proactively collect or send customer originals. When the maintainer explicitly specifies material for an external model, that authorizes only that material and purpose, without repeated confirmation or a separate desensitization audit. “Look at this file” alone is not external-processing authorization. Do not add unspecified customer records; keep credentials out of requests and customer originals out of project Git.
- Existing client-data heuristics remain unchanged. If they block explicitly authorized material, use `LLM_RUN_ALLOW_CLIENT_DATA=1` only as a per-command prefix for that call; never persist it in environment/configuration. The switch is not blanket authorization.
- `private` still selects local processing. For explicitly authorized external work, select the appropriate external route instead of reinterpreting `private`.
- DEAD aliases (`kimi-k26`, `moonshot-32k`, `glm-5.1`, …) error unless `--legacy`.
- `moonshot-v1` / `kimi-k2.5` sunset 2026-08-31; compression replacement is flash.

## Configuration and report feedback

Source and installation guide: [multi-ai repository](https://github.com/xiaodi007/multi-ai), with compatibility details in its `docs/COMPATIBILITY.md`. A private repository requires access. `multi-ai routes --json` and `multi-ai doctor` are offline. `doctor --live`, `probe`, and `list-models` may use network and quota.

The shared `~/.config/multi-ai/profiles.json` is the current profile source; `routes.json` stores routes and pools. Defaults are embedded. Nonempty process variables take precedence, then missing values are filled from `$PWD/.env`, `~/.config/multi-ai/.env`, and the old `~/.config/multi-llm-pipeline/.env`. Existing credentials are read in place; installation does not copy them.

All managed `--out` calls lock the report, status, cache and derived paths before execution. Same paths or overlapping output directories fail immediately. Sequential reuse archives the previous report and metadata; failure retains the last complete report. `--resume` needs matching complete content, configuration, implementation and source identity; non-Git context needs an immutable `--source-id`. Old Node/Python caches may require one new call after migration.

After actually validating a report, the host may record report-level feedback using the run and attempt IDs in status/manifest:

```sh
multi-ai feedback --run RUN-UUID --report ATTEMPT-ID --accepted 2 \
  --false-positive 1 --evidence 'Verified against source and tests: …'
```

Leave unknown counts and timing omitted. Source defaults to `agent`; only an explicit user decision allows `--source human --human-confirmed`, with that decision recorded in evidence. Tests passing or a model's `CONFIRMED` is not human acceptance. Feedback is append-only, tied to report hash, and does not change ranking. Corrections are new records.

For current protocol and compatibility behavior, consult `docs/COMPATIBILITY.md` in the source repository and the installed command help. Any pre-existing local `GOTCHAS.md` or `PATTERNS.md` remains user-maintained historical material, not part of the multi-ai release.
