# AI Router

AI Router 是一个面向中文 macOS 个人开发者的本地菜单栏应用，用于在 Codex CLI 或 Codex App
与多个兼容 OpenAI Responses API 的上游之间选择路由。它提供路由切换、余额查询、自动回退、
自定义 Codex 模型目录、图片生成 MCP、请求用量统计和本地配置恢复。

当前版本为 `0.1.0`，仍处于早期开发阶段。首次公开发布只提供简体中文源码，适合愿意检查并自行
构建代码的用户，不应视为稳定的通用网络代理或企业级网关。

## 支持范围

| 项目     | 当前边界                                                                              |
| -------- | ------------------------------------------------------------------------------------- |
| 操作系统 | macOS 13 或更高版本                                                                   |
| 硬件     | Apple Silicon (`aarch64`)                                                             |
| Codex    | 需要已安装并可运行的 Codex CLI 或 Codex App；自动兼容性检查固定为 `codex-cli 0.147.0` |
| 界面语言 | 简体中文                                                                              |
| 分发     | 仅提供源码；不签名、不公证；无官方二进制、安装器、App Store 或自动更新                |
| 网络边界 | 本地代理仅监听 loopback；上游请求按用户配置的 Responses API 地址发出                  |

未列出的 Codex 版本可能可以工作，但当前没有兼容性保证。Windows、Linux 和 Intel Mac 不在本次
支持范围内。完整边界见 [支持说明](./SUPPORT.md)。

## 构建前提

准备一台受支持的 Mac，并安装：

- Xcode Command Line Tools：`xcode-select --install`
- Node.js `22.22.3`，推荐通过 `nvm` 读取仓库中的 `.nvmrc`
- pnpm `10.33.2`，通过 Corepack 激活
- Rust/Cargo `1.97.1`；仓库中的 `rust-toolchain.toml` 会选择工具链和
  `aarch64-apple-darwin` target
- 可正常启动的 Codex CLI 或 Codex App

## 从源码开始

```sh
git clone https://github.com/Angry3D/ai-router.git
cd ai-router

nvm install
nvm use
corepack enable
corepack prepare pnpm@10.33.2 --activate
pnpm install --frozen-lockfile
```

先验证源码和文档契约：

```sh
pnpm docs:check
pnpm lint
pnpm typecheck
pnpm test
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
pnpm build
```

构建 macOS 应用：

```sh
pnpm tauri:prod:build
```

产物位于 `target/release/bundle/macos/AI Router.app`。从 Finder 启动该应用，菜单栏出现 AI Router
图标后，先在“设置”中新增一条路由，再明确执行 Codex 连接操作。连接会受保护地更新
`~/.codex/config.toml` 的当前 provider 传输字段；断开会恢复保存的恢复目标。首次连接前请确认你
理解 [数据、隐私与恢复](./docs/engineering/data-privacy-recovery.md) 中的配置所有权边界。

自行构建的 `.app` 没有 Developer ID 签名，也没有经过 Apple 公证，macOS 可能显示警告或阻止
启动。本项目目前不提供绕过 Gatekeeper 的步骤，也不暗示该产物经过 Apple 验证。

## 开发运行

普通界面开发使用隔离的 QA 标识和数据目录：

```sh
pnpm tauri:qa:dev
```

QA 模式不会接管生产 Codex 配置，并使用系统分配的临时代理端口。`pnpm tauri dev` 使用生产标识和
生产数据边界，只有确实需要验证真实 Codex 集成且已备份配置时才应手动运行。

## 数据与隐私摘要

- 路由、API Key、设置、请求元数据和恢复状态保存在应用数据目录的 `router.sqlite3`。
- API Key 不写入普通运行日志，但会以原始字节保存在受私有权限保护的 SQLite 数据库中；当前不使用
  macOS Keychain，也不做应用层加密。数据库和恢复点都应按敏感文件处理。
