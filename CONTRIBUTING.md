# 贡献指南

感谢你改进 AI Router。项目仍处于早期阶段，公共协作以小范围、可验证、不会暴露用户
数据的变更为主。

## 开发环境

支持环境为 macOS 13+ Apple Silicon。使用 Xcode Command Line Tools、Node `22.22.3`、pnpm
`10.33.2` 和 Rust/Cargo `1.97.1`。完整安装步骤见 [README](./README.md#构建前提)。

```sh
nvm use
corepack enable
corepack prepare pnpm@10.33.2 --activate
pnpm install --frozen-lockfile
```

不要更新锁文件来掩盖安装失败；只有依赖变更本身属于 PR 范围时才更新 `pnpm-lock.yaml` 或
`Cargo.lock`。

## 代码边界

- `crates/router-core/`：领域校验、SQLite、路由/回退、Responses 转发、Codex 配置与恢复。
- `src-tauri/`：Tauri IPC、桌面服务组合、日志与 macOS 窗口/托盘生命周期。
- `src/`：React 菜单和设置界面；Rust 生成类型在 `src/generated/`。
- `scripts/`：版本、类型、图标、许可证、公共源码检查和 QA 工具。
- `docs/engineering/`：外部贡献者需要遵守的工程契约。

详细说明从 [工程文档索引](./docs/engineering/README.md) 开始。

## 提交变更

1. 先创建聚焦的分支，并确认工作区中没有被你的操作覆盖的他人改动。
2. 在实现前搜索同类代码、常量、生成链和测试。一个业务规则只保留一个权威来源。
3. 为可观察行为添加能在修复前失败、修复后通过的聚焦测试。
4. 按改动风险选择验证，不为文档或局部改动无条件运行原生全套验收。
5. PR 说明应写清行为变化、风险、验证命令和未覆盖边界。

不要在 PR、Issue、测试 fixture、截图或提交历史中放入真实 API Key、Authorization header、完整
Codex 配置、SQLite 数据库、原始请求/响应、完整运行日志或用户绝对路径。演示和测试只使用明显
虚构的合成数据。

## 权威命令

| 改动             | 最小检查起点                                                   |
| ---------------- | -------------------------------------------------------------- |
| 仅文档           | `pnpm docs:check`、`git diff --check`                          |
| React/TypeScript | 聚焦 Vitest、`pnpm typecheck`、`pnpm lint`                     |
| Rust 单模块      | 聚焦 `cargo test`、`cargo fmt --check`、受影响 crate 的 Clippy |
| 生成 DTO         | `pnpm generate:types`，检查生成 diff，再运行前后端相关检查     |
| Web 构建/配置    | `pnpm build`                                                   |
| 版本元数据       | `pnpm version:check`                                           |
| CI/供应链配置    | `pnpm ci:policy`、`pnpm security:public:check`                 |
| 生成类型与协议   | `pnpm contracts:check`、`pnpm check:codex-retries`             |
| 依赖或第三方内容 | `pnpm license:public:check`                                    |
| 生产源码包       | `pnpm tauri:prod:build`                                        |
| 发布脚本/工作流  | 聚焦脚本测试、`pnpm ci:policy`、发布工程契约                   |

完整前端和 Rust 基线命令是：

```sh
pnpm docs:check
pnpm ci:policy
pnpm security:public:check
pnpm license:public:check
pnpm lint
pnpm typecheck
pnpm test
pnpm build
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

聚焦前端测试使用 `pnpm exec vitest run <test-file>`。跨层 DTO、数据库迁移、恢复、回退并发、流式
响应、原生生命周期或发布边界变更，才需要扩大到 [验证文档](./docs/engineering/verification.md)
中的完整范围。

## 生成文件

- 修改 Rust 导出的 IPC DTO 后运行 `pnpm generate:types`；不要手工编辑 `src/generated/`。
- 修改应用或托盘 SVG 来源后运行 `pnpm icons:generate`，保留生成的 PNG/ICNS 与来源记录。
- 修改版本时先运行 `pnpm version:sync`，再运行 `pnpm version:check` 并审查所有变更。
- 修改第三方 fixture、图标或定价快照时更新相邻来源记录、声明和许可证策略。

## 发布边界

普通贡献者和 pull request workflow 不接触 updater 签名密钥，也不创建或修改 GitHub Release。
`pnpm tauri:prod:build` 与 `pnpm tauri:source:build` 只生成无官方更新凭据的 `.app`；可分发的 DMG、
updater 归档与签名只由受保护的 tag workflow 构建。不要在本地命令、测试、日志、PR 或 Issue 中
传入真实 `TAURI_SIGNING_PRIVATE_KEY`、其密码或其他 release environment 值。

发布行为的修改必须覆盖 tag/ref/version 绑定、draft-only 修复、资产清单、ad-hoc bundle 签名、updater
签名、校验和与 provenance，并同步 [稳定版本发布操作](./docs/engineering/releasing.md)。

## 原生验证安全

正在运行的 `AI Router.app` 可能承载当前 Codex 会话。自动化不得退出、信号、重启、替换或重新启动
生产 bundle。开发和生命周期测试使用 `pnpm tauri:qa:dev` 或 `pnpm tauri:qa:build`，并在任何
破坏性操作前验证 bundle 为 `AI Router QA.app`、identifier 为 `com.relax.airouter.qa`。不要把生产
路由、密钥、数据库、日志或 Codex 配置复制到 QA。

## Issue 与 PR

普通缺陷使用 Bug 表单，功能建议使用 Feature 表单。安全问题按 [SECURITY.md](./SECURITY.md)
私下报告。维护者会根据范围、复现质量、风险和可维护性评审；提交 PR 不代表一定合并。
