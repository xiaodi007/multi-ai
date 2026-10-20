# 批量调度、额度切换与各通路实测（待办）

状态：A/B/C/D/F/G 仓库实现完成；本机部署与受限环境验收未完成，E 待另行授权。用户 2026-10-18 要求：把 multi-ai 怎么更好用、更省主模型额度做成待办，交给其他 AI 实施；Gemini 等其他模型也要测。

## 背景（别重做）

- 2026-10-17/18 在 Keel 用 multi-ai 跑了约 60 次调用：102 个提交提炼、46 条问题分类、12 例盲审回放、12 例判分。实测特性已写进 `integration/skills/multi-ai/references/route-traits.md`，先读它，不要重新测已有结论。
- 当时宿主写的包装脚本：每个任务一个文件，状态行追加到 status 文件，按通路用 `xargs -P` 限并发，失败的只按 id 重跑。产品里缺的正是这一层。
- 10-04 的 token 对照（`docs/TOKEN-BENCHMARK-v0.2.md`）显示，短任务委派反而多 8.9 倍 token。省额度的前提是任务大到宿主自己读会更贵，比如 10 万字级材料、要读整个仓库。新功能不要再宣称「省 token」，除非有对照数据。

## 观察到的问题

1. **`run` 不会自动切换**。kimi-k3-cli 在 5 小时窗口用尽后返回 `403 You've reached your 5-hour usage limit`。`classify()` 能把它判成 quota，但 `run` 只走一条通路，同批 14 个任务几秒内全挂。`review` 有按池备援，但它不是批量单任务的形状。
2. **没有批量入口**。宿主只能自己写 xargs 脚本，还撞过 zsh `xargs` 的「command line cannot be assembled」。
3. **限流没有退避**。GLM 同时 6 路，有 2 路返回 `429 code 1302`（速率限制，不是余额），串行重跑就成功了。现在这类错误直接算失败。
4. **status.json 的 `error` 取错内容**。kimi 失败时，`error` 字段装的是 CLI 回显的 git status 片段，真实原因只在 `.log` 尾部。
5. **耗时与成败没有本地统计**。route-traits.md 只能手工整理，换了套餐或 CLI 版本就过期。

## 待办

| # | 内容 | 验收 |
|---|---|---|
| A | `batch` 子命令：读一份 jobs 清单（id、prompt 文件、cwd、首选通路、备选通路），按额度池限并发，每个任务独立输出和状态，可 `--resume` 只重跑未完成的 | 假适配器跑 20 个任务：并发上限生效、单任务失败不影响其他、重跑只跑失败 id |
| B | 额度切换：quota 类错误把该任务立刻转到备选通路，同池后续任务在窗口内直接跳过该池；rate-limit 类（429 1302 / too many requests）先退避重试 N 次再算失败 | 假适配器分别返回 5 小时上限和 1302，各断言一次走向 |
| C | 区分 quota 与 rate-limit 两类，并在 status 里写真实原因：从 stderr 尾部取匹配 `classify` 的那一行，不取回显的工作区内容 | 用 10-17 kimi 日志尾部做固定样例的单测 |
| D | `stats`：从本地运行记录汇总每条通路的次数、成功率、耗时中位数和 P90、各类失败数、输入输出字节，让 route-traits.md 能用数据刷新 | 对现有本地运行记录跑出一张表 |
| E | 各通路实测（需用户授权联网）：用固定的虚构材料，同一组任务各跑一遍 | 见下表，结果补进 route-traits.md，注明日期和版本 |

E 的测法：三类任务，每类 2～3 题，各通路同题，记录耗时、输出大小、是否按格式、是否答对（对照固定答案）、失败类型。

| 任务类型 | 例子 | 需要的能力 |
|---|---|---|
| 读仓库找问题 | 在虚构小仓里埋 3 个已知缺陷，盲审 | CLI，带 cwd |
| 按表抽取 | 10 万字材料填固定列的表 | 长输入 |
| 判分合并 | 给真值与两份答案，判命中 | 短输出、遵守格式 |

