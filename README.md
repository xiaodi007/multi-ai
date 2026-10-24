# multi-ai

源码 v0.2 新增低成本委派：`pack` 离线材料快照，`delegate investigate|extract|challenge TARGET` 执行 Kimi/Grok 受限调查或 API 材料加工，返回短交接与可核验完整报告。使用前查看 `doctor` 的委派能力；完整流程、边界和指标见 [DELEGATION.md](docs/DELEGATION.md)。下方 GitHub 下载示例仍指已发布 v0.1.0；本轮 v0.2 可从源码构建或使用本地生成的发布包。

Rust 原生多 AI 工具，统一使用 `multi-ai`；`ai-review` 与 `llm-run` 二进制别名已弃用，暂保留旧调用语义。首版验收 macOS Apple Silicon；API 请求、调度、报告、输出锁和反馈均在同一程序中执行。安装后的工具本体无需 Python、Node、uv 或 Rust 编译器。

厂商 CLI 是可选依赖：使用 Codex、Grok、Antigravity、Kimi Code 或 Claude Code 通路时，仍需安装对应 CLI 并登录。纯 API 通路不依赖这些 CLI。

## 安装与回退

### 从 GitHub Release 安装

源码与发布入口为 [xiaodi007/multi-ai](https://github.com/xiaodi007/multi-ai)。当前仓库为私有仓库，下载需要有访问权限的 GitHub 账号。下例使用 GitHub CLI 下载固定版本；下载与安装所需的 `gh`、`tar` 不属于工具运行依赖。

```sh
gh auth login
gh release download v0.1.0 --repo xiaodi007/multi-ai \
  --pattern 'multi-ai-0.1.0-aarch64-apple-darwin.tar.gz' \
  --pattern 'multi-ai-0.1.0-aarch64-apple-darwin.tar.gz.sha256'
shasum -a 256 -c multi-ai-0.1.0-aarch64-apple-darwin.tar.gz.sha256
tar -xzf multi-ai-0.1.0-aarch64-apple-darwin.tar.gz
cd multi-ai-0.1.0-aarch64-apple-darwin
```

在解压后的 v0.1.0 归档目录执行原归档命令（它不含本轮新增开关；本轮功能需从源码构建 v0.2）：

```sh
shasum -a 256 -c SHA256SUMS
./install.sh
multi-ai --version
multi-ai routes
multi-ai doctor
# 让 Codex 与 Claude Code 发现独立的 multi-ai skill
./install-skills.sh
```

安装到 `~/.local/lib/multi-ai/<版本>/multi-ai`。本轮源码包新安装推荐 `--no-legacy-aliases`，只管理 `~/.local/bin/multi-ai`；此开关不会删除已有旧别名。省略开关仍创建两个弃用别名以兼容现有调用，分派逻辑继续保留。若该目录尚未在 PATH 中，请在自己的 shell 配置中加入它；安装脚本不修改 shell 启动文件。

首次安装导入当前 `~/.claude/skills/multi-llm-pipeline/references/model-profiles.json`，并创建内置 `routes.json`。已有 `~/.config/multi-ai/profiles.json` 和 `routes.json` 原样保留；不会复制 `.env` 或凭据。

安装前自动创建私有备份，成功后输出 `BACKUP_DIR=…`。按该输出回退：

```sh
./rollback.sh /完整路径/到/BACKUP_DIR
```

回退恢复原入口的文件或符号链接及权限。原有配置从未被覆盖；本轮新建配置仅在内容哈希仍匹配时删除，之后修改的配置与入口会保留并提示。旧源码、凭据、业务目录和运行历史均保留。安装脚本只管理上述入口、版本二进制及新配置；另行更新的 skills 和启动脚本需要使用各自的备份回退。

发布包提供新用户 skill 安装以及已有 skill 迁移两种入口：

```sh
./install-skills.sh
./rollback-agent-skills.sh /该脚本输出的/BACKUP_DIR
```

新 skill 默认安装到 Codex 的 `~/.agents/skills/multi-ai` 和 Claude Code 的 `~/.claude/skills/multi-ai`；已有同名目录完整保留。Codex 用 `$multi-ai`，Claude Code 用 `/multi-ai`，也可按描述自动选择。旧 Codex 的 `~/.codex/skills` 入口可用 `--codex-dir` 显式选择；发现、权限、旧入口兼容及隔离验证见 [AGENT-INTEGRATION.md](docs/AGENT-INTEGRATION.md)。安装不会让本机凭据同步到云端会话。

已有独立 multi-ai skill 可升级受管理文件：

```sh
./install-skills.sh --upgrade
./rollback-agent-skills.sh /该脚本输出的/BACKUP_DIR
```

升级保留用户修改并提供合并指南。`update-skills.sh` 仅作为该升级命令的兼容转发，不再安装或更新 `ai-review`、`multi-llm-pipeline` 技能；发布包也不再携带旧技能模板。机器上已有旧技能、命令和权限项按调用点清单交由宿主处理，不自动改写其他仓库。历史旧技能备份仍可用 `rollback-skills.sh` 恢复。

安装／升级脚本支持 `--home DIR` 和 `--templates DIR` 供隔离验证，程序配置与凭据不随 skill 安装复制。

### 从源码开发与构建

开发者可克隆仓库，运行测试并生成本地发布包：

```sh
gh repo clone xiaodi007/multi-ai
cd multi-ai
cargo test --locked
./scripts/package.sh
cd dist/multi-ai-0.2.0-aarch64-apple-darwin
./install.sh --no-legacy-aliases
./install-skills.sh --upgrade
```

构建需要 Rust/Cargo 和 macOS 开发工具；运行发布包不需要它们。产物位于 `dist`，包含二进制、安装／回退、skill 安装和升级脚本、multi-ai skill 模板、文档、`SHA256SUMS`、tar.gz 和归档校验和。构建后按上文在 `dist/multi-ai-<版本>-aarch64-apple-darwin` 中安装。打包不上传、不提交；生成的构建目录和发布包不进入源码 Git，通过 Releases 分发。

## 使用

```sh
# 离线查看真实配置身份；CLI 默认模型无法确定时显示 unknown
multi-ai routes --json
multi-ai doctor

# API 只接收明确附带的材料
multi-ai text glm --input evidence.md --out answer.md
multi-ai text run --task review --input evidence.md --out answer.md

# 厂商 CLI 通路可以使用自己已有的工具能力
multi-ai run kimi-k3-cli --prompt-file evidence.md --cwd /path/to/repo --out review.md

# 并行评审及备援；API 通路需要附带正文
multi-ai review --input evidence.md --text-ready --outdir reviews --seats 2
multi-ai verify --findings review.md --cwd /path/to/repo --out verify.md

# 批量任务与历史统计
multi-ai batch --jobs jobs.json --outdir results --resume
multi-ai stats
```

`run/review/verify` 的超时单位是**秒**；`text` 和旧 `llm-run` 的 `--timeout-ms`、`--headers-timeout-ms`、`--idle-timeout-ms` 单位是**毫秒**。文本接口未指定 `--out` 时 stdout 输出正文或 JSON；指定后相应格式写入文件，stdout 打印 `Wrote <path>`，`--quiet` 仅控制调度提示。`--input` 可重复，`--input -` 接收 stdin；图片支持 PNG、JPEG、WebP，每张最多 5 MiB。任务、领域和圆桌配置沿用原 profiles：

```sh
multi-ai text aliases
multi-ai text profiles
multi-ai text domains
multi-ai text dispatch --domain product --kind debate --input evidence.md --out discussion.md
multi-ai text run --task vision-review --image screenshot.png --out vision.md
```

`doctor` 默认只检查本地配置、凭据是否配置及 CLI 是否可执行，不验证登录或账户健康；本机 Kimi/Grok 版本与委派已验收版本不一致时，在 stderr 与 `warnings` 中提示。`doctor --live`、`probe` 和 `text list-models` 才联网；模型探测会使用额度。`doctor --models` 联网拉取各厂商模型列表（不调用模型），配置会发送的模型名已不在列表时退出码为 1，并列出接口有但配置未引用的模型和 Kimi Code CLI 未被通路使用的托管模型。`doctor --updates` 列出 claude/codex/kimi/grok/agy 的本机与最新版本，Kimi/Grok 有新版时提示升级后需重新验收委派。`review --dry-run` 保留旧版的额度采集行为，需要纯离线检查时使用 `routes` 或 `doctor`。

## 通路、配置与凭据

| 新通路 | 旧 ai-review 名称 | 额度池 | 模型来源 |
|---|---|---|---|
| `agy-cli` | `agy` | `google` | 固定 `gemini-3.8-flash-high` |
| `grok-cli` | `grok` | `xai` | CLI 默认，未知 |
| `codex-cli` | `codex` | `openai` | 共用配置解析；取不到保持 unknown |
| `glm-api` | `glm` | `zhipu` | profiles 别名 `glm` |
| `deepseek-pro-api` | `deepseek` | `deepseek-api` | profiles 别名 `deepseek-pro` |
| `claude-deepseek-cli` | `claude-ds` | `deepseek-api` | Claude Code 壳配置 |
| `claude-*-ro-cli` | `claude-*-ro` | 对应厂商池 | `anthropic_provider` 解析：`anthropicModels.main/small`，缺省取 `models.pro/fast` |
| `kimi-cli-default` | `kimi` | `kimi-code` | CLI 默认，未知 |
| `kimi-k3-cli` | `kimi-k3` | `kimi-code` | 固定 `kimi-code/k3` |
| `kimi-k3-api` | `kimi-k3-api` | `moonshot-api` | profiles 别名 `kimi-k3` |

通路带 `anthropic_provider` 时，以 Claude Code 为外壳调用该厂商 profiles 的 `anthropicBaseUrl`：子进程环境清空后只注入该厂商 key（`ANTHROPIC_AUTH_TOKEN`）、地址与模型名，去掉宿主 `ANTHROPIC_*`／`CLAUDE*` 变量和其他厂商 key；`CLAUDE_CONFIG_DIR` 指向状态区的私有目录，跑完删除，用户 settings、hooks、插件与全局 CLAUDE.md 不加载。未配置 key 时不启动。权限由通路 argv 决定，示例只读通路用 `--restricted --strict-mcp-config --tools Read,Grep,Glob`。

`delegate implement` 也接受带 `anthropic_provider` 的通路（如 `delegate implement claude-kimi-ro-cli --cwd 工作树 ...`）：通路配置的 argv 被替换为受控实施参数——`--restricted`（文件工具限于 cwd、拒绝 bypassPermissions）、`--tools Read,Grep,Glob,Edit,Write,Bash`、`acceptEdits`、Claude Code 沙箱（Bash 只在系统沙箱内、不允许出沙箱、默认不联网）以及禁止写 `<cwd>/.git/**`。前后快照、HEAD 变化检测与不自动提交沿用 Codex 实施的检查；`--network` 只支持 `codex-cli`。2026-10-24 用 DeepSeek 实测：cwd 内改文件与跑脚本可行，写 HOME、Write 工具越界、`git commit`、外网均被拒；沙箱放行临时目录（与 Codex workspace-write 相同），需要时用 `--watch` 让快照报告。

保留原有 rank、订阅／计费分类、额度冷却和按额度池去重规则。多个报告、模型家族及额度池分别统计；两个额度池不能证明两份意见认知独立。新 `run` 使用明确通路名，旧名称仍在旧入口中保留原义。

配置默认内置于程序，用户文件位于 `~/.config/multi-ai`：

- `routes.json` 管理 CLI 参数、额度池、家族、能力、排序和超时；API 通路引用别名。
- `profiles.json` 管理 API provider、模型、环境覆盖、别名、任务、领域、圆桌及提示词。
- `MULTI_AI_CONFIG_DIR` 可以指定配置目录；`MULTI_AI_STATE_DIR` 可以指定运行状态目录。

环境配置按“非空进程变量 → 当前目录 `.env` → 新配置目录 `.env` → 旧 `~/.config/multi-llm-pipeline/.env`”填补缺失值。凭据变量沿用 `DEEPSEEK_API_KEY`、`KIMI_API_KEY`／`MOONSHOT_API_KEY`、`ZHIPU_API_KEY`／`GLM_API_KEY`；日志和公开通路信息不输出密钥。

只有按角色选模型时才应用相应模型环境变量；显式 pin 保留原义。例如 `glm` 会遵循 `GLM_MODEL`，`kimi-k3` 会遵循 `KIMI_K3_MODEL`，`deepseek-pro` 显式使用 `deepseek-v4-pro`。`routes` 展示与请求共用同一个解析器。

## 报告、恢复与反馈

同一输出及重叠目录由顶层文件锁保护，冲突立即失败，不启动模型、不修改另一轮状态。顺序复用输出会保存旧报告与运行元数据；失败不会替换上一份完整报告。原始响应、历史和运行 manifest 保存在私有状态目录 `~/.local/state/multi-ai`，未识别文件不会清理。

`--resume` 只复用身份与正文哈希均匹配的完整结果；变更输入、源码、模型、配置或实现版本、损坏缓存及 partial 都会重新执行。非 Git 工作区恢复需显式 `--source-id`；其值应随外部材料或上下文变化。Python／Node 旧缓存保留，但 Rust 升级首次恢复会失配，可能重新请求。CLI 缓存也绑定客户端可执行、相关脚本及已知模型配置；默认模型身份无法可靠确定时不缓存，`--resume` 会说明并重新请求。取消及超时会终止厂商 CLI 进程组；父进程被 SIGKILL 时，只承诺文件锁自动释放，不保证杀掉所有孤儿后代。

主 AI 在实际验收后按报告代录反馈：

```sh
multi-ai feedback --run RUN-UUID --report ATTEMPT-ID \
  --accepted 2 --false-positive 1 --duplicate 0 --uncertain 1 \
  --evidence '依据实际测试结果及需求逐项核验；证据位置：…'
```

至少提供一个发现数量或实施字段及证据；不知道的数量或筛选／返工时间留空。反馈绑定报告哈希并追加保存，更正需录入新的事件，原记录保留。默认 `source=agent`；只有用户明确决定，才可加 `--source human --human-confirmed` 并记录其明确意见。模型的 `CONFIRMED` 和测试通过都不自动代表人工采纳。`scorecard` 汇总实际反馈；`recommend` 提供显式建议，普通 run/review 保留既有选路行为，batch 仅在 `route: "auto"` 时应用建议。

完整兼容语义和边界见 [COMPATIBILITY.md](docs/COMPATIBILITY.md)。

## 批量调度与本地统计

`batch` 读取 JSON 清单，路径相对清单所在目录。任务 id 只用字母、数字、下划线和短横线且不可重复；每项生成独立的 `<id>.md`、`<id>.md.status.json` 与缓存。所有输出先取整目录锁，冲突在模型调用前失败。

```json
{
  "pool_limits": {"xai": 2, "kimi-code": 1},
  "jobs": [
    {
      "id": "review-one",
      "prompt_file": "brief.md",
      "cwd": "../worktree",
      "route": "kimi-k3-cli",
      "fallback": ["grok-cli"]
    }
  ]
}
```

```sh
multi-ai batch --jobs jobs.json --outdir results --timeout 1800 \
  --pool-limit xai=2 --rate-limit-retries 2 --retry-delay-ms 500
multi-ai batch --jobs jobs.json --outdir results --timeout 1800 --resume --preflight
multi-ai run kimi-k3-cli --fallback grok-cli --prompt-file brief.md --out answer.md
multi-ai stats
multi-ai stats --json
```

每个额度池默认并发 2，清单或重复的 `--pool-limit POOL=N` 可覆盖。首选与备选均须明确列入清单，API 任务用 `text_ready: true` 声明 prompt 已含全部材料。quota 立即切到备选，池冷却期间后续任务跳过该池；5 小时用量错误在未提供重置时刻时保守冷却 5 小时。rate-limit（包括 429 code 1302）先按指数退避重试，CLI 默认额外重试 2 次、初始延迟 500 毫秒；原生 API 使用其已有重试机制，初始延迟 1000 毫秒，`--rate-limit-retries` 可覆盖次数，`--retry-delay-ms` 仅控制 CLI。重试、退避和备援共用每项的秒单位 `--timeout`；排队等待首个额度池不消耗预算。其他失败保留独立状态，退出码非零不妨碍其余任务完成。取消整个批次会停止排队任务并清理正在运行的 CLI 进程组，不再启动备援或新任务。

`--preflight` 在任何任务开跑前，对清单里出现的每条通路（含备选）各发一次极短探针（单独记为 `preflight` 运行，`--preflight-timeout` 默认 90 秒）。探针结果为 `model`、`auth`、`quota`、`cooldown`、`capacity`、`error`、`empty` 的通路本批跳过：有备选的任务直接从备选开始，`route_attempts` 首项带 `preflight: true` 记录跳过原因；没有可用通路的任务不发调用，以探针状态结束（`model_calls=0`）。`timeout`、`transport`、`rate_limit` 可能是暂时的，不剔除。长任务批次、无人值守或刚换模型 id 后建议打开。

`--resume` 继续使用完整身份和正文哈希校验；成功备选也能直接恢复，未知模型身份仍重新执行。非 Git 任务用清单的 `source_id` 或全局 `--source-id` 标识真实材料版本。修改输入、模型、代码或策略身份后会重新请求，不保证“只看 id 跳过”。失败不替换上一份完整正文，须结合本轮 status 阅读。

### 失败分类与冷却

CLI 分类只使用诊断信道：Codex 的 stdout 只取 `error.message` 和 `turn.failed.error.message`，加上 stderr；`item.*`（包括工具输出和模型正文）不参与。Grok/Kimi 等结构化流只取错误字段；无法识别的输出保留兼容回退，但先剔除工具结果块。API 继续使用响应错误体及 HTTP 状态。

| 分类 | 诊断示例 | 处理 |
|---|---|---|
| `quota` | quota exhausted、usage limit | 额度池冷却，按原规则换备选；5 小时用量错误默认冷却 5 小时；余额不足、账户停用、请充值等欠费诊断记 `billing: true`，冷却 7 天直到充值后 `reset`，`status` 显示「欠费停用」 |
| `rate_limit` | 429、1302、too many requests | 预算内指数退避重试，不写池冷却 |
| `capacity` | at capacity、overloaded、server is busy、503、try a different model、temporarily unavailable | CLI 复用限流退避；run/batch 重试耗尽后可使用显式备选（含同池），不写池冷却 |
| `model` | `unrecognized_model`、`model_not_found`、model … does not exist、模型不存在 | 通路配置的模型 id 被拒；不写池冷却，run/batch 直接换显式备选；改通路模型 id 或 `doctor --models` 核对后再用 |
| `auth` | 独立的 401、403、Unauthorized、Forbidden、authentication | 首次冷却 10 分钟；同池连续第二次及以后冷却 240 分钟；实际非 auth 执行结果或手动 reset 清零连续次数 |
| `transport` | connection reset、network、EOF | 默认冷却 5 分钟 |
| `timeout` / `error` | 超时 / 其它失败（含输入过长） | 独立记录，不触发 auth 冷却 |

`capacity` 在 `stats --json` 的 `failures`、`scorecard --json` 和 `recommend --json` 的 `failure_rates` 中独立计数／展示；历史记录保留原分类。API 的 HTTP 分类和重试规则保持原语义。review/verify 席位调度规则不变。

误冷却或修复登录后，可用已有入口离线解除（不会调用模型）：

```sh
multi-ai status
multi-ai reset openai     # 清指定额度池及连续 auth 次数
multi-ai reset --all      # 清全部额度池冷却
```

结束一个正在跑的运行（只认本机账本里未结束、且进程仍是同一 multi-ai 子命令的记录；子命令收到 SIGTERM 后自行结束厂商进程组）：

```sh
multi-ai stop --list
multi-ai stop 20261024T005326-90838   # 账本编号或其唯一前缀
```

`batch` 结束时的汇总表多一列 `fallback`，回落的任务另起一行写明原通路的状态与错误。

`stats` 只读本地运行记录，不加载 API 配置或凭据、不联网。它按记录中的实际通路汇总真实适配调用的次数、成功率、耗时中位数、P90、失败类型、输入与最终输出字节；缓存命中和未调用适配器的跳过记录单列。中位数对偶数样本取两数平均，P90 取 nearest rank（向上取整第 90% 位）；失败耗时也在样本内。历史记录缺字段会留空并显示有效样本数，不按当前配置猜历史模型。`--state-dir DIR` 可只读其他状态目录；丢失的进度目录会尽量用 manifest 回补，并报告缺失和损坏记录数。统计不证明答案正确或节省 token。

## 受控实施

使用独立的 `delegate implement` 入口，便于将会写代码的委派与只读评审、材料快照调查分别约束。`run codex-cli` 继续按既有只读配置运行。实施只接受 `codex-cli`，且使用固定参数；不会直接执行 routes 中额外的权限或目录参数。

```sh
multi-ai delegate implement codex-cli \
  --cwd /absolute/path/to/clean-worktree \
  --prompt-file /absolute/path/to/brief.md \
  --out /absolute/path/outside-worktree/summary.md \
  --timeout 1800 \
  --path-prepend ~/.nvm/versions/node/v24.19.0/bin
```

`--cwd` 必须是现存 Git 工作树顶层；主工作树和 linked worktree 都支持。默认拒绝任何未提交修改；确需在已有修改上继续时显式加 `--allow-dirty`，已有文件内容会参与前后比较。`--out` 放在工作树之外，避免输出锁与实施目录锁相互冲突。`--path-prepend` 可重复，仅影响子进程 PATH，不修改 shell 配置。`--timeout` 使用秒。

此入口固定调用 `codex exec --json -C <cwd> -s workspace-write --skip-git-repo-check -o <暂存摘要>`，不再套外层 macOS sandbox。默认不额外覆盖 Codex 的网络配置；显式加 `--network` 时追加 `-c sandbox_workspace_write.network_access=true`，status 记录 `network: true`，否则记录 `false`。开启后 Codex 执行的命令能访问任意网络地址（包括远端），事后的 Git/mtime 核对看不到网络写入。建议宿主只在任务书里给本机私有库的连接串；数据库、对象存储由宿主预先启动，实施方连接使用。此入口不提供 `danger-full-access` 选项。写入边界由 Codex 自带的 `workspace-write` 加事后核对负责。仍需在提示中明确授权要读取、修改的材料；写权限限制不等于读取材料范围的自动证明。

运行前后记录 HEAD、`git status --porcelain` 和 Git 可见文件身份。默认只通过 `git worktree list` 扫描所属仓的注册工作树、共享 Git 元数据、`~/.agents` 和 Claude 配置（skills、settings*.json、CLAUDE.md），不遍历父目录所有兄弟仓。可重复 `--watch PATH` 追加范围。保留构建目录、会话目录剪枝和并发受管工作树排除；快照记录叶条目数及 `elapsed_ms`，内部符号链接不递归跟随。

Codex JSONL 的 `item.completed` / `file_change` / `changes[].path` 用于编辑归因：解析后的目标在 cwd 外即为归因越界，包括经符号链接指向外部的目标。快照差异分为 `git_ignored`、`git_internal`（.git 下除 config/hooks）、`nested_worktree`、`sibling_worktree_tracked`、`claude_config`（含 .agents）与 `other`。只有归因越界和沙箱可写到的 Claude 配置变化判 `boundary_violation`，不回滚。受控沙箱（Codex workspace-write、Claude 壳 restricted）只能写 cwd、`/tmp` 与 `TMPDIR`：解析符号链接后不在这些位置的 Claude 配置变化视为宿主并发修改，进 `boundary_warnings.claude_config_outside_sandbox`，不判失败。其余进入 `boundary_warnings`，执行完整成功时继续使用兼容状态 **`status=ok, complete=true, exitCode=0`**。警告表示观测到外部变化，不代表由实施方造成，也不证明没有其它写入。事件流缺失、损坏或未正常结束时标 `attribution=unavailable`，快照继续按上述规则核对。shell 命令事件不提供可靠写入目标，不能从其命令文本猜测归因。

status 每桶保存计数和最多 20 条样例，`boundary_violations` 也最多 20 条；完整差异和归因路径位于 `boundary_details` 指向的 artifacts/boundary-changes.json。完整摘要原子发布并登记为 run/attempt 报告，包括归因越界或 Claude 配置越界时的诊断摘要；此时保留非零退出、`complete=false`。`summary_published` 表示本轮是否发布，`summary_kind=final|diagnostic` 区分正式和诊断摘要。执行失败、超时或空正文继续保留上一份完整摘要。快照看不到网络写入、监测范围外或两次扫描间已恢复的变化。正常收尾的 stderr 最后一行固定为 `multi-ai: status=<status> exit=<code> run=<run_id> attempt=<attempt_id>`。

不自动提交，不支持实施恢复缓存。失败、取消、超时或额度耗尽均记录在 `<summary>.status.json`，失败保留上一份完整摘要；目录和输出冲突均在启动 Codex 前失败。测试不会调用真实外部模型。


## 怎么选模型、怎么给分

派活前先读 [route-traits](integration/skills/multi-ai/references/route-traits.md)，再看本机成绩。`run`、`review`、`batch --task-type X`、`delegate` 接受自由字符串任务类型；batch 每项的 `task_type` 覆盖全局值。建议 `triage`、`review`、`investigate`、`extract`、`implement`、`walkthrough`。任务类型保存在 manifest、metadata、status 和 stats 的 `task_types` 分组中；`delegate implement` 省略时默认 `implement`；其他旧记录缺字段为 null，不猜类型。`stats --task-type X` 可筛选调用统计。

```sh
multi-ai scorecard --task-type triage --since 2026-10-18 --json
multi-ai recommend --task-type triage --jobs 10 --cwd /path/to/work --only kimi-k3-cli grok-cli
multi-ai run kimi-k3-cli --task-type triage --prompt-file brief.md \
  --cwd /path/to/work --out review.md --canary-file canaries.json
```

`canaries.json` 是数组，或直接放进 batch 每项的 `canaries` 字段：

```json
[
  {"match": "src/check.rs:42", "expect": "真问题"},
  {"match": "冗余.*保护", "expect": "真问题"}
]
```

金丝雀从**宿主已经确认的事实**产生，不要用模型自己的结论当金丝雀；答案不发给模型。金丝雀只能测召回，测不了精度。位置允许路径后缀和行号 ±3；其他 match 按 Rust 正则匹配，有歧义时可显式加 `regex:` 前缀。核对范围是一条表格行或一个空行分隔的发现段落，位置和判定必须同处此范围；带明确判定的 Markdown 标题会承接其后的空行、子标题和证据列表；建议输出 `位置 | 判定 | 证据` 的表格。真问题识别 `真问题/属实/确认/CONFIRMED/accepted` 等标签，`误报/待定/REFUTED/UNCLEAR` 等优先否定；只提位置、没有明确判定不计命中。其他 expect 要在同范围出现原文。它是文本核对，不是语义裁判，宿主仍须抽查格式和证据。报告完成后 status 写 `canary_hits/canary_total/canary_results`，并追加绑定报告哈希的 `source=canary` 反馈；失败或 partial 不自动记成功。

batch 保留 stdout JSON，同时在 stderr 打印 `id、route、status、seconds、run_id、attempt_id、canary` 表。用 TSV 一次录入宿主核验结果：

```sh
multi-ai feedback --batch results --from verdicts.tsv
```

TSV 首行为 `id<Tab>accepted<Tab>false_positive<Tab>uncertain<Tab>evidence`，每项一行；未知数量留空，至少一个数量或实施字段已知，evidence 必填。可再加 `screen_min`、`rework_min`（分钟）、`outcome`、`rework_items`、`rework_rounds`、`defect_tags` 列；实施反馈可以把发现数量列留空。默认 source=agent；human 仍需用户明确决定和 `--source human --human-confirmed`。全部行预检通过后才追加；无当前完整报告、正文被修改或不存在的 id 会拒绝整次导入；已登记的完整越界诊断摘要也可录入。更正继续追加，成绩单对同份报告每个字段取最后一个已知值，不把更正重复相加。缓存上的反馈归回原始调用，避免恢复次数虚增成绩。

`scorecard [--task-type X] [--since YYYY-MM-DD] [--json]` 只读本地历史，不加载 API 配置或凭据。按「通路 × 任务类型」输出：

| 指标 | 口径 |
|---|---|
| 样本数、失败率 | 真实适配调用；缓存、冷却跳过、未启动和进行中不计。失败细分 quota、rate_limit、capacity、timeout；失败也计耗时。 |
| 精度 | accepted / (accepted + false_positive)；只用两个字段均已知的报告；重复和待定不进分母。 |
| 金丝雀召回、漏报率 | 命中 / 已知答案总数；漏报率 = 1 − 召回。缓存不重复累计。 |
| 中位数、P90 | 单位秒；偶数中位取平均，P90 用 nearest rank。JSON 比率为 0～1。 |
| 外部秒数 / 采纳发现 | 对有耗时且采纳数已知的完整报告，加上有耗时的失败调用，除以配对样本的采纳数。 |
| 宿主核验分钟 / 采纳发现 | 对 screen-min 和采纳数均已知的报告配对计算；不猜未知时间。 |

JSON 同时给出各项样本覆盖、数量已知样本数、通路版本指纹及损坏记录数。缺失指标为 null，零采纳时每条采纳成本也为 null。调用少于 5 一律标「样本不足」；给优劣结论还要求至少 5 份成对精度反馈和 5 份含金丝雀报告。历史 text 别名继续单列，不能按当前配置冒认旧模型身份。`--since` 按调用启动日期（UTC）筛选，缺日期的旧记录在有日期过滤时不纳入。

`recommend --task-type X [--jobs N]` 默认 N=10；主力取精度×召回最高的可比较通路，约 20% 留给样本不足的通路。没有足够证据时均匀探索，不下优劣结论；5 项以上按最大余数取整，小批次按真实历史补探索欠额。模型、CLI 可执行/配置指纹变化后，仅使用当前身份的样本重新积累；未知默认模型身份不会得到主力结论。冷却池、本地缺 CLI 或未配置 API 凭据的通路排除；这仍不保证登录和账户可用。`--only/--exclude` 收窄候选，`--cwd` 使本地 CLI 配置与实际执行目录一致。普通 run/review 排序不变。

batch 清单可写 `route: "auto"`，同时指定 `task_type`；按任务类型、材料是否 text_ready、实际 cwd 分组套用上述建议。需要 API 的任务必须声明 `text_ready: true`；否则只选能读 cwd 的通路。保留显式 fallback 的 quota 备援规则；可用 batch 的 `--only/--exclude` 收窄 auto 候选。宿主须先取得对候选模型和材料范围的授权。

判断是否外包的判据是：**宿主核验成本＋漏报率×返工成本 < 宿主自己做的成本**。multi-ai 提供前两项的实测数（核验分钟和漏报率）及外部耗时；返工成本与宿主自己做的成本由宿主估计，不假装能自动算出收益，更不把外部秒数当作 token 或费用节省。样本少或漏报高时，应先修判据、补宿主已知答案和同题核验，再扩大委派。

### 实施反馈与旧记录重判（2026-10-21）

```sh
multi-ai feedback --run RUN --report ATTEMPT \
  --outcome merged-after-rework --rework-items 2 --rework-rounds 1 \
  --defect-tags criterion-duplicated,id-collision --rework-min 15 --screen-min 8 \
  --evidence '宿主复核与实际门禁证据'
multi-ai scorecard --task-type implement --json
multi-ai recommend --task-type implement --cwd WORKTREE --only codex-cli --json
```

`outcome` 取 `merged`、`merged-after-rework`、`discarded`；`rework-items` 是宿主返工条数，`rework-rounds` 是另派返工轮数，未知留空。`defect-tags` 是逗号分隔的自由字符串，可用空值更正为空列表。建议标签：`criterion-duplicated`、`id-collision`、`comment-format-churn`、`pagination-clears-data`、`sort-tiebreak`、`performance`、`browser-only`。旧 accepted/false-positive 字段继续可用，更正逐字段追加，标签列表整体替换；source 默认 agent。

成绩单按通路×任务类型分组，并将已记录的不同模型分开（model=null 不冒认身份）。失败率只计执行失败；外部变化警告、归因越界及 Claude 配置越界各自计次数，不因政策状态重复算执行失败。若越界同时执行失败，仍按内层执行原因计失败。实施格子显示反馈样本数、合入率（merged 与 merged-after-rework 合计 / 已知 outcome）、返工后合入率、弃用率、平均返工条数和轮数、缺陷标签直方图、中位/P90 秒，不给 precision。平均值只用对应已知字段，JSON 保留覆盖数。至少 5 次调用、5 份 outcome 和 5 份返工条数才可比较；recommend 优先合入率，平手取较少返工，保留探索与身份门槛。

Codex 模型解析与 routes 展示共用解析器：显式模型优先，否则从配置顶层 model 取唯一确定的值；配置冲突或未知则 model=null、model_source=cli-default-unknown，不猜 profile。若事件明确带 model，以实测值记录；和配置不同会拆开版本身份，不把旧成绩冒认为当前配置。未知默认模型仍不提供恢复成功缓存或主力结论。

旧的快照误报可离线重判。先停止相关 multi-ai 写入，备份历史目录；脚本需要 Python 3（工具本体不依赖 Python）。默认只预演，每条旧 status 重复一个 `--status`：

```sh
python3 scripts/reclassify-implement.py --state-dir STATE \
  --status SUMMARY.md.status.json
# 核对 eligible 后追加 --apply；发布包中脚本位于包根目录
python3 scripts/reclassify-implement.py --state-dir STATE \
  --status SUMMARY.md.status.json --apply
# 再用原 run/attempt 补录实施反馈
multi-ai feedback --run RUN --report ATTEMPT --outcome merged \
  --rework-items 0 --evidence '宿主复核证据'
multi-ai scorecard --task-type implement --json
```

判据：只处理没有 attribution 的旧 boundary_violation；response.status=ok、response.complete=true、response.exitCode=0、summary_published=true、Git 检查正常，且当前发布摘要以完整非空 last-message 正文结尾。校验 run/attempt、manifest、status 和 artifacts 路径身份。宿主显式应用才将旧未归因变化重判为警告、缺失类型补 implement，并登记绑定 SHA-256 的摘要；不推断采纳、返工或历史模型。保留原诊断正文与 `summary_kind=diagnostic`，原始元数据备份到 artifacts/legacy-before-reclassification.json，完整旧越界路径也在该备份。所有记录先预检，文件逐个原子替换，manifest 最后发布；进程中断需从备份人工恢复后重试，不在有并发写入时执行。同一 run 一次仅修一个 attempt。无法满足完整性判据的记录拒绝重判。

## 跨会话账本与派发 pre-hook

账本与 pre-hook 已内置于 Rust 二进制；`integration/skills/multi-ai/scripts/run.sh` 保持纯转发。`delegate`、`batch`、`review`、`run`、`verify` 各追加一对 start/finish JSONL 事件，参数错误和钩子拒绝也会收尾。账本默认 `~/.local/state/ai-ledger/events.jsonl`，`AI_LEDGER_FILE` 可覆盖；显式空串或 `off` 关闭。追加失败仅向 stderr 警告，不改变命令退出码。强杀、断电等未完成调用会留下 start，旧读者仍可用 host/pid 判 running/lost。

`--card CARD` 优先于 `AI_LEDGER_CARD`，均未设置时省略 card，不要求必填。`task_type` 优先取 `--task-type`；否则 `delegate implement`、`review`、`verify` 分别推断为 implement、review、verify，其余为 null。账本与报告级 feedback.jsonl 分工独立。

| 字段 | 兼容约定 |
|---|---|
| v、at、id、event | 保留：版本 1、UTC 时间、每次调用唯一且 start/finish 共用的 id、事件类型 |
| actor、what、cwd、out | start 保留：multi-ai、命令及位置参数、目标目录、输出路径；finish 继续带 out |
| pid、host、branch、head | start 保留；finish 重新读取 head；Git 信息不可得时为空串 |
| card | 旧字段，改为可选；参数优先于环境变量，无来源则省略 |
| exit | finish 保留，等于命令最终退出码 |
| task_type | start 新增，可空；旧读者忽略此字段即可 |

只对 `delegate implement`，运行前读取目标 `--cwd`（未指定时当前目录）的 `.multi-ai/hooks.json`；文件不存在则静默跳过。示例：

```json
{"pre_delegate_implement": [{"cmd": ["node", "scripts/collaboration/dispatch-preflight.mjs"], "required": true}]}
```

钩子按数组顺序在 cwd 中执行，cmd 为可执行文件与参数数组，不经 shell 展开；需要 shell 时显式指定。钩子的 stdout/stderr 均发往主命令 stderr，保持报告 stdout 格式，stdin 关闭。钩子环境带 `MULTI_AI_ALLOW_DIRTY`（主命令带 `--allow-dirty` 时为 1，否则为 0），供预检放行未提交修改。`required: true` 非零退出或无法启动即拒绝派发并说明原因，false（默认）仅警告并继续。配置损坏或无法读取会拒绝派发。钩子是工作树提供的可执行配置，使用前应检查其内容；二进制不包含业务项目路径。

本机切换：先安装新版二进制，再升级 skill；若旧 runner 曾私改记账或预检，升级会保留用户修改并给出合并指南。此时应将本机 `run.sh` 恢复为上游纯转发模板，否则重复记账；原来隐式发现的项目预检需迁移到目标工作树 `.multi-ai/hooks.json`（需要传 cwd 等参数时在 cmd 中明确填写）。核对纯转发 runner 后，用临时 `AI_LEDGER_FILE` 执行离线命令验证仅一对事件。本次源码修改不操作本机安装文件。

## 从 Git 建议实施 outcome

```sh
multi-ai outcome suggest --run RUN --report ATTEMPT --into main
multi-ai outcome suggest --all --into refs/heads/main --json
multi-ai outcome suggest --all --into main --apply
```

仅处理 `delegate implement codex-cli` 的记录；没有已登记完整报告的 attempt 只返回 `unknown`。默认只读，无需加载凭据或调用模型。扫描使用 `MULTI_AI_STATE_DIR`（默认状态目录），每个仓库分别解析 `--into`；不会 fetch。输出包含建议、运行前 HEAD、目标 REF 解析出的 commit、分支 tip、相关提交和匹配文件。已有绑定相同报告哈希的 outcome 跳过；`--apply` 才经现有 feedback 校验与追加路径写入 `merged` / `discarded`，source 固定为 agent，证据注明 git 推断、REF 和 commit。未知数量和返工字段留空，`unknown` 永不写入；重复或并发应用会在反馈锁内再次检查，避免重复录入。

判据按以下顺序执行，可用输出的提交号与文件列表复核：

- 读取 manifest 内该 attempt 登记的 status（`reports[].result`），使用 cwd、`git_before.branch/head`、`git_after.head` 和 `changed_files`。这份 status 不受之后复用输出路径影响。完整正文须匹配登记哈希，运行中 HEAD 变化、无改动文件均为 `unknown`。
- `discarded` 只有一种情形：工作树与 REF 可读、运行时本地分支已不存在，且所有 attempt 文件均未出现在 `运行前 HEAD..REF` 的净差异中。分支已删但有 attempt 文件到达 REF（合入后删分支、复用工作树）为 `unknown`。分支仍在时，必须仍从运行前 HEAD 派生。
- `merged`：运行前 HEAD 到当前分支 tip 恰有一次提交，且提交者时间在运行开始时或之后；该 tip 是 REF 的祖先，提交只涉及 attempt 文件，且 attempt 文件至少一个出现在 `运行前 HEAD..REF` 的净差异中。这允许宿主在运行后提交。多次提交或夹带其它文件均保守为 `unknown`。
- 分支仍在、运行后尚无提交时为 `unknown`：宿主可能仍在复核未提交的改动，不判弃用。为处理 Git 秒精度，开始所在秒也视为运行后的候选提交。
- 其它情形给 `unknown`，保留可得证据；绝不猜 `merged-after-rework`。无法区分返工时需宿主自行判断并用 `feedback` 录入。

这些是建议，不是代码内容等价性证明：同一文件中其它人的修改也会命中文件判据，单次提交内的返工无法识别；squash/cherry-pick、分支重建、改写历史、提交时间不可信、修改后又恢复都会降低判定能力。不会用 reflog 或联网找回历史。旧 status 没有分支字段、detached HEAD、工作树/仓库已删除、Git 不可用、REF 无效或报告损坏时降级为 `unknown`。本次新增分支记录，不按当前分支冒认旧运行身份。此功能没有强制录分门禁。
