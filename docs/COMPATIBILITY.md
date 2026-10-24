# 兼容说明与验收口径

首版支持并验收 macOS Apple Silicon，二进制目标为 `aarch64-apple-darwin`。其他系统或架构不据此声明支持。

## 两个旧入口（已弃用）

统一使用 `multi-ai`。`install.sh --no-legacy-aliases` 只管理新入口并保留机器上已有别名；默认安装仍兼容三个名字。`ai-review` 和 `llm-run` 的分派暂不删除，它们指向同一 Rust 二进制。程序依据调用入口保留语义：

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

默认来源为主 AI 判断，需要具体证据。人工来源还要求明确用户意见及 `--human-confirmed`；此标志是代录声明，不是工具自动验证人类身份。模型的核验结论、自动测试成功和运行成功都不等于用户采纳。不自动归因收益。2026-10-18 新增的 scorecard 按报告反馈汇总，recommend 给显式建议，只有 batch route:auto 应用建议；旧命令默认排序不变。
# v0.2 增量兼容

新增 `pack` 和 `delegate`；旧 run/review/text、旧别名及秒/毫秒语义保留。delegate的timeout为秒，初始300秒涵盖重试和备援。新命令只使用明确授权快照与target/fallback名单；受限CLI能力与旧agentic标志独立，未知版本不启用调查。routes/doctor新增delegation能力字段，不发送模型请求。

新委派整代产物通过latest.json指针读取，失败保留上一代。feedback仍绑定完整报告哈希。v0.2实现版本参与缓存身份，升级后旧缓存可能重新调用。invocations指外部适配执行次数，CLI内部模型请求不等于一次。

详情：[委派指南](DELEGATION.md)。其它架构与系统尚未声明支持。

# 2026-10-18 增量

