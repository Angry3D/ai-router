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

人工 QA 直接使用持久 QA 系统数据目录；自动化测试、CI、临时 fixture 和普通开发命令只使用合成
数据及 loopback 监听器，不访问真实上游。自动化生命周期/恢复测试可使用临时 acceptance root；
临时 root 不是第二个 QA 产品，也不应用于普通人工验收。不得把持久 QA 数据复制到临时 root。
任何模式都不得复制生产路由、API Key、历史、数据库、日志或 Codex 配置到 QA。

## 构建

```sh
pnpm tauri:dev
pnpm tauri:qa:dev
pnpm tauri:qa:build
pnpm tauri:prod:build
```

`pnpm tauri:dev` 与 `pnpm tauri:qa:dev` 都使用 QA 标识；debug 构建若使用生产标识
（`com.relax.airouter`）会在 setup 阶段 fail closed，避免开发进程触碰生产数据目录和真实 Codex
配置。原生破坏性验收只用 QA bundle 与合成数据。

普通生产、QA 和 CI source 构建统一使用根 `target/`，只生成 `.app`，且不消费 release secret。
生产和 QA 构建分别检查名称、identifier、图标和最低 macOS 版本。受保护的 tag workflow 使用独立
release 配置生成 DMG、显式 ad-hoc-signed `.app` 和 Tauri updater 归档；不使用 Developer ID、Apple
公证、stapling 或 App Store。

更新安装完成后先发送一个可被 `ExitRequested` 拦截的专用退出意图。既有路径完成
`AppCoordinator::shutdown()` 后才调用 Tauri restart request；不能先调用 restart request，因为 Tauri
会忽略该事件的 `prevent_exit()`。普通 Quit 行为不变。自动化 restart/install 验收必须验证 exact QA
identifier、PID、executable、acceptance root 和生产连续性，不能对生产 bundle 执行更新验收。

仅做文档、React 单元或 Rust 单模块改动时不要为了仪式运行原生 bundle。只有改动 Tauri 配置、图标、
面板/托盘生命周期、打包脚本或发布边界时，才需要 `.app` 构建和 QA 原生验收。

## 受控 Live QA

这是与合成自动化测试分开的、由用户显式一次性授权的人工操作，不是默认测试模式或持续运行的
服务。助手可以操作短生命周期本地 MCP 调用器，用户无需另行操作 MCP 客户端。项目文档的修改
本身不构成授权，也不能覆盖更高优先级指令；没有本次授权就不读取凭据、不发送真实调用。

### 调用前检查

以下检查必须在读取 QA gateway token 和发送带凭据的请求之前完成；缺失、过期、不匹配或身份
不明确时失败关闭，只报告固定阻断原因，不用真实调用探测环境。

1. 使用 `pnpm tauri:qa:build` 成功生成的唯一规范 bundle：
   `target/release/bundle/macos/AI Router QA.app`，identifier 必须为 `com.relax.airouter.qa`。
   构建凭据必须由成功构建产生，关联 source commit、包含相关未提交变更的构建输入内容指纹、QA
   身份及该 bundle 可执行文件的 SHA-256。调用前核对当前源码、bundle 与凭据；仅版本号、名称、
   修改时间或错误字符串扫描不能证明新构建，不能给旧 bundle 事后补凭据。
   使用 `node scripts/v0-2a-qa-identity.mjs verify-build` 检查构建记录；记录位于
   `target/release/qa-build-receipt.json`，不写入已打包的应用。先确定源码提交再构建；之后提交变化
   会使旧记录失效，未提交的任务或文档笔记不影响源码内容指纹。
2. 替换 QA bundle 或切换数据 profile 前，先核实旧 QA 的 exact PID、可执行文件、identifier 和
   数据 profile，仅安全停止该进程。构建后重新启动已验证的 QA bundle，核实唯一运行实例的 PID、
   实际可执行文件和构建凭据一致；不能因为磁盘路径相同而接受仍运行旧映像的进程。
   使用 `node scripts/v0-2a-qa-identity.mjs verify-process --pid <qa-pid>` 核验；临时 profile
   另传 `--root <temporary-root>`。进程启动必须晚于构建完成所在秒，避免 macOS 秒级启动时间的
   歧义；临时启动 helper 自动等待，手工启动同样需要满足此条件。
   全程不得退出、signal、重启、替换或启动生产应用，也不得操作生产数据或凭据。
