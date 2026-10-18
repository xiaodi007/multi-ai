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

`doctor` 默认只检查本地配置、凭据是否配置及 CLI 是否可执行，不验证登录或账户健康。`doctor --live`、`probe` 和 `text list-models` 才联网；模型探测会使用额度。`review --dry-run` 保留旧版的额度采集行为，需要纯离线检查时使用 `routes` 或 `doctor`。

## 通路、配置与凭据

| 新通路 | 旧 ai-review 名称 | 额度池 | 模型来源 |
|---|---|---|---|
| `agy-cli` | `agy` | `google` | 固定 `gemini-3.8-flash-high` |
| `grok-cli` | `grok` | `xai` | CLI 默认，未知 |
| `codex-cli` | `codex` | `openai` | CLI 默认，未知 |
| `glm-api` | `glm` | `zhipu` | profiles 别名 `glm` |
| `deepseek-pro-api` | `deepseek` | `deepseek-api` | profiles 别名 `deepseek-pro` |
| `claude-deepseek-cli` | `claude-ds` | `deepseek-api` | Claude Code 壳配置 |
| `kimi-cli-default` | `kimi` | `kimi-code` | CLI 默认，未知 |
| `kimi-k3-cli` | `kimi-k3` | `kimi-code` | 固定 `kimi-code/k3` |
| `kimi-k3-api` | `kimi-k3-api` | `moonshot-api` | profiles 别名 `kimi-k3` |

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

至少提供一个数量及证据；不知道的数量或筛选／返工时间留空。反馈绑定报告哈希并追加保存，更正需录入新的事件，原记录保留。默认 `source=agent`；只有用户明确决定，才可加 `--source human --human-confirmed` 并记录其明确意见。模型的 `CONFIRMED` 和测试通过都不自动代表人工采纳。反馈不自动评分、不改变调度排序。

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
multi-ai batch --jobs jobs.json --outdir results --timeout 1800 --resume
multi-ai run kimi-k3-cli --fallback grok-cli --prompt-file brief.md --out answer.md
multi-ai stats
multi-ai stats --json
```

每个额度池默认并发 2，清单或重复的 `--pool-limit POOL=N` 可覆盖。首选与备选均须明确列入清单，API 任务用 `text_ready: true` 声明 prompt 已含全部材料。quota 立即切到备选，池冷却期间后续任务跳过该池；5 小时用量错误在未提供重置时刻时保守冷却 5 小时。rate-limit（包括 429 code 1302）先按指数退避重试，CLI 默认额外重试 2 次、初始延迟 500 毫秒；原生 API 使用其已有重试机制，初始延迟 1000 毫秒，`--rate-limit-retries` 可覆盖次数，`--retry-delay-ms` 仅控制 CLI。重试、退避和备援共用每项的秒单位 `--timeout`；排队等待首个额度池不消耗预算。其他失败保留独立状态，退出码非零不妨碍其余任务完成。取消整个批次会停止排队任务并清理正在运行的 CLI 进程组，不再启动备援或新任务。

`--resume` 继续使用完整身份和正文哈希校验；成功备选也能直接恢复，未知模型身份仍重新执行。非 Git 任务用清单的 `source_id` 或全局 `--source-id` 标识真实材料版本。修改输入、模型、代码或策略身份后会重新请求，不保证“只看 id 跳过”。失败不替换上一份完整正文，须结合本轮 status 阅读。

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

此入口固定调用 `codex exec -C <cwd> -s workspace-write --skip-git-repo-check -o <暂存摘要>`，不再套外层 macOS sandbox，也不额外覆盖 Codex 的网络配置。写入边界由 Codex 自带的 `workspace-write` 加事后核对负责。仍需在提示中明确授权要读取、修改的材料；写权限限制不等于读取材料范围的自动证明。

运行前后记录 HEAD、`git status --porcelain` 和 Git 可见文件身份，并递归记录 cwd 外同级 Git 工作树、`~/.claude`、`~/.agents` 的 mtime 快照（仅排除本次受管理产物目录；内部符号链接只记录链接本身）。摘要列出本轮变化文件并区分原有修改；Git 忽略的缓存等文件不进入该文件清单。事后发现监测范围内的越界变化时，在 `<summary>.status.json` 标为 `boundary_violation` 并列出路径，只报告、不回滚。快照无法发现通过网络写远端、监测范围外的写入，以及两次快照间已消失或恢复 mtime 的写入；并发进程的变化也无法仅凭快照归因给 Codex。

不自动提交，不支持实施恢复缓存。失败、取消、超时或额度耗尽均记录在 `<summary>.status.json`，失败保留上一份完整摘要；目录和输出冲突均在启动 Codex 前失败。测试不会调用真实外部模型。
