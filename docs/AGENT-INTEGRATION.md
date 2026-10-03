# 让 Codex 和 Claude Code 使用 multi-ai

安装包含两层：原生二进制负责执行，skill 负责让主 AI 发现工具、选命令、准备材料和核验报告。只把二进制放进 PATH，主 AI 不会因此自动知道这套工作流。

## 新机器

先按 [README 的 Release 安装步骤](../README.md#从-github-release-安装) 下载、校验并解压 macOS arm64 发布包。私有仓库需要有访问权限的 GitHub 账号；安装 skill 不会替代程序安装。在解压后的发布包中运行：

```sh
shasum -a 256 -c SHA256SUMS
./install.sh
./install-skills.sh
```

程序安装到 `~/.local/bin/multi-ai`，独立的 `multi-ai` skill 默认安装到：

| 使用方 | 用户级 skill 目录 | 显式调用 |
|---|---|---|
| Codex | `~/.agents/skills/multi-ai` | `$multi-ai`；CLI 的 `/skills` 也可选择 |
| Claude Code | `~/.claude/skills/multi-ai` | `/multi-ai` |

以上目录和显式调用方式依据当前 [OpenAI 官方技能文档](https://learn.chatgpt.com/docs/build-skills) 与 [Claude Code 官方技能文档](https://code.claude.com/docs/en/skills)。skill 的名称和描述会参与发现，完整使用说明在选中后加载。自然语言也可触发，例如“用 multi-ai 对这份材料做两个模型评审，再由你核验结论”。是否自动选中取决于任务匹配、运行时与管理员设置；显式调用更便于确认。

安装脚本不修改全局 `AGENTS.md`、`CLAUDE.md`、shell 配置或 agent 权限，不注册 MCP，不复制 API key。API 凭据按现有读取顺序自行配置；CLI 路线还需要相应客户端安装与登录。skill 的 runner 直接执行 Rust 二进制，不依赖 Node，也能在 agent 的 PATH 没有 `~/.local/bin` 时使用默认安装位置。

先开新会话，输入 `$multi-ai` 或 `/multi-ai`，要求“仅运行 routes 和 doctor，列出可用通路，先不要联网”。然后明确指定材料与通路，执行一次短真实请求。`doctor` 的配置就绪不代表已经联网成功。

本地安装不会部署到 Codex 云端环境、Claude Cowork 或 Claude 云会话。那些环境需要各自可执行的二进制、配置与凭据；本发布包只验收 macOS Apple Silicon。

## 旧版本兼容

部分旧 Codex 运行时从 `~/.codex/skills` 读取技能；可显式安装到旧入口：

```sh
./install-skills.sh --only codex --codex-dir "$HOME/.codex/skills"
```

优先按所用 Codex 的实际发现目录安装。macOS 验证中的 CLI 和桌面运行时 0.160.0 的 `skills/list` 行为是：默认目录与旧目录各放一份同名 skill，会列出两个启用条目，不会跨目录去重。不要把双目录复制作为通用安装步骤；仅保留默认目录时，两种运行时均已发现唯一启用入口。不同版本的发现及去重行为应以自己的技能列表核验。

已有 `multi-llm-pipeline`、`ai-review` 技能保留。`install-skills.sh` 用于新建独立 `multi-ai` 技能；`update-skills.sh` 用于迁移已有旧技能。前者默认保留整个已存在的 `multi-ai` 目录，不覆盖用户修改，即使该目录尚无 `SKILL.md` 也会提示保留。

## 诊断与回退

```sh
~/.local/bin/multi-ai --version
~/.agents/skills/multi-ai/scripts/run.sh routes --json
~/.claude/skills/multi-ai/scripts/run.sh doctor
```

若没有发现技能，检查相应目录的 `SKILL.md` 是否存在，查看 Codex `/skills` 或 Claude 的技能列表，再重启会话。已安装文件并不等于当前旧会话一定已重新加载元数据。macOS 验证中 Claude 普通模式的 `/multi-ai` 已加载成功；`--bare` 启动时未加载此用户技能，应使用普通会话验收。`MULTI_AI_BIN=/绝对路径/multi-ai` 可为 runner 指定其他二进制。

skill 安装输出独立的 `BACKUP_DIR=…`，回退命令是：

```sh
./rollback-agent-skills.sh /该次安装输出的/BACKUP_DIR
```

它只删除该次新建且仍未改动的技能；任何修改、额外文件、替换符号链接或父目录变化都会保留整个技能目录。原本已有的技能从未被覆盖。保留备份记录，并先回退技能，再回退程序安装。旧技能迁移仍使用单独的 `rollback-skills.sh`。

安装脚本支持 `--home DIR`、`--codex-dir DIR`、`--claude-dir DIR`、`--only codex|claude|both` 和 `--templates DIR`。自定义目标必须位于所选 home 内；`--templates` 指向包含 `SKILL.md`、`scripts/run.sh` 和 `agents/openai.yaml` 的模板目录。可隔离验证：

```sh
sh tests/test-agent-skills.sh
```
