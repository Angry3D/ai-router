# 发布与应用更新

AI Router 只为 macOS 13+ Apple Silicon 发布稳定版本。官方分发不使用 Apple Developer ID、Apple
公证、stapling 或 App Store；DMG 内的应用使用显式 ad-hoc 签名。ad-hoc 签名能保持 bundle 内代码
结构一致，但不能证明发布者身份，也不代表 Apple 已验证该应用。

## Release 资产

每个稳定 Release 必须同时包含以下五个资产：

| 文件                              | 用途                                    | 普通用户是否需要     |
| --------------------------------- | --------------------------------------- | -------------------- |
| `AI.Router_<version>_aarch64.dmg` | Finder 首次安装和手动恢复更新           | 是，首次安装只下载它 |
| `AI.Router.app.tar.gz`            | Tauri 应用内更新下载的应用归档          | 否，由应用自动获取   |
| `AI.Router.app.tar.gz.sig`        | 归档的项目 updater 签名                 | 否，由 updater 校验  |
| `latest.json`                     | 稳定版本、说明、归档 URL 和签名元数据   | 否，由应用检查       |
| `SHA256SUMS`                      | 五个内容资产中前四个文件的 SHA-256 清单 | 可选，供人工核对     |

GitHub artifact provenance 不是第六个下载文件，而是 GitHub 对这些资产生成的可查询 attestation。
它把构建产物绑定到 workflow、repository 和 source revision。SHA-256 与 provenance 有助于人工审计，
但应用内安装授权仍以 updater 签名为准。

## 信任边界

`latest.json` 和下载内容都按不可信远端输入处理。Rust coordinator 只接受 canonical repository、
`darwin-aarch64`、严格稳定且高于运行版本的 SemVer、canonical 归档 URL、有界版本说明和有界签名。
这些检查决定是否展示更新，不授予安装权限。用户确认下载后，Tauri updater 必须使用 bundle 中的
项目公钥验证归档签名，验证成功才进入安装。签名还必须记录它对应的版本：应用启用
`requireSignedVersion`，会把 `latest.json` 声明的版本与签名 trusted comment 中的版本比对，
不一致或缺失即拒绝安装。这样"把更大的版本号与另一个合法签名包配对"的做法无法促成降级安装。
该校验从启用它的构建开始生效；更早的已安装版本不校验签名里的版本。

新版本说明由提交在 `release-notes/v<version>.md` 的审核内容生成。Rust 只解析受限的标题和平铺
项目符号，并向界面投影 `重点更新`、`问题修复`、`注意事项` 三组数据；React 不解析 Markdown 或
HTML。设置页默认展示最多三条重点更新，按需展开全部分类项目。旧 manifest 的普通文本仍以有界
兼容模式展示，并保留 canonical GitHub Release 入口。

自动检查在应用进入 `Running` 后立即执行，尝试时间先写入 SQLite，再执行网络请求。此后调度器在同一
进程内长期运行：成功后等待 24 小时，网络或元数据失败后等待 6 小时，另一项更新操作占用互斥门时以
有界短延迟重试且不记录尝试。等待按墙钟时间分段计算，机器休眠不会拉长实际周期；时钟回拨超过周期
按到期处理。每次重新启动都会重置本进程的等待周期并立即检查——持久化的尝试时间只是运行记录，不再
跨启动抑制检查，恢复点仍会清除该非关键时间戳。数据库恢复（还原恢复点、重新开始、重试启动）成功后
同样进入 `Running`，因此也会启动调度器；调度器在一个进程内只认领一次，重复调用是空操作。安装完成
进入 `restart_ready` 后本进程停止调度，由
新进程重新开始。后台失败保持安静。手动检查绕过 cadence，并显示有界、可重试错误和 canonical
Release 入口。

下载、安装和重启都不会自动发生。下载/安装与重启分别需要确认；重启通过既有 graceful shutdown
路径关闭代理、余额、数据库和恢复服务。一个时间点只允许一个更新操作，旧 generation 不能覆盖新
状态，进度 channel 不触发全局查询失效。

