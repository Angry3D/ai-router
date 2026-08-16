# 支持说明

AI Router 当前是 `0.1.2` 早期个人维护项目，提供简体中文官方 DMG 与源码。维护者会尽力处理可复现
问题，但不提供服务等级、响应时限、付费支持或旧版本长期维护。

## 支持的使用方式

- macOS 13 或更高版本、Apple Silicon；
- 按 [README](./README.md) 使用 Node `22.22.3`、pnpm `10.33.2`、Rust `1.97.1` 和锁文件构建；
- Codex CLI/App 与 OpenAI Responses API 兼容上游；自动兼容性证据固定为
  `codex-cli 0.147.0`；
- 使用 canonical GitHub Release 的官方 DMG，或项目原始源码和未修改的生产/QA 配置。

Windows、Linux、Intel Mac、其他语言、广域网代理、多人共享网关、Developer ID 签名、Apple 公证、
App Store、静默更新和 Gatekeeper 绕过说明均不在当前支持范围。官方 DMG 只有 ad-hoc 签名，未经过
Apple 验证或公证；源码构建也不嵌入官方 updater 公钥。

## 获取帮助

提交 Bug 前请先运行 `pnpm docs:check`，检查 [README 故障排查](./README.md#故障排查)，并搜索已有
Issue。公开报告应包含版本/提交、macOS、硬件、Codex 版本、构建方式、最小复现步骤、期望结果、
实际结果和稳定错误码。

不要在公开 Issue、PR 或附件中提交真实 API Key、Authorization header、完整配置（包括 Codex
配置）、SQLite 数据库、原始请求/响应、原始日志、恢复点或用户绝对路径。必须分享诊断时，只摘录与复现直接
相关的固定错误码，并再次检查内容。

安全问题使用 [安全政策](./SECURITY.md) 的私下渠道，不要创建公开漏洞报告。功能建议应说明当前
痛点、目标用户、可验证结果和明确非目标，避免把某个外部产品的完整功能集当作默认范围。

## 源码构建边界

维护者可以协助判断仓库脚本或受支持工具链中的可复现问题，但无法排查未知 shell 配置、第三方包管理
器、企业设备策略、非官方签名工具或修改后的 fork。源码构建产生的本地应用由构建者自行保管和评估。

## 安装与更新支持

首次安装下载 DMG 即可；Release 中的 `.app.tar.gz`、`.sig` 和 `latest.json` 是应用内更新所需文件，
`SHA256SUMS` 与 GitHub provenance 用于独立核对内容和来源，不是需要手动安装的替代包。macOS 阻止
首次启动时，只支持“系统设置 -> 隐私与安全性 -> 仍要打开”的系统授权路径。

应用内更新只支持 canonical repository 发布、签名正确且严格更新的稳定版本。失败会保留当前安装；
可从界面打开 canonical Release 后手动安装 DMG。第一个 updater-capable 版本必须手动通过 DMG 安装。