要覆盖的通路：`grok-cli`、`kimi-k3-cli`、`kimi-cli-default`、`agy-cli`（Gemini，旧记录说一半惰性，要显式 Pro 型号）、`codex-cli`、`glm-api`、`deepseek-pro-api`、`claude-deepseek-cli`。每条另测「同时几路开始撞限流」「多少任务撞窗口额度」，查不到就写「未测到上限」，不要猜。

## 边界

- 按仓库 AGENTS：确定性测试不联网、不读凭据；E 只在用户明确授权时跑，用虚构材料，不用 Keel 业务数据。
- 保留 `run/review/verify` 的秒单位与 `text` 的毫秒单位、旧别名和输出语义。
- 验证按 AGENTS 的四条命令；改了 skill 模板要同步安装副本并跑 `tests/test-agent-skills.sh`。
- 未经用户要求不提交、不推送、不发布。

## 交接

仓库实现 A/B/C/D/F/G 已完成；本机部署与完整验收未完成。宿主 10-18 已有的 SKILL 引用、route-traits 初版和任务材料均保留并在其基础上修改。没有提交、推送、发布或调用真实外部模型。

| 项目 | 实现与验收结果 |
|---|---|
| A batch | JSON jobs、按池限并发、独立输出/status、严格恢复。`cargo test --locked --test batch` 4/4 通过：20 任务最高并发 2，首轮 19 成功＋1 预设失败，恢复仅新增 1 次调用。 |
| B 切换/退避 | quota 立即备援并冷却；1302 指数退避；重试、备援及备选池排队共用预算。batch 4/4、`cargo test --locked --test policy` 9/9：quota 原池仅调用 1 次，4 任务全部备援成功；1302 第 3 次调用成功。也覆盖过期不启动和 review/verify；批次取消验收为 6 任务仅启动 1 次、全部 cancelled、2 份旧报告保留。 |
| C 真实原因 | 区分 quota/rate_limit，忽略工作区状态行，取匹配诊断。`cargo test --locked --test review_native` 22/22 通过。Kimi 固定样例依据任务卡错误重建，没有读取业务日志原件。 |
| D stats | 离线表格/JSON、去重回补、成功率、中位/P90、失败及字节样本。`cargo test --locked --test stats` 11/11；既有记录为 708 runs、489 attempts、461 调用、349 成功、5 缓存、23 跳过、20 组通路/别名；308 个进度目录缺失、36 个 metadata 无效或缺失，均显式报告。完整表已附任务卡。 |
| F 实施 | `delegate implement codex-cli`：cwd 写边界、dirty 保护、Git 前后状态/HEAD、Git 可见文件变化、PATH 前置、秒超时、失败保留摘要、不提交。`cargo test --locked --lib implement::tests` 15/15，`cargo test --locked --test implement` 3/3。系统 sandbox 在当前环境无法启动，已验证失败关闭；真实隔离执行仍待宿主补验。 |
| G 收口 | 删除 5 个旧技能模板，安装/升级只面向 multi-ai；新增 `--no-legacy-aliases`，保留二进制分派和已有旧别名；README/SKILL 已更新。`sh tests/test-agent-skills.sh` 通过；`sh tests/test-install.sh` 11 场景、54 断言通过；9 份 shell 语法检查通过。 |
| E 实测 | 0 次，按要求保留待办，等待另行联网授权。 |

`cargo fmt --all -- --check`、Clippy（0 警告）及技能检查通过。全量扩展检查 `cargo test --locked --no-fail-fast` 首轮为 102 通过、36 失败、4 忽略：35 个测试在本地 socket 监听时被拒绝；另 1 个旧短超时用例已改为实际 PID 检查，随后 policy 9/9、执行层 22/22 复验通过。合并最近分项结果，非监听项 104 个通过；完整测试命令仍未全绿。

`sh scripts/package.sh` 已重新构建 arm64 本地包，19 个文件校验和及归档校验和通过。新产物的 routes/doctor 各退出 0（均 9 条通路），3 项新入口帮助检查通过；`~/.local/bin/multi-ai routes`、`doctor` 也各退出 0，但仍是旧安装。