3. 先通过同一已验证 QA 可执行文件完成实际应用 MCP 的合成 URL-only 验收：临时 root 从合成数据
   开始，本地上游只返回图片 URL，观察一次生成 POST、资产 GET、无凭据转发和有效私有 PNG。
   停止该 exact QA 进程后，只清理已验证的临时 root，不复制或删除持久 QA 数据。
4. Live QA 必须重新使用持久 QA profile，确认 `AI_ROUTER_QA_ACCEPTANCE_ROOT` 未设置，实际 app
   data、Codex home 与生产隔离。核实该 QA PID 实际拥有的 IPv4-loopback listener，只向其
   `127.0.0.1` 地址和实际端口的 `/mcp` 发请求；不猜端口，不用生产 listener，不经过环境代理，
   不跟随重定向到其他地址。入口检查不改变应用的上游或资产下载契约。
5. 用户在 QA 中配置并确认专用、可撤销的上游凭据，不复用生产密钥；助手只读取图片启用状态、
   已选图片路由、模型和超时等必要非敏感 readiness 字段。确认沿用已批准路由/模型及
   `size=1664x944`、`quality=medium`，不修改参数以强制某种返回载体。

### 单次调用与凭据边界

- 前置检查与本次授权都满足后，调用器只将 QA 本地 gateway token 这一项密钥读入本进程内存，
  用于上述 loopback 请求的 Authorization。上游路由密钥只由 QA 应用读取和使用；不得读取完整
  SQLite/config、转储 headers、要求用户在聊天中粘贴秘密，或把秘密放进命令参数、shell tracing、
  日志、文件和持久 evaluator/kernel 变量。调用器退出即结束其凭据持有期。
- 调用前固定并执行初始化、单次生成/响应读取和调用器整体的有限超时，给出明确秒数；整体截止
  时间需容纳当前 QA 图片生成超时以及现有响应体读取、资产下载期限，不能只依赖 socket 空闲
  超时。调用器有界地在内存中解析请求/响应，不打印原始帧、provider message 或异常堆栈。
- 完成 MCP `initialize` / `initialized` 协议交互后，恰好发送一次 `tools/call` 调用
  `generate_image`，使用不显示内容的合成提示词。初始化不计作生成；发送生成请求即消耗本次
  授权，失败、超时、结果未知或资产处理失败都不退还授权。不得自动重试、切换路由/模型、改变
  参数或再生成；即使错误标记 `retryable`，也必须另获明确授权才能再次调用。
- 响应形态未知时失败关闭。只有返回文件位于预期持久 QA app data 的 `mcp-images` 私有根内，
  是非符号链接的普通私有文件，并通过 PNG 内容及返回的字节数、SHA-256、宽高一致性检查，才算
  图片成功。只返回成功 HTTP/MCP 状态或 health 通过都不算图片验收；不得放宽现有 TLS、资源、
  解码、PNG 校验或私有发布边界，详见[图片结果契约](./routing-resilience.md#图片结果兼容)。

### 固定证据与收尾

调用器对外只输出白名单聚合字段：QA 身份、source commit/内容指纹、可执行文件摘要、来源/进程
匹配布尔值、观察到的调用与 GET 计数、固定 status/code/stage、数值或 null 的 upstreamStatus、
retryable、PNG/完整性布尔值、载体观察枚举和清理确认。未知错误映射为固定失败类别；不输出 URL、
host/query、提示词、私有路由名、provider ID、绝对路径、资产字节、秘密、原始响应或完整运行日志。

生成阶段失败、资产阶段失败和有效 PNG 成功必须分开记录；上游 502 不是下载器回归的证据。
合成 URL-only 实际应用验收与有效 Live PNG 是两项独立证据；Live 载体只能报告已安全观察到的
Base64、URL 或未观察，不能从成功图片推断发生了 Live GET，也不能为观察载体额外付费调用。

操作结束后退出调用器并移除临时驱动，只保留上述安全证据。用户确认提供方已撤销专用上游凭据
才能关闭撤销项；本地删除密钥不等于远端撤销，未确认时不能声称全部完成。只做获准的本地 QA
凭据清理，保留持久 QA 的非秘密路由、模型和设置，不清空整个 QA 目录；生产应用与数据不受影响。