## 出站代理

应用内更新检查（`latest.json` 元数据）与更新包下载都使用设置页的“全局出站代理”，与推理、余额、
模型发现等请求共用同一份代理设置。

「跟随系统」使用 macOS 的手动 HTTP/HTTPS/SOCKS 代理和绕过规则；未配置适用代理且未启用自动代理时
直连。「自定义代理」覆盖系统配置。回环目标（`localhost`、`127.0.0.1`、`::1` 及映射形式）始终直连，
因此 QA 回环 updater 端点（`AI_ROUTER_QA_UPDATER_ENDPOINT`）语义不变。环境变量代理不会改变决策。

不执行 PAC/WPAD：混合配置使用适用的手动代理或绕过规则；仅有自动配置可用、系统读取失败或配置无效
时拒绝外部请求，并提示检查系统设置或改用自定义代理。选中代理不可达时也不会回退直连。手动检查
显示有界错误，自动检查保持安静并沿用既有失败节奏。设置页的「测试连接」只验证代理端点连通性，
不验证 GitHub 可达性。

更新检查和下载各自捕获不可变策略，并对初始目标及重定向重新判定路由。缓存的 `Update` 在之后下载时
采用当前策略，不重新检查或替换已选版本；进行中的操作继续使用旧快照。客户端仍经 updater 插件的
`configure_client` 注入，下载、签名验证和安装仍由插件负责。

该设置只作用于应用自身发出的 HTTP 请求（更新、推理转发、余额、模型发现等）。设置中的“同步官网”
在隐藏的只读 WebView 中加载官方定价页，属于平台 WebView 网络栈：它的出口由 macOS 系统网络配置
决定（系统 HTTP/HTTPS/SOCKS 代理、例外清单与 PAC），既不读取「自定义代理」，也不读取环境变量
代理。若系统未配置代理而目标网络又必须经由代理，同步会
失败关闭并保留上一次的本地价格表，不会静默回退或降级成功提示。

## 首次安装与 Gatekeeper

普通用户只下载 DMG。由于没有 Apple Developer ID 与公证，macOS 可能阻止首次打开。文档与支持只
推荐系统路径：“系统设置 -> 隐私与安全性”，在“安全性”中确认 `AI Router.app` 后选择“仍要打开”。
不要建议 `xattr`、`spctl --master-disable` 或其他终端绕过方式。

源码构建不嵌入官方 updater 公钥，也不是官方更新链的一部分。第一个 updater-capable 稳定版本必须
作为手动桥接版本通过 DMG 安装；只有已经嵌入受信公钥的官方版本才能接受后续应用内更新。

## 密钥备份与轮换

`TAURI_SIGNING_PRIVATE_KEY`、密码和发布用公钥值只配置在受保护的 GitHub `release` environment。
私钥不得进入 git、workflow 参数、构建输出、应用 bundle、日志或 PR job。至少保留一份加密、离线、
经过恢复演练的备份，并把备份访问与 GitHub environment 审批分离。

正常轮换采用桥接版本：用旧私钥签署一个仍受旧版信任、但内嵌新公钥的稳定版本；确认采用窗口后，
下一版本改用新私钥签署。旧私钥在桥接采用窗口结束前保持离线可恢复。若旧私钥已经泄露，不能再用它
签署桥接版本；立即停止应用内发布，撤销环境值，并要求用户通过明确说明 hashes 与 provenance 的新
DMG 手动升级。

## 失败恢复

workflow 首先创建或验证 unpublished draft。构建、bundle 检查、updater 签名验证、`latest.json`、
checksums、远端回读和 provenance 任一步失败时，Release 必须保持 draft。相同 tag 的重跑只可清理和
修复该 draft；一旦发布，tag 与资产视为不可变，任何修正都使用更高 patch 版本。

发布操作与 GitHub 保护设置见 [稳定版本发布操作](./releasing.md) 和
[GitHub CI 与安全设置](./github-security-settings.md)。
