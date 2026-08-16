# 风险对应的验证策略

验证目标是证明本次改动改变的行为，而不是无条件运行所有命令。先列出变更文件、可观察结果、跨层
契约和还缺少的证据，再选择能关闭证据缺口的最小命令。

| 改动范围                                            | 最小充分证据                                                          |
| --------------------------------------------------- | --------------------------------------------------------------------- |
| 文档、Issue/PR 模板                                 | `pnpm docs:check`、`git diff --check`                                 |
| 单个 React 组件/工具                                | 对应 Vitest、`pnpm typecheck`、`pnpm lint`                            |
| 单个 Rust 模块                                      | 对应 `cargo test` filter、`cargo fmt --check`、受影响 crate 的 Clippy |
| Rust/TypeScript DTO                                 | producer/consumer 测试、`pnpm generate:types`、前后端静态检查         |
| Web 入口或 bundler                                  | 前端全测、`pnpm build`                                                |
| schema、恢复、路由并发、SSE、native lifecycle、发布 | 对应领域完整测试和 workspace 检查；必要时 QA `.app`                   |

聚焦前端测试直接运行：

```sh
pnpm exec vitest run <test-file>
```

Rust 聚焦示例：

```sh
cargo test -p router-core <test-filter>
cargo clippy -p router-core --all-targets -- -D warnings
cargo fmt --check
```

跨层或高风险基线：

```sh
pnpm lint
pnpm typecheck
pnpm test
pnpm build
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## 领域证据

- 数据库/schema：迁移、完整性、外键、权限、symlink/unsafe path、事务回滚和旧数据兼容。
- 恢复：sanitized round trip、敏感/非关键表边界、保留策略、损坏主库、发布/回滚失败。
- 路由/回退：不可变快照、响应提交边界、重试分类、尝试上限、并发 generation、持久激活。
- SSE：首输出、token、终止状态、终止前/后断流和有界错误。
- Codex 配置：基线不可变、provider identity、无关字段保留、symlink、fingerprint 竞态和精确恢复。
- 原生生命周期：只使用 QA identity，验证隐藏/重开、主线程 generation、菜单聚焦和生产连续性。
- 发布：锁定工具链、license/provenance、公共树分类、secret/path scan 和干净根历史。

GitHub required checks、native source build 和仓库安全功能的稳定名称与配置见
[GitHub CI 与安全设置](./github-security-settings.md)。修改 workflow、Dependabot 或供应链策略时运行：

```sh
pnpm ci:policy
pnpm security:public:check
pnpm license:public:check
pnpm contracts:check
pnpm check:codex-retries
```

公开 GitHub checkout 使用 `security:public:check` 和 `license:public:check`，因此任何私有 workflow 路径、
禁止的模板标记、未声明视觉资产或敏感文本都会令 required check 失败。私有维护仓库使用
`security:check` 扫描其公开投影，并使用 `license:check` 审计元数据、来源和依赖；两者都不会把刻意保留的
Trellis 根误报为待发布文件。提交后的公开 export 仍必须运行两个 `public` 命令。

CI 原生源码验证使用 `pnpm tauri:source:build` 显式关闭签名；本地生产构建仍使用
`pnpm tauri:prod:build`。两个命令都只生成工作区内的 bundle，不安装或启动应用。

最后一个命令只启动临时 loopback fixture，使用 lockfile 固定的 `codex-cli 0.147.0` 和临时
`CODEX_HOME`，不读取用户 Codex 配置或真实上游。

## 避免重复

相关文件没有再变化时，可以复用已经通过的检查。一次局部测试通过后，只有完整 suite 能证明另一个
明确风险时才继续扩大。文档变更不触发 Rust workspace、前端 build 或原生 bundle；任务状态更新也
不要求重跑产品测试。

人工 QA 只覆盖自动化无法可靠判断的视觉层级、文字、焦点、真实 OS 集成和外部环境。不要让人工重复
数据库迁移、校验、竞态或恢复状态转换等已经自动证明的行为。
