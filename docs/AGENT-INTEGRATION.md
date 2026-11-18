# 让 Codex 和 Claude Code 使用 multi-ai

安装包含两层：原生二进制负责执行，skill 负责让主 AI 发现工具、选命令、准备材料和核验报告。只把二进制放进 PATH，主 AI 不会因此自动知道这套工作流。

## 新机器

本轮功能使用 [README 的源码构建步骤](REFERENCE.md#从源码开发与构建) 生成并校验 macOS arm64 本地包；固定 v0.1.0 归档尚无本轮参数。私有仓库需要有访问权限的 GitHub 账号；安装 skill 不会替代程序安装。在解压后的发布包中运行：

```sh
shasum -a 256 -c SHA256SUMS
./install.sh --no-legacy-aliases
./install-skills.sh
```

程序安装到 `~/.local/bin/multi-ai`，独立的 `multi-ai` skill 默认安装到：

| 使用方 | 用户级 skill 目录 | 显式调用 |
|---|---|---|
| Codex | `~/.agents/skills/multi-ai` | `$multi-ai`；CLI 的 `/skills` 也可选择 |
| Claude Code | `~/.claude/skills/multi-ai` | `/multi-ai` |

以上目录和显式调用方式依据当前 [OpenAI 官方技能文档](https://learn.chatgpt.com/docs/build-skills) 与 [Claude Code 官方技能文档](https://code.claude.com/docs/en/skills)。skill 的名称和描述会参与发现，完整使用说明在选中后加载。自然语言也可触发，例如“用 multi-ai 对这份材料做两个模型评审，再由你核验结论”。是否自动选中取决于任务匹配、运行时与管理员设置；显式调用更便于确认。

安装脚本不修改全局 `AGENTS.md`、`CLAUDE.md`、shell 配置或 agent 权限，不注册 MCP，不复制 API key。API 凭据按现有读取顺序自行配置；CLI 路线还需要相应客户端安装与登录。skill 的 runner 直接执行 Rust 二进制，不依赖 Node，也能在 agent 的 PATH 没有 `~/.local/bin` 时使用默认安装位置。

v0.2 的受限调查在每次委派内部启动同一 Rust 程序的 stdio MCP 读取服务，只向受控 Kimi／Grok 配置开放快照的 `list/search/read`。这是一次调用内的传输，不需要在 Codex／Claude 注册常驻 MCP，也不会让工具出现在宿主原生子 agent 面板。授权材料和通路后，skill 可以先委派大批阅读，再由主 AI 阅读短交接、核验关键证据并完成测试。默认最多并行两个独立任务；完整边界和命令见 [DELEGATION.md](DELEGATION.md)。

先开新会话，输入 `$multi-ai` 或 `/multi-ai`，要求“仅运行 routes 和 doctor，列出可用通路，先不要联网”。然后明确指定材料与通路，执行一次短真实请求。`doctor` 的配置就绪不代表已经联网成功。

本地安装不会部署到 Codex 云端环境、Claude Cowork 或 Claude 云会话。那些环境需要各自可执行的二进制、配置与凭据；本发布包只验收 macOS Apple Silicon。

## 旧版本兼容

部分旧 Codex 运行时从 `~/.codex/skills` 读取技能；可显式安装到旧入口：

```sh
./install-skills.sh --only codex --codex-dir "$HOME/.codex/skills"
```

优先按所用 Codex 的实际发现目录安装。macOS 验证中的 CLI 和桌面运行时 0.160.0 的 `skills/list` 行为是：默认目录与旧目录各放一份同名 skill，会列出两个启用条目，不会跨目录去重。不要把双目录复制作为通用安装步骤；仅保留默认目录时，两种运行时均已发现唯一启用入口。不同版本的发现及去重行为应以自己的技能列表核验。

统一使用独立 `multi-ai` 技能。安装包不再包含或更新 `multi-llm-pipeline`、`ai-review` 技能；已有旧目录和调用点交由宿主迁移，不自动删除。`install-skills.sh` 用于安装独立技能，`update-skills.sh` 只转发其 `--upgrade`。前者默认保留整个已存在的 `multi-ai` 目录，不覆盖用户修改，即使该目录尚无 `SKILL.md` 也会提示保留。

已有独立 multi-ai skill 可显式升级受管理文件：

```sh
./install-skills.sh --upgrade
```

升级只替换内容哈希和权限均匹配已发布 v0.1 或当前模板的文件，先备份再逐个发布，入口 `SKILL.md` 最后更新。被用户修改的文件和未知文件保留；输出 `MERGE_GUIDE=…` 指向需手工合并的位置。`references/delegation.md` 与 `references/route-traits.md` 随安装／升级进入 skill 目录。不会通过本地“已管理”标记把未知内容当成受管理文件。

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

它只删除该次新建且仍未改动的技能；任何修改、额外文件、替换符号链接或父目录变化都会保留整个技能目录。原本已有的技能从未被覆盖。保留备份记录，并先回退技能，再回退程序安装。历史旧技能备份可继续用 `rollback-skills.sh` 恢复；该脚本不安装技能。

对于 `--upgrade` 的备份，回退按文件检查安装后哈希和权限：未改动的文件恢复原内容及权限，新加且未改动的文件删除；之后修改的文件保留。原文件备份也校验哈希，拒绝损坏的备份。未知文件和已变化的父目录不处理。

安装脚本支持 `--home DIR`、`--codex-dir DIR`、`--claude-dir DIR`、`--only codex|claude|both` 和 `--templates DIR`。自定义目标必须位于所选 home 内；`--templates` 指向包含 `SKILL.md`、`scripts/run.sh` 和 `agents/openai.yaml` 的模板目录。可隔离验证：

```sh
sh tests/test-agent-skills.sh
```
