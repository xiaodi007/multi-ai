# v0.2 低成本委派实现

状态：v0.2 实现、本机安装与工具验收完成；原 Kimi 登录恢复及严格宿主 token 收益实验仍未完成。用户已授权实现确认的 v0.2 计划及虚构材料联网验收。

目标：在主模型大量阅读前委派材料加工、Kimi/Grok 受限仓库调查和定向复核；短交接与完整报告共用结构化结果，无额外摘要调用。

事实：multi-ai 工作树初始干净，版本 0.1.0；Keel 有其他任务修改，本轮不改业务仓库。已读 AGENTS、README、兼容/技能安装、API 与输出生命周期代码。

修改范围：新增 packet/broker/delegate/delegate_cli；接入 CLI 与能力诊断，版本及官方 rmcp 依赖；补充确定性测试、技能升级/回退和安装文档。本任务主线负责调度、发布、兼容与集成；并行任务分别拥有快照/broker 和受限 CLI 文件。

门禁：发送前输出锁；明确材料、严格备援；正文后不重发；CLI 工具目录与隔离配置实证；结果结构/证据校验后整组发布；未知实际用量留空。凭据不进入材料/报告/仓库。未经后续要求不提交推送。

实现：pack 离线、显式工作树快照；官方 rmcp 的内部 stdio list/search/read；Kimi/Grok 版本限定的私有工具配置；同步 delegate 的角色、严格备援、共用总预算；结构/源位置/哈希校验；整代产物原子 pointer；精确恢复与报告级反馈；Codex/Claude 共用 skill 和受管理升级/回退。

关键补强：API 材料加入物理行号；CLI list/失败 read/空 search 不能冒充材料读取；发布后缓存写失败不推翻成功报告；故障注入覆盖每一步整代写入；OAuth 原子刷新改目录桥接并共用刷新锁；刷新前后 OAuth 值仅在内存用于日志脱敏。

已验：最终 `cargo test --locked` 92 passed，2 项可选厂商边界检查默认 ignored（两项已单独明确执行通过）；Clippy -D warnings、格式及 diff 空白检查通过；安装 skill shell 回归通过；两家实际 CLI 配合 loopback mock 的工具表/禁止调用通过；真实 Grok 调查和复核、DeepSeek API 提取/复核；修复版 Kimi 连续两次隐藏答案调查及 106,206 字节长材料 extract。完整数字与方法见 [验证记录](../VALIDATION-v0.2.md)。

测量限制：调查/提取/复核各两组已有固定目标、基线答案及报告核验，均记录 agent 反馈。宿主无法提供按任务的实际 token、耗时/成本，因此为正确性对照及委派端测量，不宣称已完成严格 token 收益实验；缺失字段 null，字符压缩独立展示。原始运行在本地状态目录 validation/v02-pairs-7zt8wxgn，不进入源码或发布包。

必须交接的事故：早期 Kimi 单文件认证链接存在原子刷新丢失缺陷；后续原登录 grant 被拒绝并写为空 token，日志不能证明实际刷新因果，不能简单归因为自然过期。缺陷已修复并通过顺序、并发、两次真实请求回归。multi-ai 独立登录已恢复并验证可用；原 ~/.kimi-code 登录恢复等待用户设备授权，900 秒后超时，仍未恢复。原 config 哈希未改。用户需 `KIMI_CODE_HOME="$HOME/.kimi-code" kimi login` 完成新设备授权，此项不能标为完成。

分发验收：macOS arm64 release 构建通过，归档及内部 SHA256SUMS 全部校验通过。发布包为 `dist/multi-ai-0.2.0-aarch64-apple-darwin.tar.gz`，SHA256 `7687aa776383b33ac6795911a6a2534ed964904815e0e108532275bcfdfc4f7e`。归档只含二进制、安装/回退/skill 脚本与模板、说明和校验清单，没有凭据、原始模型日志或客户材料。

隔离升级/回退：先安装 v0.1、保留配置改动，再升级 v0.2 与两家 skill，三个入口及配置哈希正确；先回退 skill 再回退二进制，恢复 v0.1。证据在 `/private/var/folders/6h/20xr10ln1nv1cm7stl_rft640000gn/T/multi-ai-install-v02-p8v3zucj/verification.json`。

真实本机安装：`~/.local/bin/{multi-ai,ai-review,llm-run}` 均指向 `~/.local/lib/multi-ai/0.2.0/multi-ai`，两份现有配置哈希完全不变。Codex `~/.agents/skills/multi-ai` 与 Claude `~/.claude/skills/multi-ai` 均已按已知 v0.1 模板升级，使用新会话发现新说明。记录在 `~/.local/state/multi-ai/validation/v02-install-preflight.json`。

安装后联网验收：使用虚构 router.rs，已安装 release 在空 PATH 下原生 DeepSeek Flash API 请求成功，2,159 ms、718 total tokens；Kimi 调查成功，14,561 ms、2 次 list/read，正确引用 7319 和 handler。完全匹配的 release CLI `--resume` 命中，外部执行和模型请求均 0，报告哈希一致。证据在 `/private/var/folders/6h/20xr10ln1nv1cm7stl_rft640000gn/T/multi-ai-installed-v02-3c75l75h/verification.json`，agent 反馈已记录。

回退入口（先 skill，再程序）：

```sh
sh scripts/rollback-agent-skills.sh /Users/xiaodi/.local/state/multi-ai/backups/agent-skills-20261004T000026Z-Vbf91a
sh scripts/rollback.sh /Users/xiaodi/.local/state/multi-ai/backups/install-20261004T000025Z-cIvtFO
```

遗留：原厂商 Kimi 登录及依赖它的旧 CLI 通路仍需用户重新设备授权；multi-ai 新受限委派使用已验收独立登录，可正常使用。严格收益实验需要宿主提供实际 token/耗时与成本后补测。没有 Keel 改动；本轮没有提交、推送或上传 GitHub v0.2 release。
