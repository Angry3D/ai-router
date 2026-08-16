# AI Router 贡献者约定

本文件面向在此仓库中修改代码或文档的自动化工具与 AI 助手。普通贡献者请先阅读
[贡献指南](./CONTRIBUTING.md)，工程背景见 [工程文档](./docs/engineering/README.md)。

## 项目边界

- AI Router 是面向 macOS 13+ Apple Silicon 的本地 Tauri 菜单栏应用。
- `crates/router-core/` 负责领域模型、SQLite、路由、回退、恢复与共享 DTO；
  `src-tauri/` 负责桌面组合、IPC 与 macOS 生命周期；`src/` 负责 React 界面。
- 本地代理只监听 loopback。不要把监听地址改成局域网或公网地址，也不要扩大 Tauri 权限。
- 官方稳定版本提供 Apple Silicon DMG 和经过项目 updater 密钥认证的应用内更新；不维护
  Windows/Linux、Developer ID 签名、Apple 公证、App Store 或静默更新路径。

## 修改规则

- 先搜索同类实现和受影响的常量、脚本、测试，再修改。
- 持久化和路由规则属于 Rust；React 通过生成的 IPC 类型消费状态，不复制业务权威。
- `src/generated/` 由 `pnpm generate:types` 生成，不手工编辑。
- 应用图标由 `src-tauri/icons/*.svg` 和 `scripts/generate-app-icons.mjs` 生成；变更来源后运行
  `pnpm icons:generate`，并保留第三方来源说明。
- 不顺带重构无关代码，不删除或覆盖他人的工作区改动。

## 数据与安全

- 任何测试、Issue、PR 或文档都不得包含真实 API Key、Authorization header、完整 Codex
  配置、SQLite 数据库、原始请求/响应、完整运行日志或用户绝对路径。
- 测试使用合成数据、临时目录和本地监听器，不访问真实上游服务。
- 不自动退出、重启、替换或重新启动 `AI Router.app` / `com.relax.airouter`。需要原生生命周期
  验证时只使用 `AI Router QA.app` / `com.relax.airouter.qa` 和隔离数据。
- 不提交或打印 updater 私钥和密码。发布配置只能从受保护的 GitHub `release` environment
  临时注入公钥，失败发布必须停留在 draft。
- 更改 Codex 配置投影、数据库恢复、回退并发、流式响应或原生生命周期前，先阅读对应的
  [工程契约](./docs/engineering/README.md)，并保持失败关闭和有界诊断。

## 验证

使用与改动风险相称的最小验证集。文档改动运行 `pnpm docs:check` 和
`git diff --check`；前端改动至少运行相关测试、`pnpm typecheck` 与 `pnpm lint`；Rust 改动至少
运行相关测试、`cargo fmt --check` 与受影响 crate 的 Clippy。跨层 DTO、数据库迁移、恢复、
路由并发或发布边界改动需要扩大到对应工程文档规定的完整检查。