实际二进制安装与 `install-skills.sh --upgrade` 均在 `~/.local/state/multi-ai/backups` 创建备份时被 `Operation not permitted` 拒绝。环境只允许写本仓库和临时目录，因此二进制、`~/.claude/skills/multi-ai`、`~/.agents/skills/multi-ai` 尚未更新；7 个安装/配置/技能路径核对不变。需宿主在可写环境完成安装、技能合并同步及上述受限验证；升级器会保留未知修改并给出 MERGE_GUIDE。

旧入口调用点已只读扫描，交宿主处理：

- `~/.claude/skills/multi-llm-pipeline/bin/run.sh:3`：执行 llm-run。
- `~/.claude/settings.json:46,51,52`：llm-run、两种 ai-review 入口权限。
- `~/.claude/commands/round.md:25`、`explore-packet.md:2`：旧调用或通路称呼。
- `~/.claude/skills/ai-review/SKILL.md:89–114,152,159`：旧命令配方。
- `~/.claude/skills/multi-llm-pipeline/SKILL.md:21,23,25,27,29,47,52,53,59,66`：旧命令/runner。
- `~/.codex/skills/multi-llm-pipeline/SKILL.md:10,12,15,16`：旧帮助/恢复命令。
- 旧技能附属 README/PATTERNS/GOTCHAS、bin/test 和 Keel/mb 历史文档的具体位置见任务卡调用点附录。其他仓库执行脚本调用点为 0；4 处同名 GitHub Action 属于其他工具，未修改。旧别名分派本轮未删。

新入口示例（在本仓库执行，先将 WORKTREE、BRIEF 设为干净 Git 工作树根目录和已授权任务书的绝对路径；当前可直接使用新本地包）：

```sh
./dist/multi-ai-0.2.0-aarch64-apple-darwin/bin/multi-ai delegate implement codex-cli \
  --cwd "${WORKTREE:?设置干净工作树绝对路径}" \
  --prompt-file "${BRIEF:?设置任务书绝对路径}" \
  --out /private/tmp/multi-ai-implementation-summary.md \
  --timeout 1800 \
  --path-prepend ~/.nvm/versions/node/v24.19.0/bin
```

## 本地 stats 验收快照（2026-10-18，离线）

读取既有本地记录，未重新调用模型。记录混合历史任务与既有测试，缺失字段不补猜；这些数字不替代 E 的同题实测。耗时含失败样本，单位毫秒。

