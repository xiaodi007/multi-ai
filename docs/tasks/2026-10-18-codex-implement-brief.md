# Codex 任务书：multi-ai 批量调度、额度切换、实施通路与入口收口（2026-10-18）

你是本仓库的实施者。先读 `AGENTS.md`、`README.md`、`docs/tasks/2026-10-18-batch-failover-and-route-benchmark.md`（背景与 A～E 待办，别重测已有结论）和 `integration/skills/multi-ai/references/route-traits.md`。工作区里 `integration/skills/multi-ai/SKILL.md` 的修改和 `references/route-traits.md`、`docs/tasks/` 是宿主 10-18 写的，保留并在其基础上改。

## 用户 10-18 的要求（原话）

「multi-ai 你安排 codex 进行开发吧。以后都统一使用 multi-ai，其他路径删了。」

## 本轮范围

按顺序做，每项做完跑一次验证再做下一项：

1. **A 批量入口 `batch`**、**B 额度切换与限流退避**、**C 区分 quota / rate-limit 并在 status 写真实原因**：验收按原待办卡的表。
2. **D `stats`**：从本地运行记录汇总每条通路的次数、成功率、耗时中位数与 P90、失败类型、输入输出字节。
3. **F 实施通路**：现在 `codex-cli` 固定 `-s read-only`，宿主要让 Codex 改代码只能绕开 multi-ai 直接 `codex exec -s workspace-write`。加一个受控的实施入口（例如 `delegate implement codex-cli --cwd <worktree> --prompt-file <brief> --out <summary>`，或 `run` 的显式 `--write` 开关，你选一种并在 README 写清理由）：
   - 只允许写 `--cwd` 指定的目录，默认拒绝在含未提交修改的主工作树上写，除非显式 `--allow-dirty`；
   - 运行前后记录 `git status --short` 与 HEAD，摘要里列出改了哪些文件；
   - 不自动提交；
   - 支持 `--timeout`（秒）和 `PATH` 前置（宿主要用 Node 24：`--path-prepend ~/.nvm/versions/node/v24.19.0/bin` 一类参数）；
   - 失败、超时、额度用尽要和 run 一样写进 status。
4. **G 入口收口（"其他路径删了"）**：
   - 安装脚本与技能模板不再安装 `ai-review`、`multi-llm-pipeline` 两个兼容技能（`integration/skills/claude-ai-review.md`、`claude-pipeline.md`、`codex-pipeline.md` 一类），文档统一指向 `multi-ai`；
   - `ai-review`、`llm-run` 两个二进制别名：先 `grep -rn` 本机 `~/code`、`~/.claude`、`~/.codex` 下的调用点（只读，不改别的仓库），把结果写进交接；有调用点的列出来交给宿主处理，**本轮不删别名的分派逻辑**，只让安装脚本加 `--no-legacy-aliases` 并在 README 标为弃用；
   - SKILL.md 补一节「实施」：用第 3 项的入口，不再手写 `codex exec`。
5. **E 实测**本轮不跑（需要另行授权联网），保持待办。

## 边界

- 确定性测试不联网、不读凭据，用假适配器。
- 保留 run/review/verify 秒单位、text/llm-run 毫秒单位、旧输出语义。
- 改了技能模板要同步安装副本（`~/.claude/skills/multi-ai`、`~/.agents/skills/multi-ai` 若存在）并跑 `sh tests/test-agent-skills.sh`。
- 不提交、不推送、不发布。在当前分支工作即可。

## 验证（全部要过）

```sh
cargo fmt --all -- --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
sh tests/test-agent-skills.sh
```

再重新构建并安装本机二进制（按 `scripts/` 里现有安装脚本，保留用户配置），确认 `~/.local/bin/multi-ai routes` 与 `doctor` 正常、新子命令 `--help` 可见。

## 交接（写到 `docs/tasks/2026-10-18-batch-failover-and-route-benchmark.md` 的「交接」节，并把最后一条消息写成同样内容）

- 每项：做了什么、验收命令与结果（数字）、没做的及原因；
- 第 4 项的调用点清单；
- 新入口的一条示例命令（宿主下次直接照抄）。
