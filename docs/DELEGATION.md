# v0.2 委派：材料加工、调查与定向复核

在主模型大量阅读前委派一个边界明确的任务。主模型先获取目标与少量入口信息，选择授权材料，收回短交接后核验关键证据并修改、测试。简单搜索与小改直接在宿主完成；默认最多两个独立任务并行。

## 最小调用

```sh
multi-ai doctor
multi-ai pack --root /path/to/repo --scope src/feature --input AGENTS.md --outdir /path/to/task/packet-1
multi-ai delegate investigate kimi-cli-default \
  --packet /path/to/task/packet-1 --prompt-file /path/to/task/task.txt \
  --outdir /path/to/task/investigation --fallback grok-cli --json
```

任务文件写目标、相关规则、问题、验收标准和不需要处理的范围，不写整段宿主会话。`--input`/`--scope`/`--fallback` 可重复。相对材料路径相对于 `--root`；输出路径相对于当前目录。pack必须使用新目录；快照包含当前内容，后续源码变化需要生成新快照。默认排除凭据、构建目录、二进制和厂商扩展配置，清单显示被排除项；没有默认全仓打包。

用户本轮授权外部处理后，可在授权范围内自动调用，不逐次确认。授权某些文件不包括其他客户原件或凭据。已付订阅也消耗账户额度。

| 角色 | 用途 | skill 默认 |
| --- | --- | --- |
| investigate | 调用链、实现与契约的搜索阅读 | Kimi，明确列出 Grok 备援 |
| extract | 提取、对照与日志整理 | Kimi；可明确选择 API alias |
| challenge | 对方案、diff 检查反例与失败路径 | Grok，尽量不同家族 |

API alias复用现有模型解析器，例如 `deepseek-flash`、`glm-flash`、`kimi-k3-low`；API不能自行搜索仓库。CLI首版只支持受限Kimi/Grok适配，旧run/review通路保持原行为。Grok已被用户选入新委派流程；旧review的宿主路由偏好仍保留。来源家族与额度池分开展示，不将多个账户池称为独立认知。

## 边界与诊断

快照上限8MiB、2,000文件、单文件512KiB；API材料上限512KiB。调查的初始提示不载入源码，模型只通过同一Rust程序的内部stdio MCP服务按需list/search/read。服务验证清单、路径、哈希与读取预算，不提供shell或任意文件系统入口。默认40次工具调用、累计256KiB，超限记为未完成。

两家CLI使用私有运行home/cwd与受控agent，禁用native工具、扩展、hooks、其他MCP、网页和子agent发现。该边界依靠已验收CLI工具限制与Rust读取服务，厂商进程本身仍需读取登录配置。无需在Codex/Claude注册MCP，不是原生子agent面板功能。

`doctor`、`routes --json`显示支持角色、受检版本、配置与登录文件可用性，不把离线检查称为登录/额度健康验证。未知版本拒绝受限调查；不能退回旧run通路绕过边界。已有独立登录时优先使用，否则引用原厂商登录，私有运行目录不归档凭据；无法复用时按照[CLI隔离文档](CLI-ISOLATION.md)建立独立登录。

总预算默认300秒，重试和备援共享。只运行明确的target/fallback名单；收到正文或取消后不备援；结构/证据错误不会触发额外模型修复。发现额度问题沿用共享池冷却。

## 产物、恢复和反馈

stdout默认短交接，最多5条发现/3,500字符，显示遗漏数量、覆盖与不确定性，附完整报告。`--json`包含状态、身份、指标和产物路径。模型最终结构采用：

```json
{
  "schema_version": 1,
  "findings": [{
    "importance": "medium",
    "conclusion": "可核验的具体结论",
    "evidence": [{"path":"src/file.rs","start_line":1,"end_line":2,"sha256":"来源文件哈希","quote":"可选原文片段"}]
  }],
  "checked_scope": ["src/file.rs"],
  "uncovered_scope": [],
  "uncertainties": []
}
```

Rust校验源位置、哈希及可选引文，校验不证明推论正确。允许无发现，但必须有已检查文件。CLI还必须有至少一次成功read或返回源码内容的search；只list、失败read或无匹配search不能发布成功。API材料带原始行号前缀，引用时仍核对原文件。一次委派可能包含多轮厂商模型请求；完整报告和短交接由本地渲染，不额外总结。

整代写在 `runs/run-UUID/ATTEMPT/`；完整验证后最后原子更新 `latest.json`。读者按pointer及哈希读取同一代；失败状态在dispatch.json，保留之前latest及旧代。输出与嵌套目录冲突在发送前失败。

`--resume`复用材料、任务、模型、配置、实现一致且未损坏的完整结构；未知CLI模型身份不恢复。运行历史和反馈不参与缓存身份。反馈绑定完整report.md，而不是短交接：

```sh
multi-ai feedback --run UUID --report ATTEMPT-ID --accepted 1 --evidence '已核对指定来源与实际测试'
```

默认agent；human需要明确用户决定与 `--human-confirmed`。未掌握数量留空。通过测试或模型CONFIRMED不会自动成为人工采纳。

## 收益测量

metrics保存外部执行次数invocations、可获得的API/CLI真实usage、读取量、工具调用、交接长度、耗时和缓存。CLI内部请求数未知留null；订阅额度不是零成本。宿主token必须从宿主实际用量获得，缺失留null。字符压缩只说明交接阅读量，不能当token节省率。

验收用虚构材料做调查/提取/复核各两组成对任务，固定目标、输入、正确答案与评价标准。分别记录主模型直接处理和委派的正确性、用量、耗时及返工；缺失宿主数据时只报告可观测指标。

实现源码与本地发布包不意味着GitHub已经发布v0.2；上传由独立发布步骤完成。其它操作系统和架构尚未验收。
