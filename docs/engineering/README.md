# AI Router 工程文档

这里记录外部贡献者需要遵守的项目契约。它描述当前 `0.1.0` 源码，而不是未来路线图。

- [架构与代码边界](./architecture.md)：进程、模块、状态和生成类型的所有权。
- [路由与韧性](./routing-resilience.md)：Responses 转发、自动回退和诊断边界。
- [数据、隐私与恢复](./data-privacy-recovery.md)：本地文件、密钥、历史、日志和恢复点。
- [macOS 原生生命周期](./native-lifecycle.md)：生产/QA 隔离、菜单面板和构建安全。
- [风险对应的验证策略](./verification.md)：按改动影响选择最小充分证据。
- [GitHub CI 与安全设置](./github-security-settings.md)：required checks、ruleset 与安全功能交接清单。

项目支持范围、构建入口和协作规则分别见 [README](../../README.md)、
[贡献指南](../../CONTRIBUTING.md) 和 [安全政策](../../SECURITY.md)。