- 请求记录只保留路由、状态、token、费用估算等有界元数据，不保存原始提示词或完整响应正文。
- 运行日志使用固定错误码和有界计数，不应包含 API Key、Authorization header、请求正文、完整
  配置、上游 URL 或 provider 原始消息。
- 恢复点保存关键配置，包括路由密钥和 Codex 配置恢复信息；不包含请求历史、用量行或日志。
- AI Router 不提供云同步。所有外发网络流量来自用户配置的上游请求或 Codex 自身行为。

具体路径、备份范围和恢复限制见
[数据、隐私与恢复](./docs/engineering/data-privacy-recovery.md)。安全问题不要在公开 Issue 中提交，
请按 [安全政策](./SECURITY.md) 使用私下报告入口。

## 常用命令

| 目的          | 命令                                                    |
| ------------- | ------------------------------------------------------- |
| 文档契约      | `pnpm docs:check`                                       |
| ESLint        | `pnpm lint`                                             |
| TypeScript    | `pnpm typecheck`                                        |
| 前端测试      | `pnpm test`                                             |
| Web 构建      | `pnpm build`                                            |
| Rust 格式     | `cargo fmt --check`                                     |
| Rust 静态检查 | `cargo clippy --workspace --all-targets -- -D warnings` |
| Rust 测试     | `cargo test --workspace`                                |
| 生成 IPC 类型 | `pnpm generate:types`                                   |
| 版本一致性    | `pnpm version:check`                                    |
| 许可证与来源  | `pnpm license:check`                                    |
| 生产 `.app`   | `pnpm tauri:prod:build`                                 |
| QA 开发       | `pnpm tauri:qa:dev`                                     |

## 故障排查

- `pnpm install` 报版本错误：确认 `node --version` 为 `v22.22.3`，`pnpm --version` 为
  `10.33.2`，并重新执行 `nvm use` 与 Corepack 激活命令。
- Rust target 缺失：执行 `rustup show`，确认活动工具链为 `1.97.1` 且包含
  `aarch64-apple-darwin`。不要通过降低仓库工具链版本规避错误。
- 端口占用：在系统设置中选择未占用的 loopback 端口；不要把代理改为对外监听。
- Codex 状态为“已更改”或“冲突”：不要直接覆盖 `config.toml`。先断开 Codex，检查当前 provider 与
  保留字段，再使用界面提供的预览/修复操作。
- 数据库启动失败：保留应用数据目录，不要手工编辑或删除 SQLite/WAL/恢复点。按启动恢复界面选择
  已验证恢复点；没有可用恢复点时再决定重新开始。
- 需要诊断：优先记录版本、macOS、复现步骤和界面显示的固定错误码。不要上传完整日志、数据库、
  Codex 配置或真实请求内容。

更多说明见 [支持说明](./SUPPORT.md)。

## 工程文档

- [工程文档索引](./docs/engineering/README.md)
- [架构与代码边界](./docs/engineering/architecture.md)
- [路由与韧性](./docs/engineering/routing-resilience.md)
- [数据、隐私与恢复](./docs/engineering/data-privacy-recovery.md)
- [macOS 原生生命周期](./docs/engineering/native-lifecycle.md)
- [风险对应的验证策略](./docs/engineering/verification.md)
- [贡献指南](./CONTRIBUTING.md)
- [行为准则](./CODE_OF_CONDUCT.md)
- [安全政策](./SECURITY.md)

## 截图策略

首发 README 不包含产品截图。以后新增截图时，只接受由第一方 `AI Router QA.app` 配合合成路由、
虚构密钥和虚构用量生成的专用素材；提交前必须检查画面、文件元数据和 OCR 结果，不得使用生产
配置、真实用户数据或第三方产品界面。

## 许可证

AI Router 自有代码使用 [MIT License](./LICENSE)。第三方组件、派生图标、嵌入 fixture 和定价数据
来源见 [第三方声明](./THIRD_PARTY_NOTICES.md)。
