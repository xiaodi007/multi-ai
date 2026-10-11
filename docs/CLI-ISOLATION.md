# 委派 CLI 的读取边界

`delegate` 使用独立适配器，旧 `run/review` 的厂商参数保持原义。当前验收 Kimi `2.1.1` 与 Grok `1.0.50 (c58f321264ba)`；其他版本停止受限委派，旧入口仍可使用。`doctor` 的 `delegation.available` 表示可尝试，不代表登录成功或账户还有额度。2026-10-04 的旧版本实测保留为历史证据。

## 如何限制材料

每次委派创建私有 HOME、厂商配置与空工作目录。用户和项目的插件、hooks、skills、MCP、原生 shell/read/write/web 与子 agent 不加载。厂商只接入当前二进制的内部 stdio MCP 服务；宿主无需注册 MCP。材料服务只提供 `list/search/read`，逐次核验快照文件、哈希、路径和预算。

Kimi 自定义主 agent 的工具允许名单只有三个 `mcp__multi_ai__*` 工具，并明确拒绝 `select_tools` 的动态工具发现特例。Grok 的会话最终工具允许名单只有 `search_tool/use_tool`，可发现的 MCP 只有 `multi_ai`。Grok 的 `use_tool` 本身支持从本地文件加载参数，因此受控配置对 native Read 使用 `pattern="**"` 拒绝所有实际文件路径；`file` 和 `tool_input_file` 都不能绕过材料服务。不能省略 pattern 或写成 `"*"`：两者是整类 Read 拒绝，会连无文件路径的 `search_tool` 发现也拦住。

Grok 的账号托管 MCP 与 gateway 工具显式关闭。检测到 `/etc/grok` 或 macOS 的 Grok 系统托管策略时，受限委派标记不可用，因为私有 HOME 无法隔离系统策略；不会绕过该策略或退回原有宽权限通路。

这是厂商工具权限与材料服务的共同约束。厂商 CLI 本身仍需读取认证配置、访问推理服务和保存临时会话；没有宣称它运行在只能读取快照的操作系统沙箱中。工具返回的仓库文本属于证据，不能扩展授权、执行命令或启动另一任务。

实际 Grok 会话工具目录保存到 `cli-tools.json`，出现额外工具或 MCP 会停止执行。Kimi 使用精确版本的工具执行允许名单；其实际请求工具目录通过以下本地推理 mock 验收，不把模型自述当作依据。

## 登录和凭据

只连接已知厂商登录位置，不复制用户配置。Kimi 同时链接 `credentials/` 与 `oauth/`：前者承接 tmp+rename 的原子认证刷新，后者共享厂商的跨进程刷新锁；只配置 managed Kimi Code 的已知凭据键。单文件链接会被原子 rename 替换，不能用于刷新凭据；只链接认证目录而不共享 `oauth/` 也会让并行任务各用各的锁。Grok 使用受验收客户端支持的 `GROK_AUTH_PATH` 指向原认证文件。删除临时运行目录不会删除这两个外部目录，也不会丢掉刷新后的 token。

离线诊断只在内存中验证 Kimi 认证结构，拒绝厂商在 grant 失效后留下的空 token tombstone；不把文件存在等同于登录可用。执行时只在内存使用所选认证文件在刷新前后的 token 值做脱敏，即使厂商错误正文直接回显 token、没有 Authorization 前缀，也不能写入原始日志、工具诊断或公开结果。凭据值不会写入材料、报告、源码或安装包；结束后删除临时配置与会话目录。子进程环境清空，只提供必要的 HOME/PATH、代理、认证位置与受控开关，不继承项目 `.env` 中的其他模型密钥或运行时注入变量。

共享同一认证文件的 multi-ai 受限调用，通过认证文件真实父目录的 `.multi-ai-session.lock` 串行执行，锁从读取旧凭据前持有到刷新后日志脱敏结束。否则第二轮刷新可覆盖第一轮的中间代值，使前后两次快照漏掉日志中的敏感值。此锁独立于厂商的 OAuth 刷新锁，避免死锁；等待计入 delegate 总预算，支持取消，不同认证目录互不串行。该约束覆盖 multi-ai 会话，不宣称原生 CLI 的独立调用遵守这个锁。

程序优先使用 multi-ai 的独立登录；缺少时复用原厂商登录文件。如果厂商无法使用该登录，按实际认证错误失败，没有宽权限重跑。需要独立登录时，在自己的终端执行：

```sh
# 默认状态目录；设置 MULTI_AI_STATE_DIR 后改用该目录。
mkdir -p "$HOME/.local/state/multi-ai/delegate-cli/login/kimi"
chmod 700 "$HOME/.local/state/multi-ai/delegate-cli/login/kimi"
KIMI_CODE_HOME="$HOME/.local/state/multi-ai/delegate-cli/login/kimi" kimi login

mkdir -p "$HOME/.local/state/multi-ai/delegate-cli/login/grok"
chmod 700 "$HOME/.local/state/multi-ai/delegate-cli/login/grok"
GROK_HOME="$HOME/.local/state/multi-ai/delegate-cli/login/grok" grok login
```

