# multi-ai

[中文](README.zh-CN.md) · [Full reference (Chinese)](docs/REFERENCE.md) · [License: MIT](LICENSE)

**multi-ai** is a single native Rust CLI that lets a host coding agent (Claude Code, Codex, …) call *other* AI models in a controlled, auditable way. You can use it to:

- get **independent second opinions**: parallel code or design review by models from different vendors;
- **delegate** bounded work, such as repository investigation, material extraction, adversarial challenge, or sandboxed implementation, to cheaper or specialised models;
- **measure** which model is worth using for which task, from your own verified results instead of vibes.

It talks to vendor CLIs you already have logged in (Codex, Grok, Kimi Code, Antigravity, Claude Code pointed at other providers) and to plain HTTP APIs (DeepSeek, Zhipu GLM, Moonshot Kimi). Every call is recorded, every report is published atomically, and nothing is sent unless you attach it explicitly.

> Status: v0.2.x. Only **macOS on Apple Silicon** has been tested. The Rust code should build on other Unix systems, but nobody has tested it there.

## Why

When an AI agent asks another AI to check its work, a few things tend to go wrong:

| Problem | What multi-ai does |
|---|---|
| Reviewers agree with the author (sycophancy, same model family) | Excludes the implementer's model family from review; host-claim lint and blinding; decoy canaries and a measured *sycophancy rate*; an optional fixed **dissent seat** |
| Two reviews that look independent come from the same quota pool or model family | Routes carry explicit `pool` and `family`; review seats are de-duplicated by quota pool |
| A run fails halfway, a quota runs out, or a model id was retired | Quota-pool cooldowns, rate-limit back-off, explicit fallbacks, `batch --preflight`, `doctor --models` |
| Results get silently overwritten or mixed between runs | Output locks before any model call; a failed run never replaces the last complete report; `--resume` only reuses outputs whose identity and content hashes match |
| "Which model should I use?" gets answered by hunches | `feedback` → `scorecard` → `recommend`: precision, canary recall, latency, failure rates per *route × task type* |
| Delegated implementation touches things it shouldn't | `delegate implement` runs in a clean git worktree with a fixed sandbox, takes before/after snapshots, records out-of-bounds writes, and **never commits** |

## Install

### From source (recommended for 0.2.x)

Requirements: Rust ≥ 1.89 (edition 2024) and the macOS developer tools. The installed binary does **not** need Rust, Python, or Node.

```sh
git clone https://github.com/xiaodi007/multi-ai.git
cd multi-ai
cargo test --locked
./scripts/package.sh
cd dist/multi-ai-*-aarch64-apple-darwin
shasum -a 256 -c SHA256SUMS
./install.sh --no-legacy-aliases   # installs ~/.local/bin/multi-ai
./install-skills.sh                # optional: agent skill for Codex / Claude Code
```

- The binary goes to `~/.local/lib/multi-ai/<version>/`, and `~/.local/bin/multi-ai` points to it. Add `~/.local/bin` to `PATH` if it isn't there yet; the installer does not edit shell rc files.
- Every install makes a private backup first and prints `BACKUP_DIR=…`. To undo, run `./rollback.sh <BACKUP_DIR>`.
- Existing `~/.config/multi-ai/*.json` files are never overwritten, and credentials are never copied.

### From a GitHub Release

