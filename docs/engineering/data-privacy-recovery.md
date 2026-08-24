# 数据、隐私与恢复

AI Router 不提供账户或云同步。数据主要位于当前 macOS 用户目录，但“本地”不等于“已加密”或“可
公开分享”。

## 数据位置

生产 bundle identifier 是 `com.relax.airouter`。在标准 macOS 环境中，Tauri 将应用数据和日志解析
到以下位置：

| 内容         | 位置                                                              |
| ------------ | ----------------------------------------------------------------- |
| 应用数据     | `~/Library/Application Support/com.relax.airouter/`               |
| 主数据库     | 应用数据目录下的 `router.sqlite3`                                 |
| 恢复点       | 应用数据目录下的 `recovery/`                                      |
| MCP 图片资产 | 应用数据目录下的 `mcp-images/<assetId>.png`                       |
| 派生模型目录 | 应用数据目录下的 `codex-model-catalog.json`（存在自定义模型时）   |
| 运行日志     | `~/Library/Logs/com.relax.airouter/`                              |
| Codex 配置   | `~/.codex/config.toml`（由 Codex 所有，AI Router 只做受保护投影） |

系统 API 最终解析的目录可能受沙箱或系统策略影响。设置中的“打开日志目录”是查看运行日志的权威入口，
不要在代码或 Issue 中硬编码维护者机器的绝对路径。

QA bundle 使用 `com.relax.airouter.qa` 的独立系统数据目录；自动化验收还可以使用带安全标记的临时
root。QA 数据不能从生产数据库复制。

## 数据分类

`router.sqlite3` 保存路由、Base URL、API Key、余额脚本、active route、回退设置、gateway token、
Codex 原始/恢复配置快照、自定义模型、请求/尝试元数据和用量统计。

API Key 和 gateway token 以原始字节保存在 SQLite 中。数据库、WAL、恢复点和备份依赖私有目录/
文件权限（目录 `0700`、关键文件 `0600`），但当前不使用 macOS Keychain，也不做应用层加密。拥有
用户文件读取权限的人仍可能读取这些密钥。

请求历史不保存原始请求 body、提示词、完整响应、Authorization header 或任意上游 header。费用是
根据带日期的本地价格快照计算的估算，不是账单或当前价格承诺。

图片 MCP 会将成功生成并验证的 PNG 作为本地资产写入 `mcp-images/`，再只返回文件路径、实际尺寸、
字节数和 SHA-256。PNG、上游 Base64 和原始响应不进入 SQLite、恢复点或运行日志。该目录保存的是
用户本机上的生成结果，不是数据库状态，也不等同于备份。

图片 MCP 失败只返回稳定本地 code、本地 requestId、封闭 stage/category、数值或 null 的上游状态和
retryable。已知上游类别使用固定安全描述；只有未知类别可以在当次 MCP message 中追加一条经过控制
字符清理、空白折叠和长度限制的 provider message。这个例外只存在于当前响应：provider code/message、
provider request ID、任意上游 header、原始请求/响应和底层网络、解码、IO 错误都不会进入 error.data、
运行日志、诊断、请求/尝试历史、SQLite、恢复点或普通 IPC。

运行日志每条最多 8 KiB，单文件最多 2 MiB，最多十个文件/20 MiB，并保留最多七天。日志只应包含
固定 code、安全组件名和有界计数；即使如此，分享前仍要人工检查，并且不能直接上传完整日志。

## Codex 配置所有权

生产模式首次连接前读取并保存 `~/.codex/config.toml` 的存在状态、原始字节和 Unix mode，形成不可变
基线。连接和重连保留当前 `model_provider` 标识及无关 provider/MCP/权限，只管理选中 provider 的
传输字段、AI Router 自有模型目录指针和可选图片 MCP 条目。写入使用 fingerprint、同目录临时文件、
fsync 和替换前竞态检查。

普通断开恢复可更新的“断开目标”；完整基线始终保留为首次接管前的所有权证据。外部编辑、symlink、
无效 TOML、同名 MCP 冲突或 fingerprint 变化都会失败关闭，不应通过直接覆盖文件解决。

## 恢复点

关键配置提交后，恢复 worker 在短暂安静期后生成一个 SQLite Backup API 副本，删除历史/用量/日志等
非关键表内容，执行完整性、外键、schema、领域引用和权限校验，再原子发布。最多保留五个有效恢复点
和一个隔离的损坏主库。

恢复点包含路由密钥、gateway token、Codex 基线/断开目标和自定义模型，因此同样是敏感文件。它们
不包含请求/尝试历史、Usage 行、日志、余额缓存、外部 `config.toml` 文件、派生模型 JSON 或
`mcp-images/` 图片资产。

正常设置只允许查看保护状态和手动创建恢复点。主数据库缺失、损坏或领域无效时，启动进入独立恢复
流程，只有通过当前 schema 和完整校验的候选才能恢复。恢复和“重新开始”都不会读取或写入
`~/.codex/config.toml`。

## 备份与问题报告

备份整个应用数据目录会同时备份凭据和 Codex 配置快照，应使用加密存储并限制访问。不要在应用运行时
只复制 `router.sqlite3` 而忽略 WAL；优先完全退出应用后备份整个目录。手工编辑数据库、恢复点或
`config.toml` 可能破坏 fingerprint 和恢复所有权，请先保留原件。

`mcp-images/` 中的 PNG 只是本地生成资产。除非用户另行将这些文件纳入自己的备份方案，否则文件损坏、
删除或应用数据目录丢失后无法依赖 SQLite 恢复点找回。

公开问题报告只提供合成复现、版本、稳定错误码和必要的有界字段。数据库、恢复点、完整配置、完整日志
和真实请求内容只能通过 [安全政策](../../SECURITY.md) 中约定的私下渠道讨论，而且通常不需要上传。
