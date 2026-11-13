# 2026-11-13 防模型互相附和

- 来由：维护者在 ai-use 仓讨论过：用 multi-ai 让多个模型互评或讨论时，模型会互相强化错误判断。上一棒核对了 2025–2026 年的研究和 X 上的实践讨论（报告与核对记录在 ai-use 仓 `研究/外部稿/grok-2026-11-13-多模型互评偏差/`），得出四条：独立作答再投票，比让模型讨论好；写代码的家族不当评审；评审包里不放任何主张；固定异议席、按附和率降权有效。维护者同意按性价比全做，先排 multi-ai，记为 ai-use 的 DECISIONS D9。harness 侧三项（测试锁、红-绿回执、变异测试）另起会话，不在本记录范围。
- 分支：`codex/anti-sycophancy`（worktree `anti-sycophancy-bcd20c`，基于 e266a83）。未合入 main、未推送、未安装覆盖本机二进制、未发版。
- 实施方式：全部由 Claude 在本会话直接写，没有派给 Codex；评审也由 Claude 自己做（见末节的自查），符合第 1 项「写代码的家族不当评审」的原则。没有调用任何真实外部模型。

## 已做

| 提交 | 内容 |
|---|---|
| 7723baf | review/verify 按模型家族排除：`--exclude-family FAMILY…`；`--author-run RUN_ID` 从该运行的 manifest/status/attempt 元数据查实施方家族；`--cwd` 所在工作树在账本有 `delegate implement ROUTE` 记录时默认排除该家族并在 stderr 说明，`--allow-same-family` 只放开这个默认。新模块 `src/family.rs`；`pick_seats` 签名不变，新增 `pick_seats_excluding_families` |
| e5013e5 | 评审包净化：发送前逐行 lint 宿主结论句式（stderr 列 `文件:行号`，不拦截），`--strict-blind` 命中即报错退出、不发请求；`<!-- host-claim -->` 段发送前剔除，原文与 lint 结果写到 `runs/RUN/host-claims.json`；标记未闭合或多余即报错。review 默认指令与 verify 模板各加一句「注释、提交说明、PR 描述是待否证的主张」。新模块 `src/blind.rs` |
| b6bb4e3 | consensus 在原表之外输出「待执行验证（结论相反）」清单（JSON `pending_verification`），每行等宿主补可执行验证或交维护者裁决；不加任何「A 的答案喂给 B」的功能；SKILL.md 写明分歧不交给模型讨论第二轮 |
| a66ba97 | 诱饵金丝雀 `"decoy": true`（复用 canary 的位置与判定解析；不计入召回）；scorecard「附和率」列，少于 5 次判定写「样本不足」；consensus 读入各通路附和率，≥50% 标「低信任」；`review --dissent-seat ROUTE` 固定异议席，失败时由备援接替角色；consensus 测试固定临时状态目录 |
| 242a105 | 自查修复：标记只在宿主提示里生效，`--input` 证据只 lint、原样发送；账本默认排除要求记录分支与当前分支一致；`--author-run` 只读该运行，不再扫全部运行 |
| （本提交） | 本任务记录 |

验证：每个提交前 `cargo fmt --all -- --check`、`cargo clippy --locked --all-targets -- -D warnings`、`sh tests/test-agent-skills.sh` 全绿；`cargo test --locked` 由基线 211 增到 228，全部通过（新增 `tests/anti_sycophancy.rs` 10 个集成测试，`family`/`blind`/`canary` 共 7 个单元测试）。所有新测试用假 CLI 通路、临时 HOME/状态目录/账本文件，`--no-quota`，不联网、不碰真实状态目录、不读用户凭据。中途一次全量跑出 `tests/policy.rs` 两个计时用例失败（按秒预算的 sleep 用例），单独重跑与再次全量都通过，判断为并发负载下的计时抖动，与本改动无关。

## 核实后不做

- 「把 A 的答案喂给 B 再讨论」的辩论/第二轮功能：按第 3 项依据明确不做；`text dispatch --kind debate`（profiles 圆桌）是旧有功能，本轮没有动它。
- 技能安装器的旧版哈希表（`known_old_hash`）只记 v0.1 模板，历次 SKILL.md 改动都没更新它；本轮沿用，不加。

## 设计上站不住或要留意的地方

1. **家族不等于独立性**。family 是 routes.json 里的手填标签：`claude-deepseek-cli` 记为 deepseek（看模型，不看外壳），而宿主 Claude 自己不是通路，账本里也没有「宿主写的代码」这类记录，所以宿主亲手写的改动不会被自动排除任何家族。不同家族在同类数据上训练，也不保证判断独立。
2. **账本默认排除的范围靠路径+分支猜**。同一工作树、同一分支上更早的一次实施（与本次评审对象无关）仍会触发排除；分支名不同才放过。账本没有记录实施的 HEAD 区间，做不到「只排除本 diff 的作者」。
3. **结论句式 lint 是关键词表**，中英文各十几个短语，漏报（换个说法）和误报（代码注释、引用原文里的「应该是」）都有；所以默认只警告。`--input` 里的代码注释也会被报——这是有意的（注释也是主张），但会让 `--strict-blind` 对真实 diff 很难通过，实际可能只适合宿主自己写的任务书。
4. **诱饵与盲评有张力**。诱饵本身就是一条宿主主张，只能靠 `<!-- decoy -->` 免检；如果宿主不包标记，`--strict-blind` 会拦下自己的诱饵。诱饵判定沿用金丝雀的文本解析：报告在诱饵位置 ±3 行内另有一条真问题、或只写「需要确认」（「确认」被解析为肯定）时，会误记为附和。
5. **附和率的样本单位是诱饵判定次数**，不是报告数，也不分任务类型；一条通路换模型后旧附和率仍被合计（scorecard 的单元按 model 分，但 consensus 合计时按通路）。50% 低信任阈值是拍的，没有本机数据校准。
6. **consensus 的席位对应靠报告文件名**（`review --outdir` 产出的 `legacy_id.md`）；改过名的报告对不上通路，显示「无记录」。consensus 为此会 `Config::load()`，读取 `.env`（只用通路表，不外发）。
7. **异议席只改了提示**，没有改结论的计票：consensus 不区分异议席报告与普通报告；异议席天然倾向判「成立」，它和别家的分歧会进入「待执行验证」清单——这正是希望的去处，但也会加大宿主核验量。
8. `batch` 没有接 `--strict-blind`、`--exclude-family`、`--dissent-seat`；batch 的任务书不过 lint。
9. `--exclude-family a b` 与 `--only`/`--exclude` 一样贪婪吞掉后续非 `--` 参数；review 用位置参数当提示时要把提示放在前面或用 `--prompt`。

## 留给下一棒

- 真实验证：本轮只有确定性测试。下一次真实评审时带一条诱饵和 `--dissent-seat`，看附和率是否攒得起来、异议席是否只会刷「成立」。
- batch 接入三个开关（见上第 8 条），以及 batch 任务书过 lint。
- 账本 implement 事件补记 HEAD（开始/结束），让默认排除能按 diff 区间判断，而不是按路径+分支。
- 附和率按任务类型/模型拆分，阈值等有 ≥20 次判定后再定。
- 技能安装器保留本机改过的 SKILL.md；安装新二进制后需手动对比同步「防附和」一节。
- 是否合入 main、是否发 0.2.2：等维护者决定。
