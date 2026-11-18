# multi-ai

[English](README.md) · [完整参考](docs/REFERENCE.md) · [许可证：MIT](LICENSE)

**multi-ai** 是一个 Rust 原生的单文件命令行工具。主力编码 agent（Claude Code、Codex 等）通过它调用**其他** AI 模型，每次调用都受控、可追溯。主要用途：

- 拿**独立的第二意见**：让不同厂商的模型并行评审代码或方案；
- **委派**边界明确的活：仓库调查、材料抽取、挑反例、在沙箱里实施，交给更便宜或更合适的模型；
- **实测**哪个模型适合哪类任务，依据是你自己核验过的结果，不靠感觉。

它既能调用你本机已登录的厂商 CLI（Codex、Grok、Kimi Code、Antigravity、接到其他厂商的 Claude Code），也能直接调 HTTP API（DeepSeek、智谱 GLM、Moonshot Kimi）。每次调用都有记录，报告原子发布；材料只有显式附带的才会发出去。

> 状态：v0.2.x。只在 **macOS Apple Silicon** 上验收过；其他 Unix 理论上能编译，但没有验证过。

## 为什么需要它

让一个 AI 去检查另一个 AI 的工作，常见几种翻车：

| 问题 | multi-ai 的做法 |
|---|---|
| 评审附和作者（谄媚、同家族模型） | 评审时排除实施方的模型家族；发送前 lint 宿主结论句并做盲化；用诱饵金丝雀实测**附和率**；可固定一个**异议席** |
| 两份评审看似独立，其实来自同一额度池或同一家族 | 每条通路都显式标注 `pool` 与 `family`，席位按额度池去重 |
| 跑到一半失败、额度用尽、模型 id 已下线 | 额度池冷却、限流退避、显式备选、`batch --preflight`、`doctor --models` |
| 结果被悄悄覆盖，或几轮结果互相串 | 调用模型前先加输出锁；失败不会替换上一份完整报告；`--resume` 只复用身份和正文哈希都一致的结果 |
| 「该用哪个模型」全凭感觉 | `feedback` → `scorecard` → `recommend`：按「通路 × 任务类型」统计精度、金丝雀召回、耗时、失败率 |
| 委派实施改到了不该改的地方 | `delegate implement` 只在干净的 git 工作树里、用固定沙箱运行，前后各拍一次快照并记录越界写入，**从不自动提交** |

## 安装

### 从源码构建（0.2.x 推荐）

需要 Rust ≥ 1.89（edition 2024）和 macOS 开发工具。装好后的二进制运行时不依赖 Rust、Python 或 Node。

```sh
git clone https://github.com/xiaodi007/multi-ai.git
cd multi-ai
cargo test --locked
./scripts/package.sh
cd dist/multi-ai-*-aarch64-apple-darwin
shasum -a 256 -c SHA256SUMS
./install.sh --no-legacy-aliases   # 安装 ~/.local/bin/multi-ai
./install-skills.sh                # 可选：给 Codex / Claude Code 装 skill
```

- 二进制装在 `~/.local/lib/multi-ai/<版本>/`，`~/.local/bin/multi-ai` 指向它。如果 `~/.local/bin` 不在 PATH 里，自己加上；安装脚本不改 shell 启动文件。
- 每次安装前都会先做私有备份，并输出 `BACKUP_DIR=…`。回退用 `./rollback.sh <BACKUP_DIR>`。
- 已有的 `~/.config/multi-ai/*.json` 不会被覆盖，凭据也不会被复制。

### 从 GitHub Release 安装

