# multi-ai

源码 v0.2 新增低成本委派：`pack` 离线材料快照，`delegate investigate|extract|challenge TARGET` 执行 Kimi/Grok 受限调查或 API 材料加工，返回短交接与可核验完整报告。使用前查看 `doctor` 的委派能力；完整流程、边界和指标见 [DELEGATION.md](docs/DELEGATION.md)。下方 GitHub 下载示例仍指已发布 v0.1.0；本轮 v0.2 可从源码构建或使用本地生成的发布包。

Rust 原生多 AI 工具，统一 `ai-review` 与 `llm-run`。首版验收 macOS Apple Silicon；API 请求、调度、报告、输出锁和反馈均在同一程序中执行。安装后的工具本体无需 Python、Node、uv 或 Rust 编译器。

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

在解压后的发布包目录执行：

```sh
shasum -a 256 -c SHA256SUMS
./install.sh
multi-ai --version
multi-ai routes
multi-ai doctor
# 让 Codex 与 Claude Code 发现独立的 multi-ai skill
./install-skills.sh
```

安装到 `~/.local/lib/multi-ai/<版本>/multi-ai`，并在 `~/.local/bin` 创建 `multi-ai`、`ai-review`、`llm-run` 三个兼容入口。若该目录尚未在 PATH 中，请在自己的 shell 配置中加入它；安装脚本不修改 shell 启动文件。

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

迁移已有旧技能：

```sh
./update-skills.sh
./rollback-skills.sh /该脚本输出的/BACKUP_DIR
```

`update-skills.sh` 更新本机已有的 Claude／Codex multi-llm-pipeline、Claude ai-review 说明及启动入口；缺失的 skill 保持缺失，不另建安装。它有独立的私有备份与校验清单，不复制凭据。回退时先执行 `rollback-skills.sh`，再执行程序的 `rollback.sh`；之后修改的 skill 文件不会被覆盖。

旧技能迁移只针对已有目录；新用户使用 `install-skills.sh`。两个安装／迁移脚本均支持 `--home DIR` 和 `--templates DIR` 供隔离验证或显式选择材料目录；发布包自带旧技能迁移模板和独立的 `integration/skills/multi-ai` 模板。

### 从源码开发与构建

开发者可克隆仓库，运行测试并生成本地发布包：

```sh
gh repo clone xiaodi007/multi-ai
cd multi-ai
cargo test --locked
./scripts/package.sh
```

构建需要 Rust/Cargo 和 macOS 开发工具；运行发布包不需要它们。产物位于 `dist`，包含二进制、安装／回退、skill 安装和迁移脚本、skill 模板、文档、`SHA256SUMS`、tar.gz 和归档校验和。构建后按上文在 `dist/multi-ai-<版本>-aarch64-apple-darwin` 中安装。打包不上传、不提交；生成的构建目录和发布包不进入源码 Git，通过 Releases 分发。

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

# 旧入口继续可用
llm-run glm --input evidence.md --out answer.md
ai-review run kimi-k3 --prompt-file evidence.md --cwd /path/to/repo --out review.md
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
