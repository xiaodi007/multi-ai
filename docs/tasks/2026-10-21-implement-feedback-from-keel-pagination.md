# Codex 任务书：实施通路的越界误报、反馈与成绩单（来自 keel 10-20 分页批次的真机使用，2026-10-21）

先读 `AGENTS.md`、`README.md`「受控实施」、`docs/tasks/2026-10-18-scorecard-and-routing.md`、`docs/tasks/2026-10-19-implement-network-and-browser.md` 的交接。只改本仓库；不提交、不推送。工作树里有 10-18、10-19 两轮未提交改动，全部保留，在其上改。

## 用户 10-20 原话

「multi-ai 的反馈补录 这个需要你结合最近这么几十个commit 有什么实际使用的反馈 提出来 然后进行优化修改」

## 真机证据（宿主 Claude，keel 仓 10-20 一天 22 次 `delegate implement codex-cli`）

状态文件在 `~/code/web2/keel-audit-raw/2026-10-20-list-pagination/summary-*.md.status.json`（只读，可用来做回放夹具的形状参考，**不要把其中的路径、业务内容抄进本仓库测试**；夹具自己合成）。

1. **越界判定 22/22 误报**。22 次内层 `response` 全是 ok、exit 0，摘要完整，宿主复核后 22 次全部合入；但顶层全部 `status=boundary_violation, complete=false, exitCode=1`。越界条数 1～8750，来源全是并发的别人：
   - 同级工作树里正在跑的 Playwright e2e 产物（`keel-wt-mrjourney/.ci/browser/...` traces、`.last-run.json`），这些文件被该仓 `.gitignore` 忽略；
   - 宿主自己同时在合并树、主树里改文件、跑门禁；
   - 主仓 `.git/` 里的 `index`、`worktrees/<name>/index`、`logs/`、`refs/`——任何工作树里跑 git 都会写；
   - 嵌在主仓里的 `.claude/worktrees/<name>/` 子工作树；
   - 并行的另两条实施泳道（a2/b2/c2 三次数字都是 3，a4/b4 都是 8750）。
   一次快照扫了 375,067 个条目（含 `keel-deploy-*` 等与本次无关的同级目录）。
2. **越界后反馈录不进去**：`feedback --run <run> --report <attempt>` 一律报 `report not found in run`（manifest 里没有登记报告）。22 次的复核结论因此全部无法入库。
3. **成绩单把越界当失败**：`scorecard` 里 codex-cli × implement 失败率 0.98（boundary_violation 50 次），`recommend` 会据此排除一条实际 22/22 被采纳的通路。
4. `task_type` 22 次里 21 次是 null（宿主没传 `--task-type`；`delegate implement` 本身就是实施，不该依赖宿主记得传）。
5. `model` 22 次全是 null、`model_source=cli-default-unknown`，换模型后成绩单分不出身份。
6. 状态 JSON 动辄数千条完整路径（最大 8750），`summary_published` 在 complete=false 时也是 true，宿主分不清摘要是正式的还是诊断版；22 份日志里 14 份停在警告行，没有统一的收尾行。
7. 现有反馈字段（accepted/false_positive）是为「发现类」报告设计的，表达不了实施结果。宿主真实的复核结论长这样：结局（合入／返工后合入／弃用）、宿主返工条数、是否另派返工轮、缺陷类别（这一批的分布：判据写两份 5 次、RM 登记号撞号 6 次、删注释/压格式 2 次、翻页失败清表 2 次、排序无唯一兜底 3 次、性能未达标 1 次、单测过而浏览器才抓到 2 次）。

## 要做

### A 越界：归因优先，未归因只警告
- 用 `codex exec --json`（或当前 CLI 等价的事件流）拿到 Codex 自己的文件编辑事件（apply_patch / 文件写入工具的目标路径）。**落在 cwd 之外的编辑事件 = 归因越界**，判 `boundary_violation`（照旧不回滚）。事件流格式以本机 codex-cli 实测为准，写进 `docs/COMPATIBILITY.md`；拿不到事件流时退回现有快照逻辑并在 status 写明 `attribution: unavailable`。
- 快照差异按类分桶：`git_ignored`（在所属仓 `git check-ignore` 命中）、`git_internal`（`.git/` 下除 `config`、`hooks/` 外）、`nested_worktree`、`sibling_worktree_tracked`、`claude_config`（skills/settings/CLAUDE.md）、`other`。只有 `claude_config` 与归因越界会让状态变成 `boundary_violation`；其余写进 `boundary_warnings`，状态为 `complete`（或新值 `complete_with_warnings`，二选一，选了哪个写进 README 与 COMPATIBILITY，并保证旧读者不把它当失败）。
- status 里每个桶只存计数＋最多 20 条样例路径，完整清单另存到 artifacts 目录的一个文件。
- 快照范围收窄：默认只扫 cwd 所属仓的同级 Git 工作树（`git worktree list`）与 Claude 配置路径，不再扫父目录下所有兄弟目录；保留显式 `--watch PATH` 追加。记录扫描条数与耗时。

