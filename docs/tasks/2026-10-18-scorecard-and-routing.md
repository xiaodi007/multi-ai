# 打分、选路与宿主反馈（2026-10-18）

先读 `AGENTS.md`、`README.md`、`docs/tasks/2026-10-18-batch-failover-and-route-benchmark.md`（含「宿主真机试跑后的待办」）、`integration/skills/multi-ai/references/route-traits.md`。在当前分支 `codex/batch-implement` 上继续。不提交、不推送、不发布。

## 用户 10-18 原话

「multi-ai 希望每次你能尽量给每个模型一个核验、打分之类的。能准确识别什么时候应该用什么模型、怎么用。你就是使用者，需要你的反馈把这个东西做得更完善。目的就是花其他模型额度，能尽量和你自己做是等效，节省你的 token，做更多的任务。当然中间有个动态平衡点，也需要找到。」

## 宿主（Claude）这一天当使用者的反馈

1. `delegate implement` 首版外层沙箱真机起不来（已修）。
2. 并行两个 implement 时越界核对互相误报，且误报后摘要不发布到 `--out`，只能去 progress 目录翻 `last-message.txt`。
3. 启动前快照约 25 秒，扫了同级工作树的 `node_modules`。
4. `multi-ai routes --json | head` 管道提前关闭时 panic（`failed printing to stdout: Broken pipe`）。
5. `batch` 跑完不打印每个任务的 `run_id` / `attempt_id`。要 `feedback` 必须自己去翻每个 `.status.json`，一次 5 条反馈写了个脚本。
6. 没有地方标「任务类型」，`stats` 只能按通路汇总，回答不了「甄别任务该用谁」。
7. **最关键**：甄别试点（19 包、约 336 条候选）里，两条已知答案都被判成误报。根因是宿主的提示词判据漏了一类（冗余），不是模型的问题。没有已知答案，宿主根本发现不了这件事。⇒ 已知答案（金丝雀）应当是一等功能，不是宿主每次手工去 grep。
8. 本次核验结果：Kimi 判真 5 条，宿主核实 5 条属实；Grok 判真 2 条，宿主核实 0 条属实；已知答案召回 0/2（提示词问题，修后重跑中）。这些已用 `feedback` 记下，但现在没有任何命令能把它们汇总成「谁擅长什么」。

## 要做

### A 任务类型

`run` / `review` / `batch` 任务清单 / `delegate` 都接受 `--task-type`（batch 清单里是 `task_type` 字段），取值自由字符串，建议值：`triage`、`review`、`investigate`、`extract`、`implement`、`walkthrough`。写进 metadata 和 stats。

### B 已知答案（金丝雀）

- `batch` 清单的每个任务可带 `canaries: [{ "match": "文件:行 或正则", "expect": "真问题" }]`；`run` 有 `--canary-file`。
- 报告完成后自动核对：报告里有没有提到这个位置，判定是否等于预期。结果写进 status：`canary_hits` / `canary_total`，并自动记一条 `source=canary` 的反馈。
- 匹配逻辑要宽：路径后缀匹配，行号允许 ±3。
- README 写明：金丝雀只能测召回，测不了精度；它从宿主已经确认的事实来，不要用模型自己的结论当金丝雀。

### C 反馈更省事

- `batch` 结束打印一张表：`id、route、status、seconds、run_id、attempt_id、canary`。
- `multi-ai feedback --batch <outdir> --from verdicts.tsv`：tsv 列为 `id accepted false_positive uncertain evidence`，批量写入。

### D 成绩单 `scorecard`

`multi-ai scorecard [--task-type X] [--since DATE] [--json]`，按「通路 × 任务类型」汇总：

- 样本数；
- 精度 = accepted /（accepted + false_positive）；
- 金丝雀召回；
- 耗时中位数与 P90；
- 失败率（额度、限流、超时分开）；
- 每条被采纳发现的宿主核验成本（如反馈里填了 `--screen-min`）。

样本少于 5 的格子标「样本不足」，不给结论。

### E 选路建议 `recommend`

`multi-ai recommend --task-type X [--jobs N]`：按成绩单给出各通路的分配比例。

- 默认把精度×召回最好的那家作为主力；
- 留约 20% 给样本不足的通路（探索），这样换了模型或 CLI 版本也能重新测出来；
- 额度池当前在冷却期的排除。

`batch` 支持清单里 `route: "auto"`，按 recommend 分配。

### F 平衡点

在 `scorecard` 的 JSON 和文档里给出宿主判断「该外包还是自己做」所需的数字：

- 每条采纳发现花了多少外部秒数；
- 每条采纳发现花了多少宿主核验分钟（来自反馈）；
- 漏报率（来自金丝雀）。

文档写一段判据：外包要划算，需要「宿主核验成本＋漏报×返工成本 < 宿主自己做的成本」。multi-ai 只提供前两项的实测数，第三项由宿主估计，不要假装能算出来。

### G 修前面的小问题

上面反馈 2～5 条。

## 边界与验证

- 确定性测试用假适配器，不联网、不读凭据。
- 保留旧输出语义、秒/毫秒单位。
- 四条验证命令全过；改了技能模板就同步 `integration/skills/multi-ai/SKILL.md` 与 `references/`，写一节「怎么选模型、怎么给分」，让宿主每次派活前读到。
- `sh scripts/package.sh` 重新打包。宿主会自己安装并真机试跑。

## 交接（写回本卡末尾，最后一条消息相同）

每项做了什么、验证数字、给宿主的一条示例命令。