| 记录通路/别名 | 调用 | 成功率 | 中位 ms | P90 ms | 输入字节（已知） | 输出字节（已知） | 失败类型 |
|---|---:|---:|---:|---:|---:|---:|---|
| `agy-cli` | 56 | 82.1% | 318998 | 900600 | — | 651223 | cancelled=2、empty=1、error=1、timeout=6 |
| `claude-deepseek-cli` | 2 | 100.0% | 514722.5 | 1022903 | — | 10037 | — |
| `codex-cli` | 7 | 85.7% | 107681 | 627500 | — | 24665 | error=1 |
| `deepseek-flash` | 5 | 80.0% | 2615 | 3329 | 231535 | 7201 | invalid_result=1 |
| `deepseek-pro-api` | 47 | 76.6% | 20951 | 199591 | 11364508 | 254310 | error=5、quota=2、truncated=4 |
| `glm-api` | 48 | 33.3% | 231120.5 | 396040 | 7754383 | 90451 | error=6、incomplete=1、timeout=1、truncated=24 |
| `grok-cli` | 55 | 83.6% | 542400 | 943175 | — | 286658 | auth=2、error=1、incomplete=1、invalid_result=1、timeout=4 |
| `kimi-cli-default` | 33 | 48.5% | 196222.5 | 635528 | — | 38697 | auth=3、error=4、incomplete=1、invalid_result=6、timeout=3 |
| `kimi-k3-api` | 9 | 22.2% | 190779 | 784500 | 1258372 | 8707 | error=1、quota=3、truncated=3 |
| `kimi-k3-cli` | 40 | 60.0% | 326773.5 | 844317 | — | 149426 | auth=4、error=2、incomplete=1、invalid_result=1、quota=4、timeout=4 |
| `text:deepseek-flash` | 37 | 100.0% | 5793 | 14490 | 1052361 | 330856 | — |
| `text:deepseek-pro` | 12 | 100.0% | 14203.5 | 27192 | 994232 | 78923 | — |
| `text:deepseek/deepseek-v4-pro` | 10 | 100.0% | 45739.5 | 48149 | 1708893 | 307131 | — |
| `text:glm` | 49 | 89.8% | 97877 | 262554 | 3519978 | 207392 | quota=2、truncated=3 |
| `text:glm-flash` | 1 | 100.0% | 6242 | 6242 | 870 | 727 | — |
| `text:glm/glm-5.3` | 14 | 92.9% | 288326 | 379910 | 4284549 | 373634 | truncated=1 |
| `text:kimi-k3` | 3 | 100.0% | 171483 | 410186 | — | 16861 | — |
| `text:kimi-k3-high` | 2 | 50.0% | 488474 | 676056 | 704722 | 9288 | truncated=1 |
| `text:kimi-k3-low` | 20 | 95.0% | 128034.5 | 205966 | 1713475 | 302444 | quota=1 |
| `text:kimi-k3/kimi-k3` | 11 | 100.0% | 170947 | 207036 | 1814319 | 206703 | — |

合计：708 份 manifest、489 个去重 attempt；461 次调用、349 次成功、5 次缓存、23 次跳过；308 个 progress 缺失、36 个无效或缺失 metadata。CLI 历史输入字节缺失，表内合计只涵盖已知样本；`stats --json` 提供各指标样本数。

## 入口调用点清单（2026-10-18，只读）

对 `~/code`、`~/.claude`、`~/.codex` 使用递归 `rg` 扫描（与 grep -rn 等价），只输出匹配词、文件和行号，排除凭据、会话、日志和缓存。未修改其他仓库或用户旧入口。下列依赖交宿主处理，本轮保留二进制别名分派。

| 位置 | 调用或依赖 |
|---|---|
| `~/.claude/skills/multi-llm-pipeline/bin/run.sh:3` | 执行 `~/.local/bin/llm-run` |
| `~/.claude/settings.json:46,51,52` | 三条旧入口 Bash 权限：llm-run、~/.claude/bin/ai-review、ai-review |
| `~/.claude/commands/round.md:25` | `llm-run dispatch --domain product --kind debate` |
| `~/.claude/commands/explore-packet.md:2` | judge 通路仍称 llm-run |
| `~/.claude/skills/ai-review/SKILL.md:89–114,152,159` | 旧 run/verify/probe/usage/status/dispatch/reset 配方 |
| `~/.claude/skills/multi-llm-pipeline/SKILL.md:21,23,25,27,29,47,52,53,59,66` | 旧命令与 runner 配方 |
| `~/.codex/skills/multi-llm-pipeline/SKILL.md:10,12,15,16` | 旧帮助和恢复配方 |
| Claude pipeline `README.md:17–33`、`PATTERNS.md:53,54,124`、`GOTCHAS.md:3` | 旧命令示例 |
| Claude/Codex pipeline `bin/llm-run.mjs`，Claude `bin/transport.mjs`、各旧技能 `test/` | 旧实现与测试，保留交宿主处理 |
| `~/.claude/skills/multi-ai/SKILL.md:30` | 原有兼容说明；独立新技能安装副本需同步 |