独立登录目录位于运行状态区，不是仓库或发布资产。Kimi 默认模型只接受当前验收的 managed Kimi Code 别名；任意自定义 provider 配置不会自动移入新适配器。`kimi-k3-cli` 保持明确 K3 模型，Grok 记录会话初始化和真实 usage 返回的模型身份；未知身份不能用于恢复缓存。

Kimi 普通 `kimi-for-coding` 与 `highspeed` 的隔离配置为 262144 上下文，K3 为 1048576，`k3-256k` 为 262144。仅 K3 家族声明 `support_efforts=["low","high","max"]`，并将调查 effort 设为 low；普通 coding 模型不伪装支持 K3 effort。Kimi 2.1.1 失败时也会 flush assistant 正文，只有最终无工具调用正文、随后有效的 `session.resume_hint` 和进程 exit 0 同时成立才算完成；Grok 仍必须有 success result 与完整正文。

## 验收证据

常规离线测试不要求安装厂商 CLI，不读取用户凭据：

```sh
cargo test --locked --test delegate_cli
```

以下检查需要安装精确版本 CLI，使用私有配置、loopback mock 与虚构 API key，不访问用户认证文件或发送真实外部推理请求：

```sh
cargo test --locked --test delegate_cli installed_kimi -- --ignored --nocapture
cargo test --locked --test delegate_cli installed_grok -- --ignored --nocapture
cargo test --locked --test delegate_cli installed_vendors -- --ignored --nocapture
cargo test --locked --test grok_refresh
```

Kimi 检查真实 API 请求仅携带三个材料工具，并强制返回未经提供的 `Bash` 与 `select_tools` 调用，确认厂商拒绝执行。Grok 检查实际请求只有两个 MCP 元工具，分别强制返回 `bash`、`use_tool.file`、`use_tool.tool_input_file`，确认注册表与 Read 拒绝规则阻断调用，外部 canary 不被读取或改写。正向检查两家实际完成 list/search/read、拒绝材料包外路径并正常收尾；Grok 的 `search_tool` 必须成功，K3 请求实际传入 `thinking.effort=low`。确定性假 CLI 回归覆盖两次顺序和两次外层并发刷新、日志脱敏、共享登录守卫等待超时，未读取用户登录。

2026-10-11 在 macOS arm64 上使用上述精确新版实际完成三项厂商 loopback 检查；共享认证和配置生成回归也已执行。Kimi 源码依据固定在 2.1.1 commit `f67e6398fb3210ad8ace970e2dfd5bcc984ed61f`，其 native 协议将 effort 编码为 `thinking.effort`。Grok 1.0.50 的公开源码未能按其 build SHA 获取，配置主分支只作为线索，实际二进制的正反向测试才是本轮版本门禁的验收依据。本轮没有发送真实外部模型请求，不宣称登录、额度或推理服务健康已验证。

2026-10-04 在 macOS arm64 上完成以上检查，并分别使用真实 Kimi/Grok 调查虚构两行源码：任务提示不包含答案，模型通过材料服务找到端口 `7319`，输出的文件哈希与行号通过主程序校验。Kimi 用 2 次材料调用，Grok用 4 次。真实 CLI 用量缺失时保留 null；没有把一次委派当成一次厂商模型请求，也没有据此宣称主模型节省率。

同日后续验收发现 Kimi 原登录 grant 被拒绝并由厂商写成空 token tombstone。早期适配器曾用单文件认证链接，可能丢失轮换后的 grant；原始日志无法确定那次是否实际刷新，不能归因于自然过期。已改为认证目录与刷新锁目录共同桥接，并补充两次顺序、两次并发的 dummy 原子刷新回归：清理私有目录后四次更新均保留，锁互斥且正常释放。完成独立设备登录后，修复版连续两次真实受限调查成功（10.4 秒与 13.9 秒），均返回有效文件哈希和行号；不以早期单文件桥接的成功冒烟代替修复版验收。

本次本机验收的独立 managed 登录已验证可用；用户原 Kimi home 的恢复仍待完成。已通过官方 `kimi login` 启动原 home 的恢复，但第二次设备授权未完成，CLI 在本地 900 秒等待后超时退出；原配置保持不变，不能称为原登录已恢复。用户可在浏览器可操作时运行 `KIMI_CODE_HOME="$HOME/.kimi-code" kimi login`，完成一次新的官方设备授权；不要复制独立登录的轮换 grant 到另一目录。

取消和超时终止厂商进程组；父进程被 SIGKILL 时仅承诺既有输出锁可释放，不能保证回收所有孤儿进程。

实现依据：[Kimi agent 工具允许名单](https://moonshotai.github.io/kimi-code/en/customization/agents)、[Kimi MCP 配置](https://moonshotai.github.io/kimi-code/en/customization/mcp)、[Kimi 原子认证存储](https://github.com/MoonshotAI/kimi-code/blob/main/packages/oauth/src/storage.ts)、[Kimi 跨进程刷新锁](https://github.com/MoonshotAI/kimi-code/blob/main/packages/oauth/src/oauth-manager.ts)、[Grok 配置与兼容开关](https://docs.x.ai/build/settings/reference)、[Grok use_tool 文件输入](https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-tools/src/implementations/use_tool/mod.rs)。
