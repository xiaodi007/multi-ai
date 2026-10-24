# 2026-10-24 自查修复与 0.2.1

- 来由：维护者要求「基于那么多轮的调用，发现需要完善的地方」。依据本机 1433 次运行记录（stats/scorecard/manifest 扫描）与 keel 记忆里记过的坑。
- 分支：`codex/self-audit-fixes`，已 ff 合入 main 并推送（含先前未合的 `codex/claude-shell-routes` 三个提交）。
- 本机：从 `dist/multi-ai-0.2.1-aarch64-apple-darwin` 安装；`~/.local/lib/multi-ai/0.2.0` 保留可回退。0.2.1 未打 tag、未建 GitHub Release。

## 已做

| 提交 | 内容 |
|---|---|
| 13e83fc | 进程内测试不再写真实状态目录（历史 637 条 transport-test 移到 `~/.local/state/multi-ai/backups/transport-test-leak-20261024/`）；无 `--out` 运行结束时 `current_status` 不再停在 running；scorecard 加 MODEL 列；新失败类 `model` |
| 62d4e18 | 沙箱写不到的 `~/.claude` 变化记 `claude_config_outside_sandbox` 警告（维护者选定，代价：沙箱被绕过只剩警告）；`batch --preflight` |
| 2666440 | `lost` 判定（`alive.lock` + pid，旧记录 24h）；样本不足写明缺项；缺陷标签同义归并；`scripts/backfill-codex-model.py`（已对本机 apply，回填 90 次，备份 `metadata.before-model-backfill.json`）；route-traits 10-24 复盘段 |
| 46343d3 | `delegate implement --preflight`；batch 写 `verdicts.template.tsv`，`feedback --batch` 跳过空行；版本 0.2.1 |
| 105591e | 子命令 `--help` 列出与校验同表的参数 |

验证：每批 `cargo fmt --check`、`cargo test --locked`（最终 210 通过）、clippy `-D warnings`、`tests/test-agent-skills.sh`、`tests/test-install.sh` 全绿；每次测试前后真实 `runs/` 数量不变。实跑冒烟（scratchpad，玩具提示）：`batch --preflight` 两任务 deepseek-pro-api 各约 2 秒成功，preflight 运行探了主备两条通路，新运行带 pid/alive.lock，模板生成。

## 核实后不做

- glm-api 截断 24 次：全是 10-17 前旧记录。
- 清理不可用通路：12 条通路 10-24 01:46 全量探测都成功（当日已充值），无需清理。
- probe 临时目录：结束时删除，不堆积。
- 版本探测 3 秒超时、文档「额度」误判冷却、codex 模型未知：先前已修。

## 未做 / 留给下一棒

- `implement --preflight` 只在假 CLI 用例验过，未对真实 codex-cli 跑；下次真实派活带上即可顺带验证。
- scorecard 仍大多「样本不足」，缺人工核对与金丝雀：派 batch 后用 `verdicts.template.tsv` 录反馈。
- 技能安装器会保留本机改过的 SKILL.md；升级后要手动对比同步。`~/.claude/skills/multi-ai/references/route-traits.md` 有维护者自加的回归集一行，别覆盖。
- 是否打 `v0.2.1` tag / 建 Release：等维护者决定。
