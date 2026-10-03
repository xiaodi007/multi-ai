---
name: multi-llm-pipeline
description: Use multi-ai text (legacy llm-run) for requested DeepSeek, Kimi, or GLM text processing and useful external second opinions. Repo tools and small edits stay on the host.
---

# Multi-LLM Pipeline (Codex entry)

Read the canonical skill at `~/.claude/skills/multi-llm-pipeline/SKILL.md` for the workflow and material-authorization policy. This compatibility entry is installed only when that existing canonical skill is present. New users install the standalone `multi-ai` skill instead.

- Rust binary: `~/.local/bin/multi-ai`; `llm-run` and `ai-review` are compatibility entries to the same executable. API calls need no Node/Python.
- Canonical runner: `~/.claude/skills/multi-llm-pipeline/bin/run.sh`. This directory's `bin/run.sh` forwards there; do not maintain a second implementation.
- Search/count/check with host tools. Use available read-only subagents for large separable exploration; do not nest `codex exec` or spawn an agent just to run llm-run.
- A packet does not require a model call. Use external extraction or review only when needed; no default map or roundtable.
- Current routing: `profiles`, `domains`, `aliases`. These list local configuration; do not call `list-models` for an offline check.
- Learn locally with `llm-run help` and `ai-review help`; use `ai-review help run|dispatch|verify` for current parameters. These help entries do not call models.
- GLM defaults to streaming with durable progress files; completeness, time budgets, explicit material, and `ai-review --resume` are documented only in the canonical skill.
- From Codex, do not auto-pick Grok/xAI unless the user explicitly requests that provider.

Patterns and protocol details are maintained only in the canonical tree. Local historical scripts/configuration remain compatibility-only.

After actual report validation, record report-level feedback with `multi-ai feedback --run ID --report ATTEMPT-ID --evidence TEXT` and known counts only. Default source is `agent`; `human` requires an explicit user decision and `--human-confirmed`. Model CONFIRMED and passing tests do not imply human acceptance. See canonical skill for output ownership, history and exact resume rules.
