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

## 图片结果兼容

图片 MCP 优先选择 `data` 数组中首个字符串型 `b64_json`；只有不存在此类字段时，才选择首个字符串型
`url`。空或无效 Base64 仍按编码错误处理，不会改用 URL 掩盖失败。两条路径都必须通过完整 PNG 校验和
私有文件发布，成功仍只返回包含 `status / path / mimeType / width / height / bytes / sha256 / assetId`
的单个文本 JSON 块。HTTP 图片入口保持成功响应字节不变，不解析其中的 URL 或下载资产。

资产下载只接受公共 HTTPS 的 443 端口。每次连接都检查全部 DNS 结果，固定获准地址并保留原域名的
TLS 验证；本机、私网、链路本地和保守规则排除的特殊用途地址均拒绝。下载使用独立客户端，不继承
生成接口密钥、gateway token、Cookie、Referer 或环境代理。签名查询参数只用于当前下载。

最多跟随三次经过同样检查的重定向，即最多四次资产 GET；整个下载阶段共用 600 秒截止时间，单跳 DNS
最多 10 秒、连接最多 30 秒。传输及内容解码后的资产各不超过 48 MiB，最终仍以 PNG 字节校验为准。
需要额外认证的图片源、自定义端口、私网图片源和非 PNG 格式不在支持范围内。

## 图片错误

Images HTTP/MCP 始终只调用一次已选择的图片路由，不自动重试，也不进入文本 Fallback。MCP 错误会明确
区分请求构造、连接、发送、上游超时、响应体读取、上游 HTTP 状态、响应解码、结果校验、资产下载和资产存储；
同时返回稳定本地 code、本地 requestId、数值或 null 的 upstreamStatus、封闭 category 和 retryable。
retryable 只供调用方判断以后是否值得手动重试，不会让 Router 重放本次请求。

生图接口的 3xx 响应按上游非成功状态处理，生成客户端显式关闭自动重定向和重试，避免 307/308
导致第二次生成 POST。生成结果中的图片 URL 使用前述独立的受控 GET 重定向规则。

没有可用图片载体返回 `image_result_missing`，不允许的 URL 或跳转目标返回 `image_result_invalid_url`，
下载失败返回 `image_asset_download_failed`，其 stage 为 `asset_download`。这些本地错误使用固定描述，
category 为 `unknown_upstream`，retryable 为 false；下载失败不会重新生图或切换路由。

upstreamStatus 指向实际相关操作：来源校验和首次 GET 前的 URL 拒绝使用生成响应状态；下载失败使用
资产 GET 的状态，尚无响应时为 null；跳转目标拒绝使用对应重定向状态；PNG 校验使用提供图片字节的
响应状态。不能用生成成功的 200 代替下载失败的状态。

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
资产 URL、签名参数及 Location 同样不能进入错误描述或诊断；下载 HTTP 错误正文直接丢弃。

修改路由/回退时至少证明：

- 同一请求的快照保持一致；
- 可重试分类和响应提交边界准确；
- 最大尝试次数有限，旧 generation 不能持久化；
- API Key、URL、body 和 provider 消息不进入日志或持久诊断；
- 流式成功、终止前断流和终止后断流各自有确定结果。