新增 batch、stats 和 delegate implement codex-cli；run 可显式 --fallback，quota 与 rate_limit 分开，run/review/verify/batch 共享限流重试。原 run/review/verify 秒单位、text/llm-run 毫秒单位不变。batch恢复仍要求完整身份和正文哈希；实施有副作用，不提供resume，不自动提交。实施使用 Codex 原生 workspace-write 加事后 Git/mtime 核对，不套外层系统 sandbox，默认不覆盖网络配置；显式 --network 追加 sandbox_workspace_write.network_access=true，允许任意网络地址（包括远端），status 记录 network 布尔值；归因越界或 Claude 配置变化记为 boundary_violation，其余外部快照变化仅警告，只报告不回滚，监测范围与盲区见 [README](../README.md#受控实施)。旧技能模板已移除，技能安装与升级只面向 multi-ai，历史rollback仍可使用。


## 打分、选路与宿主反馈增量（2026-10-18）

run/review/batch/delegate 新增 task-type，历史缺失保持 null。金丝雀核对使用完整报告，source=canary 与 agent/human 精度反馈分开；不将模型自评或缓存命中视为新样本。TSV 导入预检所有报告身份和正文哈希，更正追加保存、按字段取最后已知值。batch stdout JSON 保留，ID 汇总表打印到 stderr；routes 提前关闭管道正常退出。

scorecard 的时间单位为秒、screen-min 为分钟，比率为 0～1；旧 text/llm-run 毫秒参数不变。recommend 只使用当前模型/CLI身份的充分样本，冷却和本地缺失通路排除；auto 只影响显式选择它的 batch 项。失败保留完整报告的规则继续适用；实施本身成功但检测到归因越界或 Claude 配置变化时，可发布顶部带警告的诊断摘要，仍为 complete=false、非零退出且不作成功缓存；完整诊断摘要同样登记为可反馈报告。边界快照不扫描构建树、Claude 会话及时间重叠的其他受管工作树；排除清单可审计，范围外变化仍无法证明没有发生。

CLI 身份检查在 cwd 位于 HOME 内时，祖先配置发现到 HOME 为止；隔离 HOME 的确定性夹具不继承其外侧的宿主配置。显式配置路径仍参与身份指纹。

成绩口径、低样本门槛和外包判据见 [README](../README.md#怎么选模型怎么给分)。

## 实施归因、反馈和重算增量（2026-10-21）

成功状态继续为 `ok`、`complete=true`、退出 0，未增加 complete_with_warnings，旧读者可继续按原成功值判断。新增 attribution、boundary_warnings、boundary_buckets、boundary_violation_count、attributed_boundary_count、boundary_details、summary_kind。每桶只保留计数和最多 20 条路径，完整清单在 artifacts。summary_published 与 summary_kind 分别表示本轮有无发布、正式/诊断；诊断完整正文可绑定哈希录反馈，仍不作为成功恢复缓存。

本机离线检查：codex-cli **0.163.0** 的 `exec --help` 确认 `--json` 输出 JSONL；安装二进制内的 exec 序列化字面量确认 `thread.started`、`turn.started`、`turn.completed`、`item.completed`、`file_change`、`changes`、`path`、`add/delete/update`。解析支持 `item.completed` 的 `item.type=file_change`、`item.changes[].path`（绝对或相对 cwd）、`kind`、`item.status`。thread.started 与 turn.completed 完整且无损坏行时 attribution=available；部分流仍保留已经观测到的越界编辑证据，并标 unavailable。已用假 Codex 合成流验证，**本轮遵守“不运行其他实施器”，未启动真实 exec 会话，真实事件形状尚需宿主安装后验证**。真实流未必带 model；此时使用共用配置解析器，取不到保持 unknown。

边界默认范围为 git worktree list 的注册工作树、共享元数据和指定配置路径，可重复 --watch。git_ignored 使用所属仓 git check-ignore（批量 NUL 路径），git_internal 除 config/hooks，nested_worktree 包括嵌入的 .claude/worktrees；只有归因越界与 claude_config（含旧 .agents 保护）拒绝通过；2026-10-24 起 claude_config 只含沙箱可写到（cwd、/tmp、TMPDIR，解析符号链接后）的变化，其余记 claude_config_outside_sandbox 警告。配置根为符号链接时也扫描解析后的根；内部链接只看链接本身。快照 entries/elapsed_ms 记录规模和耗时，stderr 统一收尾行见 README。缺流时仅快照警告不能当作成功归因证明。

实施字段：--outcome merged|merged-after-rework|discarded、--rework-items N、--rework-rounds N、--defect-tags tag1,tag2；TSV 可选列为 outcome/rework_items/rework_rounds/defect_tags/screen_min/rework_min。旧反馈字段和追加更正规则不变；未知数留空，source=agent，human 仍需用户明确决定。实施成绩单不给 precision，以实际 outcome 和返工数据选路，归因和警告均不进入执行失败率；底层同时失败时仍按执行原因计入。旧缺字段记录照常可读；不同历史 model 单列，未知模型不按当前配置填充。旧记录离线重判脚本、严格判据和恢复限制见 [README](../README.md#实施反馈与旧记录重判2026-10-21)，发布包附 reclassify-implement.py。

CLI 配置身份发现受工作树根和 HOME 约束；cwd 不在 HOME 下时只发现 cwd 项目配置和显式用户配置，避免隔离 HOME 的夹具继承其外侧宿主配置。多个配置模型冲突时保守 unknown，routes 展示与执行使用同一解析器。

# v0.2.1 增量（2026-10-24）

不改既有输出语义与单位；新增均为可选开关或新字段。

- 失败分类新增 `model`（通路模型 id 被拒），不写池冷却，run/batch 视同可备援。
- batch 与 delegate implement 新增 `--preflight` / `--preflight-timeout`；默认关闭，行为与 0.2.0 一致。
- 实施边界：沙箱写不到的 Claude 配置变化改记 `boundary_warnings.claude_config_outside_sandbox`，不再判 `boundary_violation`。
- 运行目录新增 `alive.lock`，manifest 新增 `pid`；stats/scorecard 把进程已不在的未结束运行记 `lost`。
- 无 `--out` 的运行结束时 manifest `current_status` 不再停在 `running`。
- scorecard 表格新增 MODEL 列与样本不足原因（JSON `assessment_missing`）；缺陷标签读写共用同义表。
- batch 在 outdir 写 `verdicts.template.tsv`；`feedback --batch` 跳过除 id 外全空的行。
- 新离线脚本 `backfill-codex-model.py`，默认预演。
- 子命令 `--help`（及 `help CMD`）列出与参数校验同一张表的可用参数。
- 新离线命令 `consensus`：多份复核报告按 文件:行号 对照一致与分歧。
