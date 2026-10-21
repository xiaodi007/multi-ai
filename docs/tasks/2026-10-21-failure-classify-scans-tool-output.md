# 失败分类扫了工具输出：容量满被判成认证失败、额度池冷却 4 小时

- 状态：待做
- 来由：keel 2026-10-21 验收后续，`delegate implement codex-cli` 门禁泳道 60 秒即 `status=auth exit=1`（run 5a7f3394）
- 负责人：待派

## 事实（已复算）

1. Codex 实际失败原因在 stdout 末尾两行：`{"type":"error","message":"Selected model is at capacity. Please try a different model."}`、`{"type":"turn.failed",…}`。这是服务端容量满，属于暂时性失败。
2. `review::classify` 在 CLI 退出后，把 `stderr + stdout` 整段拿去按顺序匹配 error／quota／rate_limit／auth／transport 的正则（`src/review.rs:196` 起；调用点 `src/review.rs:1064`、`src/delegate_cli.rs:1005`）。
   - 「at capacity」不在任何一类里。
   - stdout 是 Codex 的 JSONL，里面包含 `item.completed` 的 `command_execution.aggregated_output`，也就是模型读到的仓库文件原文。keel 任务卡表格里的 `SEED-FORMAL-CLI-TEST-CONFIG-FORBIDDEN` 命中 auth 正则里的 `forbidden` ⇒ 判成 `auth`。
3. `auth` 触发 `cool_down(pool,"auth")`（`src/review.rs:373`），默认 240 分钟。之后 4 小时内经 multi-ai 派给 codex 的任务都会被当成冷却中；宿主当天只能绕过 multi-ai 直接调 `codex exec`。
4. 同类风险：仓库里任何出现 `401`、`403`、`forbidden`、`authentication`、`429`、`network`、`proxy`、`EOF` 的文件，被模型 `cat` 一下就可能改写失败分类；`transport` 里的 `EOF`、`network` 尤其常见。

## 要做

1. **只扫诊断信道，不扫工具输出。** 按厂商解析：
   - codex：只取 JSONL 里 `type=="error"`、`turn.failed.error.message` 与 stderr。`item.*` 事件（含 `aggregated_output`、`agent_message`）一律不参与分类。
   - grok／kimi 等：已有 `parse_stdout` 时同样只取它识别出的错误字段；识别不了的再退回整段，但先剔除工具结果块。
   - API 路径保持现状（它本来只看响应体）。
2. **新增「容量／过载」类**：`at capacity`、`overloaded`、`server is busy`、`503`、`try a different model`、`temporarily unavailable`。按 rate_limit 处理：退避重试、可换同池备选模型，**不**进 auth 冷却。名字可沿用 `rate_limit` 或新开 `capacity`，成绩单与 recommend 的失败率要能区分。
3. **auth 正则收紧**：数字和词都加边界（`\b401\b`、`\bforbidden\b`），并要求出现在第 1 步选出的诊断文本里。
4. **auth 冷却要有二次确认**：auth 判定后先做一次廉价登录探测（`doctor --live` 同款），探测通过就降级为 `error`、不冷却；或者至少把冷却从 240 分钟降到首轮 10 分钟、连续两次才拉长。
5. **用例**：
   - 回放本次 stdout（脱敏后放 `tests/fixtures/`：只留含 `FORBIDDEN` 的 `aggregated_output` 行和末尾两行 error）⇒ 期望 `capacity`／`rate_limit`，不是 `auth`，且不写冷却文件。
   - 工具输出含 `401 Unauthorized` 文本、但 error 事件是普通错误 ⇒ 期望 `error`。
   - error 事件本身是 `401 Unauthorized` ⇒ 仍判 `auth`。
6. **补救工具**：`multi-ai cooldown --clear POOL`（若已有就写进 README），这次这种误冷却能手动解除。

## 不做

- 不改 review／verify 的席位调度规则。
- 不在分类里猜模型意图；只认厂商给的错误字段。

## 验收

`cargo test` 全绿，新增的三条回放用例在旧实现上先红；`multi-ai stats --json` 能看到新分类计数；README 的失败分类表同步。
