# macOS 原生生命周期

AI Router 是 `Accessory` 激活策略的菜单栏应用。`menu` WebView 启动时隐藏，并在 macOS 上转换为非
激活的 `NSPanel`；`settings` 保持普通窗口。托盘点击、首次展示和 LaunchServices Reopen 都复用同一
展示路径。

## 菜单面板

隐藏 WebView 可能暂停 WebContent 或停止调度 `requestAnimationFrame`。原生层先显示并定位窗口，再
发送带单调 generation 的准备事件；前端完成布局后回传高度，并同时提供 timer fallback。原生主线程
在真正展示前再次检查 generation，旧请求不能重新唤醒已隐藏菜单或写入首次展示状态。

菜单使用 key-capable、non-main、non-activating panel，使用户可以操作菜单而不把整个应用带到前台。
显式打开 Settings 才调用普通窗口聚焦。Escape、托盘切换、close request 和失焦隐藏菜单，不退出
进程。

修改菜单几何或 route usage preview 时，尺寸、位置、固定 backing 和 controller 状态必须在同一主
线程转换中提交；失败要回滚为完整基础几何，不能让异步旧命令覆盖新 revision。

## 生产与 QA 身份

| 模式 | bundle             | identifier              | Codex/data 行为                                          |
| ---- | ------------------ | ----------------------- | -------------------------------------------------------- |
| 生产 | `AI Router.app`    | `com.relax.airouter`    | 使用生产 app data；显式连接时管理 `~/.codex/config.toml` |
| QA   | `AI Router QA.app` | `com.relax.airouter.qa` | 使用独立 app data、隔离 Codex home 和系统分配代理端口    |

生产应用可能正在为当前 Codex 会话提供代理。自动化和贡献者脚本不得退出、signal、重启、替换或重新
启动生产 bundle。原生生命周期和破坏性恢复验收只能针对已验证 identifier 的 QA bundle，并使用
合成数据。

人工 QA 直接使用持久 QA 系统数据目录；自动化生命周期/恢复测试可以使用临时 acceptance root。
临时 root 不是第二个 QA 产品，也不应用于普通人工验收。任何模式都不得复制生产路由、API Key、
历史、数据库、日志或 Codex 配置到 QA。

## 构建

```sh
pnpm tauri:qa:dev
pnpm tauri:qa:build
pnpm tauri:prod:build
```

构建脚本统一使用根 `target/`，只生成 `.app`。生产和 QA 构建分别检查名称、identifier、图标和最低
macOS 版本。当前没有 DMG、安装器、签名、公证、App Store 或自动更新。

仅做文档、React 单元或 Rust 单模块改动时不要为了仪式运行原生 bundle。只有改动 Tauri 配置、图标、
面板/托盘生命周期、打包脚本或发布边界时，才需要 `.app` 构建和 QA 原生验收。
