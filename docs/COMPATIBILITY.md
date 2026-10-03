# 兼容说明与验收口径

首版支持并验收 macOS Apple Silicon，二进制目标为 `aarch64-apple-darwin`。其他系统或架构不据此声明支持。

## 两个旧入口

安装后的 `multi-ai`、`ai-review` 和 `llm-run` 指向同一 Rust 二进制。程序依据调用入口保留语义：

| 原接口 | Rust 入口 | 保留行为 |
|---|---|---|
| `ai-review run PROVIDER` | `multi-ai run ROUTE` | 指定通路、材料、cwd、秒单位超时、报告 |
| `ai-review dispatch` | `multi-ai review`／`dispatch` | review/explore/verify、并行席位、按额度池去重、即时备援 |
| `ai-review verify` | `multi-ai verify` | 核验已有 findings，输出模型判断 |
| `ai-review probe/usage/status/reset` | 同名命令 | 显式探测、额度查看、冷却与重置 |
| `llm-run ALIAS` | `multi-ai text ALIAS` | 原 aliases、正文输入、JSON 与输出路径 |
| `llm-run run --task TASK` | `multi-ai text run` | 原 task、系统提示、模型默认值及显式覆盖 |
| `llm-run dispatch --domain DOMAIN` | `multi-ai text dispatch` | 原 domain/class 与领域路由，和多席位 dispatch 区分 |
| `llm-run profiles/domains/aliases` | 对应 `text` 子命令 | 离线展示配置 |
| `llm-run list-models` | `multi-ai text list-models` | 联网列 API 模型，凭据不展示 |
| `llm-run codex/grok …` | 对应 `text` 子命令 | 直接交给可选厂商 CLI，保留 argv |

旧 ai-review 路由名称为 `grok`、`codex`、`agy`、`kimi`、`kimi-k3`、`deepseek`、`glm`、`kimi-k3-api`、`claude-ds`。新路由名及对应额度池见 README。

旧 llm-run 的 `kimi` 是 API 别名 `kimi-k3-high`，与旧 ai-review 的 `kimi` CLI 不同；旧 llm-run 的 `deepseek` 是 Flash，旧 ai-review 的 `deepseek` 是 Pro API。旧入口保留各自含义，新入口要求明确选择通路。

`--only`／`--exclude`、`--no-metered`、`--agentic`、`--text-ready`、`--cwd`、`--source-id`、`--resume` 继续属于评审调度接口。`--input` 重复参数、stdin、JSON、图片、task/domain/roundtable、thinking/effort、stream、毫秒超时属于文本接口。各自参数不互相改名或换算单位。

DEAD aliases 默认失败；`--legacy` 沿用原 successor 路由，不复活停用模型。`text`／旧 `llm-run` 未指定 `--out` 时 stdout 输出正文／JSON；指定后所选格式写文件，stdout 打印 `Wrote <path>`。stderr 记录调度提示与进度，`--quiet` 控制提示。新增 run/attempt 身份用于追踪，不能把它们当作内容证据。

## 原生 API 与厂商 CLI

API 客户端无需 Python／Node。保持 GLM 默认流式、K3 high/max 默认流式、K3 的 `reasoning_effort` 与 K2.x 的 `thinking` 协议差异、always-thinking 模型以及原模型 guard。显式模型不被环境变量覆盖；角色模型按 profiles 的 modelEnv 解析。图片只允许匹配模型能力的通路。

响应头、空闲和总预算分别计时，退避计入总预算；认证失败不重试，已接收正文后不自动重发。截断、错误结束、缺结束标志、空正文或仅推理内容均不能发布完整报告。失败状态及 partial 保留用于诊断。

CLI 通路仍使用厂商程序、登录和工具能力。仅缺少某个 CLI 时，其对应通路不可用；API 通路可继续运行。Codex、Grok、Kimi、Antigravity 及 Claude Code 的执行权限和 argv 继承当前 routes，并不由 API 文本模型提供工具循环。

## 输出保护、缓存与历史

原生 run/review/verify、text 及旧 llm-run 的受管理输出统一取锁，包含报告、状态、缓存和派生目录。目录重叠也构成冲突；无关输出可并行。锁状态位于工作区之外，句柄不传给外部 CLI。任意外部编辑器或绕过本工具的脚本不参与锁协议。

使用同一输出路径顺序运行时，归档将被替换的已知报告／manifest／status，不清理未知文件或原始响应。新运行失败时，旧完整报告仍在原路径，本轮状态明确标失败。应一起查看状态与报告，避免将旧报告当成本轮成功。

恢复仅认完整正文哈希、材料与源码身份、实际模型和选项、配置及实现版本。CLI 同时绑定客户端可执行、相关脚本、已知模型配置及覆盖变量哈希；默认模型身份不能可靠确定时不缓存，恢复时提示并重新请求。缓存命中记录原 run/attempt 的复用身份，并返回本轮可录反馈的 ID，不再生成模型调用。Rust 不冒用旧 Python／Node 缓存身份；旧缓存失配后提示可能新请求。

非 Git 恢复依赖显式 `source-id` 表示材料版本，工具不能替维护者证明这个标识仍然真实。生成的历史、状态及反馈不应改变输入源码指纹。升级后首次 `--resume` 可能重新使用额度。

## 安装、回退与数据边界

安装脚本使用 macOS 标准 shell 工具，先建立私有备份，再部署版本二进制、导入配置、切换 PATH 入口。配置使用排他新建，不覆盖已有普通文件或符号链接；安装过程不复制凭据。旧引擎源码保持可用，以便另行回退。

回退只处理本次安装管理的文件；入口必须仍指向安装目标，二进制和新配置必须仍匹配安装哈希。之后修改的内容保留并提示。重复回退不删除修改后的用户文件。报告、缓存、反馈和备份不会被回退脚本清理。

`doctor` 默认离线且不证明登录健康。`doctor --live`／`probe` 会调用模型；`usage` 和 `text list-models` 可能联网。`review --dry-run` 沿用旧额度采集，不承诺零网络或零状态写入。

外部 API 只读取明确附带的材料。客户原件仅在维护者指定的范围内处理；凭据仍由环境或既有本地凭据文件提供，不进入源码仓库。工具结果只是线索，不能放宽业务门禁或代替用户验收。

## 报告级反馈

反馈以 run ID、attempt ID 和报告 SHA-256 绑定；正文变化则拒绝录入。数量为采纳、误报、重复、待定，未掌握字段留空；筛选与返工时间可选。原记录追加保存，不建立逐发现数据库。

默认来源为主 AI 判断，需要具体证据。人工来源还要求明确用户意见及 `--human-confirmed`；此标志是代录声明，不是工具自动验证人类身份。模型的核验结论、自动测试成功和运行成功都不等于用户采纳。首版不自动归因收益、汇总跨报告得分或重新排序通路。