### B 实施报告可录反馈
- 执行成功且摘要完整（无论有没有越界警告）时，把摘要登记为该 run 的报告，`feedback` 能按 run/attempt 找到。越界（归因）时也登记，但带标记。
- 实施类反馈字段：`--outcome merged|merged-after-rework|discarded`、`--rework-items N`、`--rework-rounds N`（另派了几轮返工）、`--defect-tags tag1,tag2`（自由字符串）、沿用 `--rework-min`、`--screen-min`、`--evidence`。旧字段照旧可用；同一报告的更正规则不变。
- `feedback --batch` 的 TSV 增加可选列支持上面这些字段（列名写进帮助）。

### C 成绩单与选路
- 失败率只统计执行失败（额度、限流、超时、错误、空结果）；越界警告单列计数，不进失败率；归因越界单列。
- implement 类型的格子给：样本数、合入率、返工后合入率、弃用率、每次平均返工条数、返工轮数、缺陷标签直方图、中位/P90 秒。不给 precision。
- `recommend --task-type implement` 按合入率与返工条数排序；样本不足照旧标注。

### D 身份与收尾
- `delegate implement` 未传 `--task-type` 时默认 `implement`。
- 从事件流或 codex 配置取模型名写进 `model`；取不到写明来源 unknown，别填猜测值。
- `summary_published` 之外加 `summary_kind: final|diagnostic`。
- 进度日志最后一行统一打印 `multi-ai: status=<status> exit=<code> run=<run_id> attempt=<attempt_id>`。

## 边界与验证
- 确定性测试用假 codex（输出合成的事件流），覆盖：cwd 内编辑＋同级工作树 ignored 文件变化 → complete＋警告；cwd 外 apply_patch → boundary_violation；`.git/index` 变化 → 警告；Claude skills 变化 → boundary_violation；越界后 feedback 能写入；scorecard 不把警告算失败。
- 保留旧输出语义与单位；旧状态记录（没有新字段的）在 stats/scorecard 里照旧可读，**并提供一个离线迁移或重算方式**，让 10-20 那 22 条旧记录在补录反馈后能被 scorecard 正确归为「执行成功＋越界警告」（例如按 `response.exitCode==0 && 摘要存在` 重判；写清判据）。
- 四条验证命令全过；同步 `integration/skills/multi-ai/SKILL.md` 与 `references/`（实施一节写：越界警告的含义、怎么录实施反馈、缺陷标签建议值）。
- `sh scripts/package.sh` 重新打包。宿主随后安装并真机试跑。

## 交接（最后一条消息）
改了哪些文件；事件流实测得到的字段名；四条验证命令的数字；打包产物路径；旧记录重判的命令。

## 本轮交接（2026-10-21）

在已有 10-18、10-19 未提交修改之上实施，全部保留；未提交、未推送、未安装、未启动真实外部模型或其他实施会话，未读取业务报告。验证临时文件、隔离状态、日志和发布包均放在本工作树 target/dist 下。

本轮涉及的文件：

- `src/implement.rs`、`src/implement_tests.rs`：固定 `exec --json`；file_change 编辑路径归因；工作树列表限定扫描、显式 --watch、快照扫描条数/耗时；差异分桶和20条样例上限、完整 artifacts 清单；完整越界诊断报告登记；默认 task_type=implement；summary_kind 和统一 stderr 收尾行。
- `src/review.rs`、`src/config.rs`：展示和执行共用配置模型解析；配置冲突保守 unknown；配置发现限定工作树/HOME，HOME 缺失不回退到宿主；事件模型与配置不同时拆开版本身份。
- `src/cli.rs`、`src/runtime.rs`：新增实施反馈字段与 TSV 可选列，沿用证据、哈希和追加更正，允许已登记完整诊断摘要反馈；更新帮助。
- `src/scorecard.rs`、`src/stats.rs`：执行结果与边界政策分开统计；不同已记录模型分组；实施合入/返工/弃用、均值和标签直方图；实施主力按合入率优先、较少返工次序，保留探索和身份门槛。
- `src/delegate_cli.rs`：版本探测失败时附带安全的退出/IO原因，帮助定位此前已出现的间歇失败；不输出厂商版本命令原始 stderr 或凭据。
- `tests/implement.rs`、`tests/scoring.rs`：纯合成事件和虚构 Git 仓夹具；覆盖 ignored/index 警告、外部编辑归因、Claude 配置与符号链接根、嵌入工作树、反馈/更正/正文损坏拒绝、旧记录重判、模型分组及实施选路。
- `scripts/reclassify-implement.py`：离线预演/显式应用旧记录重判，备份原始元数据，登记摘要哈希，拒绝失败或损坏摘要；`scripts/package.sh` 将脚本打包并纳入 SHA256SUMS。
- `README.md`、`docs/COMPATIBILITY.md`、`integration/skills/multi-ai/SKILL.md`、`references/delegation.md`、`references/route-traits.md`：同步口径、命令、标签和边界；本任务卡追加交接。

