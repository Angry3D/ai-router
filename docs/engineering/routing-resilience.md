# 路由与韧性

## 网络边界

AI Router 的本地代理只监听 loopback，并使用本地 gateway token 保护 Codex 投影。文本推理入口是
Responses API；项目不宣称兼容完整 OpenAI API。用户保存的 Base URL 可以是 API 前缀或一个终止于
所选协议端点的完整地址，内部统一保存前缀并只派生一个最终端点（Responses 为 `/responses`，
Chat Completions 为 `/chat/completions`），不会自动猜测 `/v1`。

每条路由选择一个上游协议。Responses 路由把原始请求字节转发给 `/responses`；Chat Completions 路由
只在这一次尝试内把 Responses 请求翻译成 Chat 请求，并把上游响应翻译回 Responses 事件。翻译失败
属于本地客户端错误：不发送上游请求、不打击路由健康、也不静默跳到下一条路由。

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

## Chat Completions 上游兼容

Chat Completions 路由仍然向 Codex 提供 Responses 契约：请求在这一次尝试内被翻译，上游响应被翻译回
Responses 事件，回退、历史、用量和诊断继续沿用既有语义。翻译是有界且失败关闭的，不会静默丢弃
语义：

- 工具声明按 64 字节上限扁平化命名空间（超长时追加 SHA-256 后缀），重名或空名一律失败关闭，而不是
  静默丢弃工具。
- 托管 `tool_search` 使用固定的合成函数声明，并在返回时还原为 `tool_search_call`；默认形态的
  `web_search` 声明（仅 `external_web_access` 布尔值）会被省略并记录兼容标记，更丰富的形态失败关闭。
- 自定义工具把原始声明嵌入函数描述，调用输入包装为字符串字段，返回时还原为 `custom_tool_call`。
- 可读的 reasoning 文本会回放到对应的 assistant 轮次，`reasoning.effort` 的
  `low | medium | high | xhigh | max` 映射为顶层 `reasoning_effort`；只有不透明
  `encrypted_content`、provider 专有形态或不支持的值会失败关闭。
- Codex 本地压缩仍是受支持路径；带 `compaction_trigger` 的请求在发送上游前失败关闭。
- 上游若忽略 `stream: true` 而返回 JSON，同一套条目构建逻辑仍会生成 Responses SSE。
- 增量 SSE 组帧有 256 KiB 上限：单个完整帧和未完成的尾部都受同一上限约束，超限即输出有界失败并
  结束本轮，不会无限增长缓冲。
- 上游完全未提供用量时不会伪造计费数据；上游提供了用量但缺少必需字段时，会按 Codex 0.155.1 的
  契约把 `input_tokens_details.{cached_tokens,cache_write_tokens}` 等必需字段补零，而不是让整条
  响应解析失败。本版本对非流式客户端请求的 Chat 路由失败关闭。

这些兼容行为只影响 Chat Completions 路由；Responses 路由保持字节不变的转发。

## 图片结果兼容

图片 MCP 优先选择 `data` 数组中首个字符串型 `b64_json`；只有不存在此类字段时，才选择首个字符串型
`url`。空或无效 Base64 仍按编码错误处理，不会改用 URL 掩盖失败。两条路径都必须通过完整 PNG 校验和
私有文件发布，成功仍只返回包含 `status / path / mimeType / width / height / bytes / sha256 / assetId`
的单个文本 JSON 块。HTTP 图片入口保持成功响应字节不变，不解析其中的 URL 或下载资产。

资产下载使用独立客户端，保持 `no_proxy()`，不继承生成接口密钥、gateway token、Cookie、Referer 或环境代理。URL 只需通过客户端的解析和请求构造；不再执行 URL 长度、scheme、端口、userinfo、fragment、主机类别、DNS 结果、公网地址或远端 peer 准入。资源上限、截止时间、PNG 完整性和私有发布仍然生效。

最多跟随三次显式处理的重定向，即最多四次资产 GET；整个下载阶段共用 600 秒截止时间。支持的 HTTP(S) 目标由 reqwest 直接请求，无法构造或发送的目标按资产下载失败处理。传输及内容解码后的资产各不超过 48 MiB，最终仍以 PNG 字节校验为准。
额外认证的图片源不会继承生成请求凭据；图片源是否可访问由该直接 GET 的网络结果决定。

## 图片错误

Images HTTP/MCP 始终只调用一次已选择的图片路由，不自动重试，也不进入文本 Fallback。MCP 错误会明确
区分请求构造、连接、发送、上游超时、响应体读取、上游 HTTP 状态、响应解码、结果校验、资产下载和资产存储；
同时返回稳定本地 code、本地 requestId、数值或 null 的 upstreamStatus、封闭 category 和 retryable。
retryable 只供调用方判断以后是否值得手动重试，不会让 Router 重放本次请求。

生图接口的 3xx 响应按上游非成功状态处理，生成客户端显式关闭自动重定向和重试，避免 307/308
导致第二次生成 POST。生成结果中的图片 URL 使用前述独立的受控 GET 重定向规则。

没有可用图片载体返回 `image_result_missing`，URL 解析、请求构造、网络、状态或响应读取失败返回
`image_asset_download_failed`，其 stage 为 `asset_download`。这些本地错误使用固定描述，
category 为 `unknown_upstream`，retryable 为 false；下载失败不会重新生图或切换路由。

upstreamStatus 指向实际相关操作：来源选择/校验失败使用生成响应状态；资产 URL 解析或请求尚未收到响应时为 null；资产 GET 或重定向处理失败使用相关资产响应状态。PNG 校验使用提供图片字节的响应状态。不能用生成成功的 200 代替下载失败的状态。

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