[Releases](https://github.com/xiaodi007/multi-ai/releases) currently ships **v0.1.0** only. That build predates `batch`, `delegate`, `scorecard` and the anti-sycophancy features, so build from source to get them.

```sh
gh release download v0.1.0 --repo xiaodi007/multi-ai \
  --pattern 'multi-ai-0.1.0-aarch64-apple-darwin.tar.gz*'
shasum -a 256 -c multi-ai-0.1.0-aarch64-apple-darwin.tar.gz.sha256
tar -xzf multi-ai-0.1.0-aarch64-apple-darwin.tar.gz && cd multi-ai-0.1.0-aarch64-apple-darwin
./install.sh
```

### Agent skill

`install-skills.sh` installs a `multi-ai` skill into `~/.agents/skills/multi-ai` (Codex: `$multi-ai`) and `~/.claude/skills/multi-ai` (Claude Code: `/multi-ai`). If a directory with that name already exists, it is left alone. To upgrade the managed files later, run `./install-skills.sh --upgrade`. Details are in [docs/AGENT-INTEGRATION.md](docs/AGENT-INTEGRATION.md).

## Configure

1. **Vendor CLIs (optional).** Install and log in to whichever you want to use: `codex`, `grok`, `kimi`, `agy`, `claude`. A missing CLI only disables its own routes.
2. **API keys (optional).** Copy [.env.example](.env.example) to `~/.config/multi-ai/.env` and fill in only the keys you use:

   ```sh
   DEEPSEEK_API_KEY=
   ZHIPU_API_KEY=        # or GLM_API_KEY
   KIMI_API_KEY=         # or MOONSHOT_API_KEY
   ```

   Values are filled in this order: process env → `./.env` → `~/.config/multi-ai/.env` → legacy `~/.config/multi-llm-pipeline/.env`. Keys are never printed.
3. **Check what you have**, offline:

   ```sh
   multi-ai routes          # every route with its pool, family, model source
   multi-ai doctor          # local config, credentials present, CLIs executable
   multi-ai doctor --models # (network) are configured model ids still served?
   ```

Config lives in `~/.config/multi-ai/` (`routes.json` for CLI routes, pools, ranks and timeouts; `profiles.json` for API providers, aliases, tasks and prompts). Run state and reports live in `~/.local/state/multi-ai/`. You can move them with `MULTI_AI_CONFIG_DIR` and `MULTI_AI_STATE_DIR`.

## Quick start

```sh
# One explicit route; vendor CLIs may use their own tools inside --cwd
multi-ai run kimi-k3-cli --prompt-file brief.md --cwd ~/src/project --out review.md

# API routes only see what you attach
multi-ai text glm --input evidence.md --out answer.md

# Parallel review by two seats from different quota pools,
# excluding the model family that wrote the change
multi-ai review --input diff.md --text-ready --outdir reviews --seats 2 --exclude-family openai

# Cross-check several reviews: opposite verdicts first, as an executable-verification list
multi-ai consensus reviews/*.md --out consensus.md

# Verify a list of findings against the code
multi-ai verify --findings review.md --cwd ~/src/project --out verify.md
```

### Delegation

```sh
# Snapshot only the materials you authorise, then let a cheap model investigate them
multi-ai pack --root ~/src/project --scope src/feature --input AGENTS.md --outdir /tmp/task/packet
multi-ai delegate investigate kimi-cli-default --packet /tmp/task/packet \
  --prompt-file task.txt --outdir /tmp/task/out --fallback grok-cli

# Sandboxed implementation in a clean worktree (never commits; reports out-of-bounds writes)
multi-ai delegate implement codex-cli --cwd ~/src/project-wt \
  --prompt-file brief.md --out /tmp/task/summary.md --timeout 1800
```

Roles: `investigate` (search and read code), `extract` (pull out and compare material), `challenge` (look for counter-examples), `implement`. See [docs/DELEGATION.md](docs/DELEGATION.md).

### Batch, measure, choose

```sh
multi-ai batch --jobs jobs.json --outdir results --pool-limit xai=2 --preflight --resume
multi-ai feedback --batch results --from verdicts.tsv     # record what you actually verified
multi-ai scorecard --task-type review --json             # precision / recall / latency per route
multi-ai recommend --task-type review --jobs 10          # suggested allocation
multi-ai stats                                           # offline run statistics
```

`jobs.json`:

```json
{
  "pool_limits": {"xai": 2, "kimi-code": 1},
  "jobs": [
    {"id": "review-one", "prompt_file": "brief.md", "cwd": "../worktree",
     "route": "kimi-k3-cli", "fallback": ["grok-cli"], "task_type": "review",
     "canaries": [{"match": "src/check.rs:42", "expect": "real bug"}]}
  ]
}
```

*Canaries* are facts the host has already confirmed. They are never sent to the model, and they measure recall. *Decoys* (`"decoy": true`) are deliberately false claims planted in the brief, and they measure how often a model just agrees.

## Command overview

| Command | What it does | Network |
|---|---|---|
| `routes`, `doctor`, `status`, `stats`, `help` | Inspect config, health, cooldowns, local history | offline |
| `run` | Call one explicit route (optional `--fallback`) | ✔ |
| `review` / `dispatch`, `verify` | Parallel review seats, finding verification | ✔ |
| `text` | Native text/API features (aliases, profiles, domains, vision) | ✔ |
| `batch` | Many jobs with per-pool concurrency, fallback, preflight, resume | ✔ |
| `pack`, `delegate` | Material snapshots; investigate / extract / challenge / implement | ✔ |
| `consensus` | Align several reports by `file:line`, list contradictions | offline |
| `feedback`, `scorecard`, `recommend`, `outcome` | Record verified results, score routes, suggest allocation, infer merge outcome from git | offline |
| `probe`, `usage` | Explicit live probe, account quota | ✔ |
| `reset`, `stop` | Clear a pool cooldown, stop one of your own running jobs | offline |
| `install-config` | Import config without overwriting or copying credentials | offline |

Units: `run/review/verify/batch` timeouts are in **seconds**. `text --timeout-ms` is in **milliseconds**.

## Safety model, in short

- Only materials you attach explicitly are sent. API routes cannot browse your repo.
- Deterministic tests never touch the network or real credentials.
- Raw responses, manifests and feedback stay in the local state dir. Feedback is append-only and bound to report hashes.
- `feedback` defaults to `source=agent`. A result counts as human-confirmed only with an explicit `--source human --human-confirmed`.
- Delegated implementation is sandboxed and checked after it runs. It still runs code on your machine, so keep it to worktrees you trust.

## Documentation

| Doc | Content |
|---|---|
| [docs/REFERENCE.md](docs/REFERENCE.md) | Full reference: every route, flag, failure class, scoring rule (Chinese) |
| [docs/DELEGATION.md](docs/DELEGATION.md) | Delegation workflow, boundaries and metrics |
| [docs/AGENT-INTEGRATION.md](docs/AGENT-INTEGRATION.md) | Skill discovery for Codex / Claude Code, legacy entries |
| [docs/CLI-ISOLATION.md](docs/CLI-ISOLATION.md) | How vendor CLIs are isolated from your host config |
| [docs/COMPATIBILITY.md](docs/COMPATIBILITY.md) | Compatibility with legacy `ai-review` / `llm-run` |
| [docs/VALIDATION-v0.2.md](docs/VALIDATION-v0.2.md), [docs/TOKEN-BENCHMARK-v0.2.md](docs/TOKEN-BENCHMARK-v0.2.md) | Validation and token-cost measurements |
| [docs/tasks/](docs/tasks/) | Design records for each change |

## Development

```sh
cargo fmt --all -- --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
sh tests/test-agent-skills.sh
```

The skill template lives in `integration/skills/multi-ai/`, and `scripts/package.sh` builds a release package into the ignored `dist/`. Contributor notes are in [AGENTS.md](AGENTS.md).

## License

[MIT](LICENSE)
