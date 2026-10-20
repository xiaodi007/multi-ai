# Codex 任务书：`delegate implement` 放开本机网络，让实施方能跑真库与 Playwright（2026-10-19）

先读 `AGENTS.md`、`docs/tasks/2026-10-18-implement-sandbox-fix.md` 的交接。只改本仓库；不提交、不推送。

## 背景（宿主 10-19 真机实测，codex-cli 0.163.0）

keel 的实施项要跑真 PG 集成用例和 Playwright e2e。宿主一直以为「Codex 沙箱连不上 PG」是死限制，10-19 实测：

| 参数 | 连 127.0.0.1:55441（宿主起好的 PG） | listen＋fetch 本机端口 | Playwright chromium 启动 |
|---|---|---|---|
| `-s workspace-write`（multi-ai 现状） | EPERM | 未测 | 失败（Target closed） |
| 加 `-c sandbox_workspace_write.network_access=true` | 通 | 通 | 默认参数失败；加 `--single-process --no-zygote --disable-gpu` 通 |
| `-s danger-full-access` | 通 | 通 | 通 |

沙箱内 `CODEX_SANDBOX=seatbelt`，被测仓库可以据此给 chromium 加启动参数（那是 keel 的事，不归本仓）。Docker socket 仍不可用：数据库、对象存储由宿主先起好，实施方只连。

## 要做

1. `delegate implement codex-cli` 加显式开关 `--network`：带上时追加 `-c sandbox_workspace_write.network_access=true`，不带保持现状。不提供 `danger-full-access` 选项。
2. status 记录 `network: true|false`；README「受控实施」一节写明：开了之后 codex 执行的命令能访问任意网络地址（包括远端），事后的 Git/mtime 核对看不到网络写入；建议宿主只在任务书里给本机私有库的连接串。
3. 单测：假 codex 下验证带与不带 `--network` 的参数。
4. 跑本仓库验证命令，`sh scripts/package.sh` 重新打包；宿主随后安装并真机试跑。

## 交接（最后一条消息）

改了哪些文件；验证结果数字；打包产物路径。

## 补充（宿主派活时加）

- 工作树里有 10-18 那轮未提交的改动，全部保留，在其上改。
- 本轮 codex 以 `network_access=true` 启动，`cargo test --locked` 里绑 127.0.0.1 的用例应该能跑；请跑全量（状态目录用临时 `MULTI_AI_STATE_DIR`），交接写通过/失败数字，失败要列名字和原因。

## 本轮交接（2026-10-19）

已完成 `delegate implement codex-cli --network`：显式开启时追加 `-c sandbox_workspace_write.network_access=true`；省略时保持现有参数，不覆盖 Codex 默认网络配置。status 在成功和失败时均记录 `network: true|false`，初始 attempt 元数据也记录该值。不提供 `danger-full-access`，并拒绝带值的 `--network=false`，防止被误当作开启。

本轮修改 9 个文件，保留开始时全部未提交修改：

- `src/args.rs`、`src/cli.rs`：解析、校验显式开关并更新帮助。
- `src/implement.rs`：按开关构造固定参数，记录 network。
- `src/implement_tests.rs`：更新受控参数构造函数调用。
- `tests/implement.rs`：假 Codex 验证带/不带开关的 argv 和 status；验证 dirty 失败也记录 network，以及拒绝带值写法。
- `README.md`、`docs/COMPATIBILITY.md`、`integration/skills/multi-ai/SKILL.md`：同步网络能力和边界；开启后可访问任意网络地址（包括远端），Git/mtime 核对看不到网络写入，建议任务书只给本机私有库连接串，服务由宿主预先启动。
- 本任务卡：追加交接。

验证结果：

- `cargo fmt --all -- --check`：退出 0。
- 临时 `MULTI_AI_STATE_DIR` 下 `cargo test --locked --no-fail-fast`：156 通过、2 失败、4 忽略，退出 101；所有测试目标均已运行。本机 TCP 用例已能运行，api_native 21/21、delegation 7/7、lifecycle 8/8 通过；实施集成测试 12/12 通过。
- 首次失败均在既有 `tests/delegate_cli.rs`：`oauth_values_echoed_without_bearer_prefix_are_redacted` 在第 665 行因找不到 `stderr.raw` 而 unwrap 失败；`kimi_atomic_refresh_survives_cleanup_and_shares_concurrent_refresh_lock` 在第 741 行断言失败，实际返回 `cannot determine vendor CLI version; explicit fallback list exhausted`。随后用临时状态目录执行 `cargo test --locked --test delegate_cli -- --test-threads=1`，8 通过、0 失败、3 忽略，退出 0；两项失败均未复现。首次失败根因尚未确认，未将全量首次结果记作全绿，也未修改 Kimi 委派实现。
- `cargo clippy --locked --all-targets -- -D warnings`：退出 0，0 警告。
- `sh tests/test-agent-skills.sh`：退出 0。
- `git diff --check`：退出 0。
- `sh scripts/package.sh`：退出 0，macOS arm64 release 构建成功，包内 19/19 校验通过；归档 SHA-256 校验通过。已核对包内二进制帮助包含 `--network`。

验证日志目录：`/private/tmp/multi-ai-network-validation.Gs7cgP/`（`cargo-test.log`、`delegate-cli-recheck.log`）；打包日志：`/private/tmp/multi-ai-network-package.log`。

打包产物：

- 目录：`dist/multi-ai-0.2.0-aarch64-apple-darwin/`
- 归档：`dist/multi-ai-0.2.0-aarch64-apple-darwin.tar.gz`
- 归档校验：`dist/multi-ai-0.2.0-aarch64-apple-darwin.tar.gz.sha256`

宿主随后自行安装并真机验证 PG/Playwright；本轮未安装、未运行真实外部模型、未提交、未推送。
