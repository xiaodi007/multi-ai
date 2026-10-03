# multi-ai 协作入口

用中文沟通。先执行 `git status --short`，保留未知修改；阅读 README，再按需读取 `docs/COMPATIBILITY.md` 和 `docs/AGENT-INTEGRATION.md`。本仓库独立于业务项目，不导入其业务文件或凭据。

本工具共用 Rust 原生 API、CLI 调度、配置、输出生命周期和反馈实现；`ai-review` 与 `llm-run` 是兼容入口。修改时保留旧别名和输出语义，以及 run/review/verify 的秒单位、text/llm-run 的毫秒单位。模型展示与执行必须共用解析器。

输出冲突须在调用模型前失败；完整报告原子发布，失败保留上一份完整结果。恢复只复用身份和正文哈希一致的完整产物。未知模型身份、partial 或损坏缓存不能视为成功。反馈按报告绑定哈希、追加保存，未知数量留空；默认 source=agent，human 需要用户明确决定。

只在明确授权时运行真实外部模型，使用指定范围的材料；确定性测试不能联网或读取用户凭据。不打印、提交或上传 API key、登录文件、真实报告、客户原件、编译缓存。CLI 是可选依赖，其缺失只影响相应通路。

按修改范围验证：

```sh
cargo fmt --all -- --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
sh tests/test-agent-skills.sh
```

目前只验收 macOS Apple Silicon。发布包由 `scripts/package.sh` 生成到被忽略的 dist；更改发布资产时同步校验和。技能模板在 `integration/skills/multi-ai`，二进制安装与技能安装分别验证，保留已存在配置和用户修改。

未经用户明确要求，不提交、推送或创建发布。功能开发分支使用 `codex/` 前缀。
