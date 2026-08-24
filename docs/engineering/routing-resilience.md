# 路由与韧性

## 网络边界

AI Router 的本地代理只监听 loopback，并使用本地 gateway token 保护 Codex 投影。文本推理入口是
Responses API；项目不宣称兼容完整 OpenAI API。用户保存的 Base URL 可以是 API 前缀或一个终止于
`/responses` 的完整地址，内部统一保存前缀并只派生一个最终 `/responses`，不会自动猜测 `/v1`。

请求在进入代理后读取一份不可变路由快照。一次尝试绑定 route、Base URL、API Key、余额脚本设置和
服务层级策略；配置更新只影响后续快照，不能在进行中的请求里混合新旧字段。

## 自动回退

自动回退使用有序的参与路由边界。只有被明确分类为可重试、且尚未向 Codex 提交响应的失败才能尝试
下一路由。下游 headers/body 一旦开始传递，系统不能隐藏部分响应并切换上游。图片生成走独立的
单次路由，不复用文本自动回退。

切换成功必须持久化新的 active route；如果持久化失败，当前请求返回失败，不能只在内存中声称已经
切换。并发请求通过选择 generation 和有界尝试次数避免旧请求覆盖用户的新选择或形成回退循环。

## 流式响应

Responses SSE 在转发前观察有界事件数据，用于确定首个有效输出、终止状态、token 用量和历史结果。
代理不缓存完整无限流，也不把 provider 文本变成回退依据。超时、连接失败、上游 HTTP 状态和协议
错误保持不同的稳定分类。

## 图片错误

Images HTTP/MCP 始终只调用一次已选择的图片路由，不自动重试，也不进入文本 Fallback。MCP 错误会明确
区分请求构造、连接、发送、上游超时、响应体读取、上游 HTTP 状态、响应解码、结果校验和资产存储；
同时返回稳定本地 code、本地 requestId、数值或 null 的 upstreamStatus、封闭 category 和 retryable。
retryable 只供调用方判断以后是否值得手动重试，不会让 Router 重放本次请求。

上游 HTTP 错误只解析 64 KiB 内的 lowercase `error.{code,message}` 或顶层 `{code,message}`。
category 只来自精确、区分大小写的 code 白名单；未知 code 始终是 `unknown_upstream`，不会根据自由文本或
HTTP 状态猜测。仅在没有有效 code 时，400/422、401、403、429 和 5xx 才按状态使用封闭兜底分类。
内容策略、参数、鉴权、权限和配额失败不可重试；429 可重试，500/502/503/504 可重试，其他情况遵循
固定矩阵，但所有分支仍保持单次上游调用。

## 诊断与历史

向 Codex 返回的错误是有界 Responses 风格 DTO。当前响应可以显示经过长度和控制字符处理的 provider
错误消息，但它不会进入运行日志、请求历史、恢复点或长期推理状态。历史保存请求 ID、路由、尝试、
状态、时间、token、费用估算和回退结果，不保存提示词或完整响应正文。

Images 的已知类别只使用固定安全 message。只有 `unknown_upstream` 可以在当前 MCP message 中追加一条
经过控制字符清理、空白折叠和 240 个 Unicode 字符限制的 provider message；provider code、request ID、
header、原始 body 和底层网络/IO 错误不会进入 MCP 安全字段、日志或持久化。

修改路由/回退时至少证明：

- 同一请求的快照保持一致；
- 可重试分类和响应提交边界准确；
- 最大尝试次数有限，旧 generation 不能持久化；
- API Key、URL、body 和 provider 消息不进入日志或持久诊断；
- 流式成功、终止前断流和终止后断流各自有确定结果。