`~/code` 非本仓库未发现执行这两个二进制的脚本。仍有文档调用配方：`~/code/web2/keel/docs/协作/README.md:110,113,114,115,123`、`远程并行开发方案-2026-09-21.md:213`、`任务/2026-10-01-多AI离线帮助入口.md:12,22,23,28`、`任务/2026-10-03-multi-ai安装后实测.md:34` 及同目录历史任务；相同历史文档还在 keel-r4-fields、keel-deploy-*、keel-audit-raw、`.codex/boundary-artifacts` 快照中。`~/code/web2/mb/docs/05-依据档案/方法论轮次-round1-6/round2-架构深评与K3实战测评-2026-07-22.md:126` 也是历史示例。

四处 GitHub Action 名称命中是 `travixFinance/travix-ai-reviewer@main`，不是本工具：`~/code/work/{perpdex-proto,travix-perp-dex-fix-user-safety,travix-perp-dex}/.github/workflows/ai-review.yml:32` 与 `~/.codex/worktrees/f6e0/travix-perp-dex/.github/workflows/ai-review.yml:32`。没有据此修改业务项目。

## 宿主真机试跑后的待办（2026-10-18）

- `delegate implement` 首版外层 seatbelt 真机起不来，已按 `2026-10-18-implement-sandbox-fix.md` 修复并安装；K5、K6 两个 Keel 实施任务经它完成。
- **越界核对在并行任务下误报**：两个 implement 同时跑时，各自把对方工作树的写入、以及宿主会话写 `~/.claude/projects/*.jsonl`、`~/.claude/sessions/`、`~/.claude/skills/synced/` 报成 `boundary_violation`，摘要因此未发布到 `--out`（只在 progress 目录的 last-message.txt）。建议：排除同批其他 implement 任务的 cwd；`~/.claude` 只监测 `skills/`（排除 `synced/`）、`settings*.json`、`CLAUDE.md`，不监测会话与项目记录；越界时仍发布摘要并在顶部标注。
- 启动前快照约 25 秒（递归扫同级工作树，含 node_modules）。建议跳过 `node_modules`、`target`、`dist`、`.git/objects`。


---

A–G 已实现，交接已写回[任务卡](/Users/xiaodi/code/multi-ai/docs/tasks/2026-10-18-batch-failover-and-route-benchmark.md)。全量验收仍受环境限制。

| 项目 | 修改内容 |
|---|---|
| A | 四入口支持任务类型，写入元数据和 stats。 |
| B | 金丝雀支持路径后缀、行号 ±3、正则；本地核对并自动追加绑定报告哈希的反馈。 |
| C | batch 打印任务身份表；支持 TSV 批量反馈、整批预检和追加更正。 |
| D | scorecard 按通路×类型汇总精度、召回、耗时、失败率和成本；未知留空，低样本不下结论。 |
| E | recommend 主力＋约20%探索；auto 支持严格恢复，排除冷却及本地缺依赖的通路。 |
| F | 提供每条采纳发现的外部秒数、宿主核验分钟、漏报率；文档补充外包判据。 |
| G | 修复并行边界误报、快照扫描缓存、越界摘要不发布及 routes Broken pipe。 |

README、兼容说明、SKILL.md 和参考文档已同步。

验证使用工作树内临时/状态目录及 Cargo 离线模式：

- `cargo fmt --all -- --check`、`cargo clippy --locked --all-targets -- -D warnings`、`sh tests/test-agent-skills.sh`：通过。
- `cargo test --locked`：未通过。补充串行 `cargo test --locked --no-fail-fast`：**123通过、35失败、4忽略**；35项均因沙箱拒绝回环 HTTP 监听（EPERM）。评分集7/7、实施12/12、评审22/22通过。
- `sh scripts/package.sh`：成功；19个文件及归档 SHA-256 校验通过，包内离线冒烟2/2通过。[本地包](/Users/xiaodi/code/multi-ai/dist/multi-ai-0.2.0-aarch64-apple-darwin.tar.gz)。

未完成：35项受限测试需在允许本地监听的环境复验；安装和真机试跑由宿主完成。本轮未提交、推送或发布，真实外部模型调用0次。

宿主示例命令：

```sh
./dist/multi-ai-0.2.0-aarch64-apple-darwin/bin/multi-ai scorecard --task-type triage --since 2026-10-18 --json
```
