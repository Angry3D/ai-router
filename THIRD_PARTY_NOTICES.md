# 第三方声明

AI Router 的根 `LICENSE` 只授权项目自有内容。本文件列出的第三方内容不因进入本仓库而
重新授权为 MIT；使用和再分发时应同时遵守对应的上游许可证。

<!-- provenance:openai-codex-base-instructions -->

## OpenAI Codex base instructions

- 本地文件：`fixtures/codex-base-instructions-v0.147.0.md`
- 上游项目：`openai/codex`
- 上游版本：`rust-v0.147.0`
- 上游路径：`codex-rs/protocol/src/prompts/base_instructions/default.md`
- 上游地址：<https://github.com/openai/codex>
- 许可证：Apache License 2.0
- 本地处理：内容未修改；同目录 provenance JSON 绑定上游版本、路径和 SHA-256。

Apache License 2.0 全文见
[`third-party/licenses/Apache-2.0.txt`](./third-party/licenses/Apache-2.0.txt)。

<!-- provenance:lucide-icons -->

## Lucide icons

- 本地文件：`src-tauri/icons/app-icon*.{svg,png,icns}`、`icon.png` 以及
  `src-tauri/icons/tray-*.{svg,png}`
- 上游项目：`lucide-icons/lucide`
- 使用版本：`lucide-react 0.468.0`
- 上游地址：<https://github.com/lucide-icons/lucide>
- 许可证：ISC
- 本地处理：Route、TriangleAlert 和 CircleX 几何经过缩放、描边和组合；应用图标的背景、
  颜色、QA 标记及整体构图为 AI Router 项目自有设计。PNG 与 ICNS 由仓库脚本从 SVG 生成。

Lucide 的版权与 ISC 许可全文见
[`third-party/licenses/ISC-Lucide.txt`](./third-party/licenses/ISC-Lucide.txt)。
图标源文件、生成链路和输出边界见
[`src-tauri/icons/README.md`](./src-tauri/icons/README.md)。

<!-- provenance:openai-pricing-snapshots -->

## OpenAI pricing snapshots

- 本地文件：`crates/router-core/pricing/catalogs/openai-standard-2026-07-27.json`、
  `crates/router-core/pricing/catalogs/openai-priority-2026-07-28.json`
- 数据来源：OpenAI 官方定价、Prompt Caching 和 Fast mode 文档；每份 JSON 内保留具体 URL
  与捕获日期。
- 本地处理：只记录公开的事实费率并转换为整数
  `micro_usd_per_million_tokens`；未复制上游代码或说明文案。JSON 模式和实现代码属于
  AI Router 项目自有内容，采用根 MIT 许可证。

这些快照只用于本地估算，不是 OpenAI 的账单、报价或持续有效承诺。上游可能随时调整费率，
使用者应以实际账单和当前官方文档为准。详细单位与更新规则见
[`crates/router-core/pricing/catalogs/README.md`](./crates/router-core/pricing/catalogs/README.md)。

## 依赖项

npm 与 Cargo 传递依赖不会因出现在 lockfile 中而重新授权为 MIT。运行
`pnpm license:check` 可从当前 lockfile 和已安装依赖生成审计摘要；策略位于
`scripts/license-policy.json`。任何缺失、未知或未审核的许可证都会阻断检查。
