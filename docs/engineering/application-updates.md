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
`darwin-aarch64`、严格稳定且高于运行版本的 SemVer、canonical 归档 URL、有界纯文本说明和有界签名。
这些检查决定是否展示更新，不授予安装权限。用户确认下载后，Tauri updater 必须使用 bundle 中的
项目公钥验证归档签名，验证成功才进入安装。

自动检查在应用进入 `Running` 60 秒后开始，尝试时间先写入 SQLite，再执行网络请求。跨启动 24 小时
内不重复自动尝试；未来时间戳按到期处理，恢复点会清除该非关键时间戳。后台失败保持安静。手动检查
绕过 cadence，并显示有界、可重试错误和 canonical Release 入口。

下载、安装和重启都不会自动发生。下载/安装与重启分别需要确认；重启通过既有 graceful shutdown
路径关闭代理、余额、数据库和恢复服务。一个时间点只允许一个更新操作，旧 generation 不能覆盖新
状态，进度 channel 不触发全局查询失效。

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