[Releases](https://github.com/xiaodi007/multi-ai/releases) 目前只有 **v0.1.0**。这个版本还没有 `batch`、`delegate`、`scorecard` 和防附和功能，要用这些请从源码构建。

```sh
gh release download v0.1.0 --repo xiaodi007/multi-ai \
  --pattern 'multi-ai-0.1.0-aarch64-apple-darwin.tar.gz*'
shasum -a 256 -c multi-ai-0.1.0-aarch64-apple-darwin.tar.gz.sha256
tar -xzf multi-ai-0.1.0-aarch64-apple-darwin.tar.gz && cd multi-ai-0.1.0-aarch64-apple-darwin
./install.sh
```

### Agent skill

`install-skills.sh` 会把 `multi-ai` skill 装到 `~/.agents/skills/multi-ai`（Codex 里用 `$multi-ai` 调用）和 `~/.claude/skills/multi-ai`（Claude Code 里用 `/multi-ai`）。同名目录已存在时保持原样不动；以后升级受管理文件用 `./install-skills.sh --upgrade`。详见 [docs/AGENT-INTEGRATION.md](docs/AGENT-INTEGRATION.md)。

## 配置

1. **厂商 CLI（可选）**：想用哪个就装哪个并登录：`codex`、`grok`、`kimi`、`agy`、`claude`。缺了某个 CLI，只影响它自己的通路。
2. **API key（可选）**：把 [.env.example](.env.example) 复制到 `~/.config/multi-ai/.env`，只填你要用的：

   ```sh
   DEEPSEEK_API_KEY=
   ZHIPU_API_KEY=        # 或 GLM_API_KEY
   KIMI_API_KEY=         # 或 MOONSHOT_API_KEY
   ```

   取值按这个顺序补缺：进程环境变量 → 当前目录 `.env` → `~/.config/multi-ai/.env` → 旧的 `~/.config/multi-llm-pipeline/.env`。日志里不会输出密钥。
3. **离线自检**：

   ```sh
   multi-ai routes          # 列出全部通路，以及各自的额度池、家族、模型来源
   multi-ai doctor          # 检查本地配置、凭据是否配了、CLI 能否执行
   multi-ai doctor --models # （联网）核对配置里的模型 id 还在不在
   ```

配置目录是 `~/.config/multi-ai/`：`routes.json` 管 CLI 通路、额度池、排序和超时，`profiles.json` 管 API 厂商、别名、任务和提示词。运行状态与报告放在 `~/.local/state/multi-ai/`。两个目录分别可以用 `MULTI_AI_CONFIG_DIR`、`MULTI_AI_STATE_DIR` 改位置。

## 快速上手

```sh
# 调用一条指定通路；厂商 CLI 可以在 --cwd 里用它自己的工具
multi-ai run kimi-k3-cli --prompt-file brief.md --cwd ~/src/project --out review.md

# API 通路只看得到你附带的材料
multi-ai text glm --input evidence.md --out answer.md

# 两个不同额度池的席位并行评审，排除写这次改动的模型家族
multi-ai review --input diff.md --text-ready --outdir reviews --seats 2 --exclude-family openai

# 对照多份评审：结论相反的排最前，列成待执行验证清单
multi-ai consensus reviews/*.md --out consensus.md

# 按代码逐条复核发现
multi-ai verify --findings review.md --cwd ~/src/project --out verify.md
```

### 委派

```sh
# 只打包你授权的材料，交给便宜模型调查
multi-ai pack --root ~/src/project --scope src/feature --input AGENTS.md --outdir /tmp/task/packet
multi-ai delegate investigate kimi-cli-default --packet /tmp/task/packet \
  --prompt-file task.txt --outdir /tmp/task/out --fallback grok-cli

# 在干净工作树里沙箱实施（不自动提交，越界写入会记进报告）
multi-ai delegate implement codex-cli --cwd ~/src/project-wt \
  --prompt-file brief.md --out /tmp/task/summary.md --timeout 1800
```

四种角色：`investigate`（搜索、阅读代码）、`extract`（抽取、对照材料）、`challenge`（找反例）、`implement`（实施）。详见 [docs/DELEGATION.md](docs/DELEGATION.md)。

### 批量、打分、选模型

```sh
multi-ai batch --jobs jobs.json --outdir results --pool-limit xai=2 --preflight --resume
multi-ai feedback --batch results --from verdicts.tsv     # 录入宿主实际核验的结果
multi-ai scorecard --task-type review --json             # 各通路的精度、召回、耗时
multi-ai recommend --task-type review --jobs 10          # 给出分配建议
multi-ai stats                                           # 离线运行统计
```

`jobs.json` 示例：

```json
{
  "pool_limits": {"xai": 2, "kimi-code": 1},
  "jobs": [
    {"id": "review-one", "prompt_file": "brief.md", "cwd": "../worktree",
     "route": "kimi-k3-cli", "fallback": ["grok-cli"], "task_type": "review",
     "canaries": [{"match": "src/check.rs:42", "expect": "真问题"}]}
  ]
}
```

**金丝雀**是宿主事先确认过的事实，答案不会发给模型，用来测召回。**诱饵**（`"decoy": true`）是故意写进任务书的错误主张，用来测模型有多容易附和。

## 命令一览

| 命令 | 作用 | 联网 |
|---|---|---|
| `routes`、`doctor`、`status`、`stats`、`help` | 查看配置、健康、冷却、本地历史 | 离线 |
| `run` | 调一条指定通路（可加 `--fallback`） | ✔ |
| `review` / `dispatch`、`verify` | 多席位并行评审、逐条复核发现 | ✔ |
| `text` | 原生文本 / API 功能（别名、profile、领域、看图） | ✔ |
| `batch` | 批量任务，支持按额度池限并发、备选、预检、恢复 | ✔ |
| `pack`、`delegate` | 材料快照；investigate / extract / challenge / implement | ✔ |
| `consensus` | 按 `文件:行号` 对齐多份报告，列出相互矛盾的结论 | 离线 |
| `feedback`、`scorecard`、`recommend`、`outcome` | 录入核验结果、给通路打分、建议分配、从 git 推断实施是否合入 | 离线 |
| `probe`、`usage` | 显式探测通路、查看账户额度 | ✔ |
| `reset`、`stop` | 清除某个额度池的冷却、结束自己的一个运行 | 离线 |
| `install-config` | 导入配置，不覆盖已有文件、不复制凭据 | 离线 |

单位：`run/review/verify/batch` 的超时单位是**秒**；`text --timeout-ms` 的单位是**毫秒**。

## 安全边界

- 只发送你显式附带的材料；API 通路无法自己浏览你的仓库。
- 确定性测试不联网，也不读真实凭据。
- 原始响应、manifest 和反馈都只存在本地状态目录；反馈只追加不改写，并绑定报告哈希。
- `feedback` 默认 `source=agent`；只有显式加 `--source human --human-confirmed`，结果才算人工确认。
- 委派实施有沙箱，跑完后还会逐项核对，但它终究在你的机器上执行代码，只给它你信得过的工作树。

## 文档

| 文档 | 内容 |
|---|---|
| [docs/REFERENCE.md](docs/REFERENCE.md) | 完整参考：所有通路、参数、失败分类、打分口径 |
| [docs/DELEGATION.md](docs/DELEGATION.md) | 委派流程、边界与指标 |
| [docs/AGENT-INTEGRATION.md](docs/AGENT-INTEGRATION.md) | Codex / Claude Code 发现 skill 的方式、旧入口兼容 |
| [docs/CLI-ISOLATION.md](docs/CLI-ISOLATION.md) | 厂商 CLI 怎样与宿主配置隔离 |
| [docs/COMPATIBILITY.md](docs/COMPATIBILITY.md) | 与旧 `ai-review` / `llm-run` 的兼容 |
| [docs/VALIDATION-v0.2.md](docs/VALIDATION-v0.2.md)、[docs/TOKEN-BENCHMARK-v0.2.md](docs/TOKEN-BENCHMARK-v0.2.md) | 验收记录与 token 成本实测 |
| [docs/tasks/](docs/tasks/) | 每次改动的设计与任务记录 |

## 开发

```sh
cargo fmt --all -- --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
sh tests/test-agent-skills.sh
```

skill 模板在 `integration/skills/multi-ai/`；`scripts/package.sh` 把发布包生成到被 git 忽略的 `dist/`。协作约定见 [AGENTS.md](AGENTS.md)。

## 许可证

[MIT](LICENSE)
