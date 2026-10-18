# v0.2 实际 token 收益对照

状态：完成。用户明确要求实测实际节省，授权固定虚构材料的 Codex/Kimi/Grok 联网对照。

目标：同一当前配置 Codex 主模型/推理等级，六个任务分别直接处理 A 与委派后核验 B，记录真实 usage、耗时、质量及外部用量。范围选择、pack/delegate、交接读取、关键证据核验及报告反馈均纳入 B。

事实：本机 Codex CLI 0.160.0，用户配置 gpt-6.1-sol / ultra。OpenAI 官方非交互文档支持 exec --json 的 turn.completed usage；cached 为 input 子集，reasoning 不重复计入 output。本桌面聊天没有按任务提供 token 数据，所以本实验是 CLI 受控对照，不冒充当前聊天或所有日常任务的节省率。

方法：重新执行六对，AB/BA 交替，A 可自然 rg/read，不强制全读；B 不读既有报告、不用 --resume、不省略实际核验。关闭同系子 agent，两臂同配置。首次运行使用新输出目录，失败、重试及备援保留；完整、质量等价的配对按 1−ΣB/ΣA 计算，逐任务都展示。缓存输入、外部用量、等待时间单列，不推算订阅额度或费用。

修改范围：新增本任务文件、独立 benchmark harness（如适合复现则保存到 scripts/benchmarks）、结果说明 docs/TOKEN-BENCHMARK-v0.2.md。运行记录与原始模型结果留在私有本地状态目录，虚构工作树在临时目录；不修改 v0.2 执行代码、安装入口、现有配置或 Keel。

已读：multi-ai AGENTS、技能及 delegation 参考、已有六任务验证 metadata。另一 agent 只读方法审查，强调不能拿旧 null usage 当基线、不能只算交接、不能用关键词命中替代正确性核验。

计量验证：校准 exec 实际返回 input/cached/output/reasoning。六对已重新联网完成，AB/BA 交替，各臂相同配置，无 resume。引用位置、原文及源文件未改通过自动校验；两个调查和两个提取由另一 agent 独立复核，两个 challenge 由主 AI 逐条对照固定事实核验。

结果：六组 A 总 token 212,429，B 1,893,031（8.91倍）；未缓存输入 88,277→228,741（2.59倍）；各臂合计时间250.094→1,673.302秒。4/6委派完成，2次失败后主模型接手。日志 B 核心结果正确但漏第一次503历史，严格rubric记partial；五组完整等价的B/A仍为8.79，三组委派成功且完整等价的B/A为8.59。本轮没有证实省token。

外部：10次适配器启动，不等于实际模型请求数。4次Kimi启动EMFILE、Grok6次中4次完整/2次超时。未提供的usage和provider_requests留null，失败dispatch内联metrics被完整纳入核验汇总；不能把缺少metrics.json当零调用。

新问题：Kimi启动期fs.watch EMFILE，不能证实是登录、FD耗尽或sandbox原因；Grok全局native Read deny误拒search_tool，属于可复现的适配功能缺陷。没有中途修改执行代码或安装版本，保持实验可比较。后续修复必须复测允许发现与禁止file/tool_input_file canary，不可直接删除deny。

产物：[实际结果说明](../TOKEN-BENCHMARK-v0.2.md)、`scripts/benchmarks/delegation_tokens.py`。完整私有汇总 `/Users/xiaodi/.local/state/multi-ai/validation/multi-ai-token-bench-ntv4upnh/verified-pairs.json`，每臂事件、最终答案和失败全部保留。执行脚本SHA-256 `32113c5995fff86f05a7c6c59a5af03f9b1e507e2c6a2d40129ae9ce2b642c07`。

验证：12臂退出0、有真实usage、引文有效、源材料不变；这不代表12次委派成功。脚本AST语法校验通过，文档/改动diff空白检查通过；本轮未修改Rust，不重复运行既有完整Rust测试。未提交、推送、重新打包或安装。

边界：当前桌面聊天、实验编写/协调/评分没有按任务token计量，不冒充账户总用量。新会话初始化成本高，任务大多短小，长记录答案可搜索；真实大型综合任务和熟悉会话复用仍未测量。