事件格式检查：本机 codex-cli 0.163.0 的离线 `exec --help` 确认 --json；安装二进制 exec 序列化字段字面量包含 thread.started、turn.started、turn.completed、item.completed、file_change、changes、path、add/delete/update。实现解析 item.type=file_change、item.changes[].path；合成流使用 item.status/kind，模型仅在事件明确给 model 时记录，否则共用配置解析。**按本次禁止运行其他实施器的限制，没有真实 exec 会话的事件流实测；真实字段形状仍须宿主安装后验收。** 缺失/损坏流标 attribution=unavailable，不据快照归因。成功值选择保留 `ok/complete=true/exitCode=0`，没有引入旧读者可能误判的新状态。

最终验证（TMPDIR 和 MULTI_AI_STATE_DIR 指向工作树 target/task-20261021 下的隔离目录）：

- `cargo fmt --all -- --check`：退出 0。
- `cargo test --locked --no-fail-fast`：退出 0，15 个测试目标汇总 **165 通过、0 失败、4 忽略**；实施 17/17、成绩单 9/9、受限调查 8/8。忽略项为已有的可选厂商真机/边界测试，本轮未启用。
- `cargo clippy --locked --all-targets -- -D warnings`：退出 0，0 警告。
- `sh tests/test-agent-skills.sh`：退出 0，安装、保留、升级、回退、校验和及符号链接保护均通过。
- `git diff --check`：退出 0。
- `sh scripts/package.sh`：退出 0，包内 **20/20** SHA-256 校验；归档校验通过。发布包携带离线重判脚本和更新后的技能参考。

排查经过保留：首次完整测试因新 routes 展示在空配置环境回退宿主 HOME 而有1项单测失败，已修正，随后38/38单测通过。之后 `cargo test --locked` 在既有受限调查测试中有2项间歇失败：oauth_values_echoed_without_bearer_prefix_are_redacted（缺 stderr.raw）和 adapter_uses_private_cwd_and_home_and_drops_project_environment（cannot determine vendor CLI version）。补充版本探测错误原因后，单独默认并发复跑8/8、完整默认并发复跑165/165通过；间歇问题根因尚未确认，未宣称已经根治。日志保存在 `target/task-20261021/logs/`：cargo-test.log（中途失败）、delegate-diagnosis.log、full-diagnosis.log（最终全目标通过）、fmt.log、clippy.log、skills.log、package.log。

打包产物：

- `dist/multi-ai-0.2.0-aarch64-apple-darwin/`
- `dist/multi-ai-0.2.0-aarch64-apple-darwin.tar.gz`
- `dist/multi-ai-0.2.0-aarch64-apple-darwin.tar.gz.sha256`

旧记录重判（宿主在停止相关写入后执行；默认预演，确认 eligible 再加 --apply；多条可重复 --status）：

```sh
python3 scripts/reclassify-implement.py --state-dir STATE --status SUMMARY.md.status.json
python3 scripts/reclassify-implement.py --state-dir STATE --status SUMMARY.md.status.json --apply
multi-ai feedback --run RUN --report ATTEMPT --outcome merged-after-rework \
  --rework-items 2 --rework-rounds 1 --defect-tags criterion-duplicated,id-collision \
  --evidence '宿主实际复核与门禁证据'
multi-ai scorecard --task-type implement --json
```

发布包中脚本位于包根目录。判据为旧无 attribution 的 boundary_violation、内层 response ok/complete/exitCode=0、summary_published=true、Git 检查正常、发布摘要以非空完整 last-message 正文结尾，以及 run/attempt/manifest/status/artifacts 身份匹配。原始记录和长路径清单留在 legacy-before-reclassification.json，旧诊断正文不改；不会推断历史模型、采纳、返工数量。每文件原子替换、manifest 最后发布；中断需按备份人工恢复后重试。

未完成：宿主安装、真实事件流和真机 PG/Playwright 验收；22条真实旧记录的重判和宿主反馈补录（本轮仅用合成夹具验证）；上述版本探测间歇失败的根因仍待后续定位。
