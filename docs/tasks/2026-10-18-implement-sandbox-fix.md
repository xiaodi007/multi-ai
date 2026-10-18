# Codex 任务书：`delegate implement` 在真机上起不来（2026-10-18）

先读 `AGENTS.md`、`docs/tasks/2026-10-18-codex-implement-brief.md` 的 F 项与交接。只改本仓库；不提交、不推送。

## 现象（宿主真机复现，两次）

```
multi-ai delegate implement codex-cli --cwd <干净的 git 工作树> --prompt-file <任务书> --out <仓外摘要> --timeout 5400 --path-prepend ~/.nvm/versions/node/v24.19.0/bin
[codex-cli] error: Error: failed to initialize in-process app-server client: Operation not permitted (os error 1)
```

摘要文件没有产出，几秒内退出。

## 原因（宿主判断，请核实）

`src/implement.rs` 用 `/usr/bin/sandbox-exec` 把整个 `codex` 进程包进一层 seatbelt（`sandbox.sb`），再让 codex 自己 `--sandbox workspace-write`。问题：

1. macOS 不允许嵌套 seatbelt：codex 自己调用 sandbox 时被外层拒绝。
2. 外层 profile 只放行 cwd 与 git 目录：codex 写不了它自己的状态目录（`~/.codex`，会话、日志、app-server socket），也连不上模型服务。外层拦住的是 codex 本身，而不只是它执行的命令。

宿主一直用 `codex exec -C <工作树> -s workspace-write --skip-git-repo-check -o <摘要>` 跑，没有外层沙箱，十几次都正常，写入也都在工作树内。

## 要做

1. 去掉外层 `sandbox-exec`。写边界改由 codex 自带的 `workspace-write` 负责，实际参数与宿主那条命令等价（保留 `-o`、`-C`、`--skip-git-repo-check`）。不要再给 codex 传 `network_access=false`：那一项只管 codex 执行的 shell 命令，跟模型调用无关，但宿主的任务需要 pnpm 离线可用即可，保持 codex 默认。
2. 保留并加强事后核对：运行前后记录 HEAD、`git status --porcelain`，以及 cwd 之外几个敏感位置的 mtime 快照（仓库父目录的同级工作树、`~/.claude`、`~/.agents`）。有越界变化就在 status 里标为 `boundary_violation` 并列出路径。不要回滚，只报告。
3. 头注和 README 改掉「外层 macOS sandbox 是强制的」的说法，写明：边界是 codex 自带 workspace-write＋事后核对；对哪一类越界天然失明（例如通过网络写远端）。
4. 单测：用假 codex 可执行文件验证参数、事后核对能报出越界写；去掉只验证 seatbelt 的用例。
5. 跑本仓库四条验证命令，`sh scripts/package.sh` 重新打包。

## 交接（最后一条消息）

- 改了哪些文件；验证结果数字；
- 打包产物路径；宿主随后会自己安装并做真机试跑。


## 本轮交接（2026-10-18）

已移除整个 Codex 进程外的 `sandbox-exec`、`sandbox.sb` 和测试专用绕过分支，生产与假 Codex 共用直接执行入口。实际固定参数为 `codex exec -C <canonical cwd> -s workspace-write --skip-git-repo-check -o <暂存摘要>`；保留共享解析器取得的显式模型与子进程 PATH 前置，不再注入网络或其他 sandbox 配置。

原因核对：旧 profile 的 `deny file-write*` 会限制 Codex 自身状态写入，且整个进程被外层 Seatbelt 包装，存在嵌套 sandbox 风险；原 profile 使用 `allow default`，没有显式禁止网络，因此不能据此认定它直接阻断了模型联网。本轮未调用真实模型，真机 app-server 启动由宿主安装后验证。

运行前后保存 HEAD、`git status --porcelain=v1` 和 Git 文件身份；另保存同级 Git 工作树、`~/.claude`、`~/.agents` 的递归纳秒 mtime/类型/长度快照。前后根集合取并集，能发现新增或消失的监测目录；内部符号链接仅记录链接元数据，敏感根本身为链接时也检查解析目标。本次受管理产物目录单独记录并排除，避免摘要位于同级工作树时误报工具自身写入。扫描失败的未知条目不冒充已删除文件，核对不完整不发布成功摘要。

成功、失败、取消、超时后均执行核对；Git 后检失败也不跳过外部路径核对。观察到越界变化时，status 写 `boundary_violation` 和 `boundary_violations` 绝对路径列表，保留原执行状态、原因和 quota 冷却，只报告、不回滚、不替换上一份完整摘要。README 与技能模板已说明网络远端、未监测位置、快照间消失或恢复 mtime 的写入、内部 symlink 目标等盲区，以及无法仅凭差异归因并发进程。

本轮修改 7 个文件（保留所有既有未提交修改）：

- `src/implement.rs`：启动参数与 Git/mtime 前后核对。
- `src/implement_tests.rs`：去除 Seatbelt 专用用例和 FakeRunner，补充扫描错误与内部 symlink 测试。
- `tests/implement.rs`：公共 CLI 的假 Codex 参数、正常实施及越界写回归。
- `README.md`、`docs/COMPATIBILITY.md`、`integration/skills/multi-ai/SKILL.md`：同步边界与使用说明。
- 本任务卡：保存交接结果。

验证结果：

- `cargo fmt --all -- --check`：退出 0。
- `cargo test --locked`：首次在默认用户状态目录不可写的既有锁测试处停止，35 通过、1 失败。随后用临时 `MULTI_AI_STATE_DIR` 执行 `cargo test --locked --no-fail-fast`：112 通过、35 失败、4 忽略；35 个失败全部是本地 `TcpListener::bind("127.0.0.1:0")` 被当前沙箱以 `Operation not permitted` 拒绝（api_native 21、delegation 6、lifecycle 8）。全量尚未全绿。
- 实施相关：16 个库单测、10 个公共 CLI 集成测试全部通过，共 26/26；包括参数、递归同长改写/新增/删除、首次创建敏感目录、新建 sibling、失败/quota/Git 损坏后的越界核对、旧摘要保留与不回滚，以及摘要位于 sibling 的自有工件排除。
- `cargo clippy --locked --all-targets -- -D warnings`：退出 0，0 警告。
- `sh tests/test-agent-skills.sh`：退出 0。
- `git diff --check`：退出 0。
- `sh scripts/package.sh`：退出 0；macOS arm64 release 构建成功，包内 19/19 校验通过，归档校验通过。

打包产物：

- 目录：`dist/multi-ai-0.2.0-aarch64-apple-darwin/`
- 归档：`dist/multi-ai-0.2.0-aarch64-apple-darwin.tar.gz`
- 归档校验：`dist/multi-ai-0.2.0-aarch64-apple-darwin.tar.gz.sha256`

未安装、未修改仓外技能副本；宿主随后自行安装并真机试跑。真实模型调用 0 次，未提交、未推送。
