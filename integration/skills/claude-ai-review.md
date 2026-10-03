---
name: ai-review
description: 按额度池并行分发复核材料，失败时补备援；用于有价值的第二意见、交叉复核和通路失效切换。ai-review 是 multi-ai Rust 原生工具的兼容入口。报告数、配置家族数和额度池数分别展示，不自动认定认知独立。
---

# ai-review —— 多模型复核分发器

`ai-review`、`llm-run` 与 `multi-ai` 现在调用同一个 Rust 原生二进制。新入口使用 `multi-ai run <明确通路名>`、`multi-ai review`、`multi-ai verify`；旧命令含义保留。API 通路不需要 Python、Node 或 uv；厂商 CLI 仍需安装和登录。源码、安装与回退见 [multi-ai 仓库](https://github.com/xiaodi007/multi-ai)，边界见仓库的 `docs/COMPATIBILITY.md`；私有仓库需要访问权限。

新 AI 先运行 `ai-review help` 读离线短指南（配方、材料、进度、失败、恢复及本机通路），再用 `ai-review help run|dispatch|verify` 查看对应子命令的当前参数。帮助不探测账户、不调用模型、不写运行产物；完整技能仍在本文件。

## 两条硬约定

**① 调度按 `rank`，小的先用。** 默认顺序见下表，可在自己的 `routes.json` 中配置。`rank` 与 `cost` 是两件事：`cost` 只分「订阅 / 按量计费」，管的是 `--no-metered`；`rank` 是用户的路由偏好，不是实际账单价格。所以默认的 `kimi` 虽是订阅档，仍排在计费标注的 `glm`/`deepseek` 后面。

**② 备援按额度池切换。** 不同额度池并不证明模型认知独立；报告数、配置家族数和额度池数分别计数。 `claude-ds` 和 `llm-run deepseek` 共用同一个 DeepSeek key —— 一个撞额度另一个必然也撞，切过去等于没切。

| rank | provider | pool | 钱 | agentic | 说明 |
|---|---|---|---|---|---|
| 10 | `agy` | `google` | 订阅 | ✅ | Gemini 3.8 Flash High，走 **Antigravity CLI**，见下 |
| 20 | `grok` | `xai` | 订阅 | ✅ | 跟随 Grok CLI 默认模型，具体模型未知；需要登录态 |
| 25 | `codex` | `openai` | 订阅 | ✅ | ChatGPT 账户额度 |
| 30 | `glm` | `zhipu` | 💳 计费 | ❌ | GLM-5.3，纯文本；做**对抗性验证**很称职 |
| 40 | `deepseek` | `deepseek-api` | 💳 计费 | ❌ | |
| 45 | `claude-ds` | `deepseek-api` | 💳 计费 | ✅ | 与 `deepseek` **同池**；agentic 烧 token 最多 |
| 50 | `kimi` | `kimi-code` | 订阅 | ✅ | Kimi Code 订阅（`api.kimi.com`） |
| 55 | `kimi-k3` | `kimi-code` | 订阅 | ✅ | K3、1M 上下文，与 `kimi` **同池**（重活首选） |
| 60 | `kimi-k3-api` | `moonshot-api` | 💳 计费 | ❌ | Moonshot API，与 `kimi` CLI **不同池** |

`dispatch` 每池只取一个席位，`--only` 也遵守同池去重。默认 `--kind review` 是 2 席，按 rank／额度挑选；rank 并不保证先用完订阅池才用计费 API。`--no-metered` 才是禁用当前 cost 标为计费的通路。上表是本地调度标注，不代替账户账单；本轮不改 rank／cost。

### 额度能看见多少（2026-09-23 实测）

`ai-review usage` 把查得到的剩余和重置时间记到 `~/.local/state/multi-ai/review/quota.json`。`dispatch` 仍按 rank 取席。只有 5 小时和周额度都在、最紧一档低于 40%、并且另有一份订阅额度高于 70%，这一池才让到后面。缺窗口的记录（例如 Codex 只有周额度）只展示，不改次序。Kimi 仍排在贵的那头，不因为剩余多就先用。

撞额度时：报错里若有具体钟点（`try again at …`），冷却到那个钟点，可以短于 60 分钟。没有钟点时，若快照里已有低于 40% 的一档，冷却到该档重置。没有这样的档，或对方只说「retry after 1 second」，才回落到 60 分钟。

| 池 | 怎么查 | 选池用不用 |
|---|---|---|
| `google`（agy） | `agy -p "/usage" --output-format json`，打印模式直接回答，不扣额度。Gemini 周 / 5h 各有剩余比例和重置时间 | 用 |
| `kimi-code` | 交互里的 `/usage`。`kimi -p "/usage"` **不会**执行这条命令，会当成提示词发给模型。工具改打 CLI 自己用的 `GET https://api.kimi.com/coding/v1/usages` | 用 |
| `openai`（codex） | 没有不花钱的子命令。每次会话把 `rate_limits` 写进 `~/.codex/sessions/**/rollout-*.jsonl`。目前这条记录没有 5 小时窗口 | 只展示，不改选池 |
| `deepseek-api` / `moonshot-api` | 公开余额接口，是钱不是时间窗 | 只展示，不改 rank |
| `zhipu` | `/v4/usage`、`/v4/account/balance` 仍是 404 | 不用 |
| `xai`（grok） | `grok usage` 要会话 ID，是这一次的 token，不是账户额度 | 不用 |

Claude Code 自己的 5 小时 / 7 天额度只在会话状态栏里下发（`used_percentage`、`resets_at`），没有独立子命令。`ccusage` 加总的是本机记录，不是官方剩余。它不是 `ai-review` 的一个席位。

### Gemini 这条走 `agy`，不是 `gemini` CLI

此工具的 Gemini 适配器调用 Antigravity `agy`，不会调用 `gemini` CLI。用户需自行安装并登录 AGY，以 `multi-ai doctor` 检查可执行文件，再用自己的账号进行一次小请求验收。其他客户端的登录状态不等于 AGY 可用。

若需要不同家族的意见，先用 `agy models` 和实际配置确认所选模型；AGY 能列出的非 Gemini 模型不能自动计为 Gemini 家族。即使模型家族不同，也不能保证判断独立。

当前 `agy` 默认固定为 `gemini-3.8-flash-high`；升级前先用 `agy models` 确认新模型确已可用，不跟随非 Gemini 条目。

## 三个配方（按省 token 的程度排）

### 配方 A · 日常小改：只外包探索

我最贵的不是「读代码」，是读完还要**逐条验回去**。小改动直接把取证外包，我只读证据包：

```bash
ai-review dispatch --kind explore --prompt-file 探索任务.md --cwd <repo> --outdir out/
```

`--kind explore` 只派 **1** 个能读仓库的订阅席，按 rank 挑额度不紧的那家。

探索任务书必须写死三件事，否则拿回来的是结论不是证据：**① 每条事实都要 `文件:行号` + 原文引用；② 只给事实，不给建议不给评价；③ 单列「没查到的」和「我不确定的」两节。** 拿到 40 行证据包后我抽验 1-2 条（有行号，很便宜），再决策。

例如探索一个虚构项目的小计函数：要求列出函数定义、调用点及测试的路径与摘录，再由主 AI 对照原代码抽验；模型总结不能替代这些证据。

### 配方 B · 高风险改动：并联复核 + 交叉印证

```bash
ai-review dispatch --kind review --prompt-file 复核任务.md --outdir out/
```

默认 2 席。材料只放需求、契约和 diff，不放实施结论或任务卡。任何可核验发现都值得检查；多家一致仍须回到原始代码或证据核实。

若任务书没有包含正文，用重复的 `--input 契约.md --input 差量.diff` 真正附带证据。纯文本通路不能读取 `--cwd`：有 cwd 的单次文本调用必须附 `--input` 或显式 `--text-ready`；分发器遇到仅给 cwd 的任务自动只选可读仓库通路。

例如材料要求 `subtotal(120, 3)` 返回 `360`，代码却使用加法；核验时自行复算实际值 `123`，再确认修复表达式和测试。两家都指出同一缺陷仍只是一条需要核验的发现。

### 配方 C · 大批量：分片复核 + 对抗验证（最省）

```bash
# 第一轮：分片，各管一块
ai-review run grok --prompt-file 复核-模块A.md --cwd <repo> --out A.md
ai-review run agy  --prompt-file 复核-模块B.md --cwd <repo> --out B.md

# 第二轮：交叉验证 —— 谁出的发现，就排除谁
ai-review verify --findings A.md --exclude grok --cwd <repo> --out A-matrix.md
ai-review verify --findings B.md --exclude agy  --cwd <repo> --out B-matrix.md
```

`verify` 让验证者**独立取证**后逐条判 `CONFIRMED / REFUTED / UNCLEAR`，末尾给计数。主 AI 按风险核对关键证据；`CONFIRMED` 是模型判断，不能自动当成已修复、采纳或用户确认。实际核验后可录入报告级 `agent` 反馈。

⚠️ 两家一致 ≠ 对，可能集体漏。所以无论如何**随机抽 1 条自己复算**作兜底。

## 用法

```bash
ai-review help                       # 离线学习指南
ai-review help dispatch              # 当前参数与配方，与 dispatch --help 共用 parser
ai-review probe --only kimi glm --timeout 90  # 隔离空目录，用小请求核实这两条通路
ai-review usage                      # 各池 rank + 真实余额 + 冷却状态
ai-review verify --findings A.md --exclude <出具方> --cwd <repo>   # 交叉验证，见配方 C
ai-review status                     # 哪个池在冷却、还剩多久、哪些是计费的
ai-review dispatch --kind review --prompt-file 材料.md --outdir out/
ai-review dispatch --kind explore --prompt-file 材料.md --dry-run   # 先看会派给谁
ai-review dispatch --prompt-file 材料.md --seats 5 --dry-run   # 席位数盖过 kind 的默认值
ai-review run grok --prompt-file 材料.md --out r.md
ai-review reset --all                # 清冷却（确认额度恢复后）
```

常用参数：`--kind explore|review|verify`（默认 review，2 席；explore 是 1 个订阅 agentic 席；verify 是 1 席，须 `--exclude` 出具方）。`--seats N` 盖过默认席位数；`--only` 不带 seats 时以所列额度池为目标。目标席位不足也退出非零。`--no-metered` 禁用当前计费标注；`--dry-run` 不派发复核，但保留旧额度采集行为，可能联网；纯离线使用 `multi-ai routes`／`doctor`；`--agentic` 只用能自己读仓库的；`--exclude` 连同出具方的同池别名一起排除。Codex 宿主不得自动选择 Grok，除非用户明确点名。

时间参数单位为秒：`--timeout 900` 是子进程总预算；`--headers-timeout 180` 是 API 等响应头上限；`--idle-timeout 120` 是无子进程输出上限。CLI idle 默认为 0（只观测，不因安静思考终止），API 内层默认仍有数据空闲上限。`--max-tokens` 仅控制 API 的输出预算，思考也会占用；不要靠延长总超时解决正文预算不足。

`--out FILE` 的进度在 `FILE.progress/run-*/<provider>-*/`，分发 `--outdir DIR` 的进度在 `DIR/runs/`；不指定输出则在 `~/.local/state/multi-ai/progress/`。双管道 raw、事件、时间 metadata 实时写入私有文件，API 另有 transport 子目录和正文 partial。每 15 秒有耗时／距最后输出的提示。失败现场不删除；只清理临时请求文件。Kimi 的 stream-json 是消息事件，不保证逐 token 返回。

`--resume` 需要 out/outdir，只复用同 prompt／材料／源码／配置／工具版本且哈希完整的成功报告；partial 永不复用。Git agentic 任务识别 HEAD 和当前已跟踪／未跟踪源码，输出目录不参与；非 Git 或工作区外符号链接需维护者提供覆盖其内容的不可变 `--source-id`。首次调用无此身份仍可运行，但不缓存。`FILE.status.json`／`DIR/dispatch.json` 记录本轮状态与席位结果；失败可能保留旧报告，不能凭文件存在判成功。恢复不会从 partial 接着生成；失败部分仍需重跑。

## 失败怎么处理

| 分类 | 判定 | 动作 |
|---|---|---|
| `quota` | 429/402/余额/限额 | 冷却到报错里的钟点；没有钟点时，冷却到快照里已低于 40% 的那一档的重置。都没有才 60 分钟。然后换池 |
| `auth` | 401/403/未登录 | 整池冷却 240 分钟（要人工重登），换下一池 |
| `timeout` | 响应头／无数据／总预算超时 | 不冷却，立即补位；保留阶段时间，不能单凭超时认定材料过大或登录失败 |
| `transport` | 连接类 | 冷却 5 分钟，换下一池 |
| `truncated` / `incomplete` / `empty` | 输出预算用尽、流缺终态、只有思考或无最终正文 | 不计完成、不缓存；保留 partial，立即补位；拆材料或调整正文预算 |
| `error` | 其他 | 不换 —— 多半是 prompt 本身有问题，换了也一样 |

冷却状态落盘在 `~/.local/state/multi-ai/review/state.json`；旧状态首次读取兼容保留，跨会话有效。

失败席位一结束就安排备援，同时其他席位继续运行；不等本轮最慢通路收尾。小请求先核实登录和通路，元数据中的 access token 过期不等于 CLI 自动刷新后仍不可用。

## 要守的纪律

- **材料授权在调用之前**。默认不主动搜集或外发客户原件；维护者明确指定外部处理的材料即授权该范围，不重复要求脱敏审计。凭据不外发，客户原件不进入项目 Git；未指定材料不自动纳入。
- **独立性靠输入控制**，不靠模型不同。复核方只给原始需求 + 契约 + diff，**不给实施方的结论和任务卡** —— 否则它只会顺着你的话说。
- **判据是事实源，不是票数**。三家都说对不等于对；涉及外部 API 字段、版本号、官方行为的主张逐条去官方资料核实。
- 长任务给宿主工具足够的总预算并使用可返回进度的后台／分段等待方式；外层调用的超时可能先于模型结束。

## GOTCHAS

- **外层进程被终止不等于模型网络故障**。先区分宿主调用预算、CLI 总预算及 API 响应头／空闲预算，结合本轮 metadata 判断失败阶段。
- **macOS 没有 `timeout` 命令**。`multi-ai` 用 Tokio 并行读取两个输出管道，超时终止整个进程组；即便管道提前关闭，总预算仍生效。
- API 席位（`deepseek`/`kimi-k3-api`/`glm`）在 Rust 程序内共享原生 API 客户端，显式附带材料；别名尊重 out/json，并验证完整终态。
- `ai-review kimi` 与 `ai-review kimi-k3` **同池**；`ai-review kimi-k3-api` 和 `llm-run kimi-k3` 是另一个 Moonshot API 池。不能把 CLI 两个模型当两份独立席位。

相关：`multi-llm-pipeline` skill 管纯文本通道的 task/domain/seat 路由细节。项目自己的归档、交叉质询和决策记录由主 AI 按项目规则执行；此发布包不安装额外的 `/round` 命令。


## 原生入口与输出反馈

当前路由以 `multi-ai routes --json` 为准。新入口通路名：`grok-cli`、`codex-cli`、`agy-cli`、`kimi-cli-default`、`kimi-k3-cli`、`deepseek-pro-api`、`glm-api`、`kimi-k3-api`、`claude-deepseek-cli`。旧 `ai-review run kimi-k3` 是 CLI；`llm-run kimi-k3` 是 API；不要因同名把两者混用。CLI 默认模型无法确认时显示跟随 CLI 默认／未知，不猜模型版本。

输出锁覆盖报告、状态、缓存和派生目录；同路径及目录重叠立即拒绝。顺序重用自动归档，失败保留上一份完整报告，同时查看本轮 status／dispatch.json。历史和原始响应不清理，旧引擎缓存迁移首次可能重新请求。厂商 CLI 收到取消或超时后终止进程组；SIGKILL 只保证系统释放输出锁。

主 AI 在实际核验后从 status／manifest 取 run 与 attempt ID 代录：

```sh
multi-ai feedback --run RUN-UUID --report ATTEMPT-ID --accepted 2 \
  --false-positive 1 --duplicate 0 --evidence '对照代码及测试的核验依据：…'
```

不知道的数量或筛选／返工时间留空。默认来源 `agent`；只有用户明确决定才可写 `--source human --human-confirmed` 并注明意见。模型 `CONFIRMED` 与测试通过不会自动变成人工采纳。绑定报告哈希，追加保存；更正新增记录，首版不自动改调度排名。
