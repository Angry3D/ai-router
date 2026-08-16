# 安全政策

## 支持范围

AI Router 处于 `0.1.x` 早期阶段。安全修复只面向最新的 `main` 和最新稳定官方版本，不承诺旧提交、
非官方 fork、自行修改的二进制或未支持平台的长期维护。官方 DMG 使用 ad-hoc 签名，不包含 Apple
Developer ID 身份，也没有经过 Apple 公证。

## 私下报告

请通过 GitHub 的
[Private vulnerability reporting](https://github.com/Angry3D/ai-router/security/advisories/new)
提交安全问题。报告至少包含：受影响版本或提交、macOS/Codex 版本、风险影响、使用合成数据的最小
复现步骤，以及界面显示的稳定错误码。

不要在公开 Issue、Discussion、PR、截图或附件中提交真实 API Key、Authorization header、完整配置
（包括 Codex 配置）、SQLite 数据库、原始请求/响应、原始日志、恢复点或用户绝对路径。不要为了证明
问题而访问不属于你的账户、上游或设备。

如果 GitHub 私下报告入口暂不可用，可以创建一个不含技术细节和敏感材料的公开 Issue，仅请求维护者
提供私下联系渠道。维护者确认渠道前不要补充漏洞内容。

## 处理方式

维护者会尽力确认收到、评估影响并协调修复，但个人维护项目不提供固定响应时限或漏洞奖励。公开披露
时间应在修复和受影响用户可采取行动之后共同确定。

以下通常属于安全范围：

- loopback 边界、网关 token 或上游凭据泄露；
- Codex 配置未经授权覆盖、恢复或所有权判断错误；
- SQLite、恢复点、日志或请求元数据越权暴露；
- 公共源码、构建或依赖链携带凭据和未声明第三方内容；
- updater 私钥泄露、`latest.json`/归档替换、签名绕过或发布资产与 tag/commit 不一致；
- 能跨越生产/QA 数据或 bundle 隔离的原生生命周期问题。

普通崩溃、界面问题、兼容性建议和不含安全影响的上游错误请按 [支持说明](./SUPPORT.md) 使用公开
Issue。

## 安全事实

本地代理只监听 loopback。API Key 以原始字节存入私有权限的 SQLite 文件，当前不使用 Keychain 或
应用层加密；恢复点也包含关键配置。运行日志被设计为只记录有界固定码，但完整文件仍按敏感材料处理。
更多边界见 [数据、隐私与恢复](./docs/engineering/data-privacy-recovery.md)。

官方应用内更新同时依赖 canonical GitHub HTTPS 边界和项目 updater 签名。ad-hoc macOS 签名、
SHA-256 或 provenance 都不能单独替代 updater 签名校验。签名私钥与密码只存在于需要人工批准的
GitHub `release` environment 和离线备份中，不进入仓库、应用 bundle、日志或 pull request job。
密钥轮换与泄露处理见 [发布与应用更新](./docs/engineering/application-updates.md)。
