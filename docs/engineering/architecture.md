# 架构与代码边界

AI Router 是一个单进程 Tauri 菜单栏应用。Rust 拥有持久化和运行时状态，React WebView 通过类型化
IPC 展示快照并发起命令。代理和界面共享同一 Rust 运行时，不存在独立云服务。

## 代码层次

```text
Codex CLI / Codex App
        |
        | loopback Responses API / MCP
        v
router-core  <->  SQLite / recovery / bounded logs
        |
        | typed Tauri IPC + state-area events
        v
React menu + Settings WebViews
        |
        v
user-configured upstream Responses APIs
```

`crates/router-core/` 是领域权威：

- `domain.rs` 校验 route ID、Base URL、API Key、设置值和共享枚举；
- `storage.rs` 在专用 SQLite executor 后维护 schema、事务、密钥和查询；
- `proxy.rs` 与 `proxy/` 负责 loopback ingress、Responses 流式转发、历史和回退；
- `codex_config.rs` 与 `codex_catalog.rs` 管理受保护的 Codex 投影和派生模型目录；
- `recovery.rs` 负责关键配置恢复点；`runtime_log.rs` 负责有界日志维护；
- `app_api.rs`、`state.rs` 和其他带 `TS` 导出的 Rust 类型构成 IPC 契约。

`src-tauri/` 是桌面组合边界。它可以创建服务、映射安全错误、发布状态事件、管理托盘/窗口并调用
macOS API，但不应拥有 SQL 或复制核心路由规则。`application_update.rs` 是 Tauri updater 的唯一
应用边界：它拥有远端元数据校验、pending update、单操作 gate、进度、安装与 graceful restart 意图。

`src/` 按功能组织菜单、设置和共享展示组件。`src/api/ipc.ts` 集中命令调用，`src/api/query.ts`
管理 React Query 快照和状态失效。`src/generated/` 来自 Rust，不能手工修改。

## 状态流

写操作先通过 Rust 校验并在需要时原子持久化，再更新内存路由快照，最后发布带 revision 的状态区域
事件。React 收到事件后只失效相关查询。界面本地状态可以保存输入草稿和交互状态，但不能成为路由、
恢复、连接或应用更新状态的持久权威。高频更新下载进度只走 typed channel；稳定状态边界才发布
`StateArea::ApplicationUpdate`，并只失效更新 snapshot。

错误在 core 中保持具体分类，到 `src-tauri` 才映射成稳定 `code`、安全中文消息、可重试标记和可选
字段名。IPC 不返回 SQL、文件绝对路径、TOML、原始上游 body 或凭据。

## 生成链

修改导出的 Rust DTO 后运行：

```sh
pnpm generate:types
pnpm typecheck
pnpm lint
```

提交应同时包含 Rust 来源和经过审查的 `src/generated/` 结果。若生成 diff 出现无关文件，先定位
注册或序列化漂移，不要手工修补输出。
