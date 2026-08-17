# 稳定版本发布操作

稳定发布由 `.github/workflows/release.yml` 在 canonical repository 的 `vMAJOR.MINOR.PATCH` tag 上触发。
不要从本地上传正式资产，不要给 pull request workflow 发布权限，也不要把 updater 私钥写入命令行、
文件、日志或 workflow summary。

## 首次配置

1. 在 GitHub 创建名为 `release` 的 environment，配置 required reviewers，并限制为受保护的 `v*` tag。
2. 创建 tag ruleset：阻止 `v*` tag 删除、移动与强制更新，只允许维护者在 `main` 已通过 required
   checks 的提交上创建稳定 tag。
3. 在 `release` environment 中配置且只配置以下 secret：
   `AI_ROUTER_UPDATER_PUBLIC_KEY`、`TAURI_SIGNING_PRIVATE_KEY`、
   `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`。
4. 将加密私钥与恢复说明保存到独立离线位置，做一次不接触生产 app 的签名恢复演练。
5. 保持 repository 默认 `GITHUB_TOKEN` 为只读；release job 只在 environment 审批后取得
   `contents: write`、`id-token: write` 和 `attestations: write`。

公钥值采用 Tauri 生成的 base64-encoded minisign public-key 文件内容。私钥必须使用密码保护。不要把
真实值用于测试；本地测试使用一次性合成密钥与 `AI Router QA.app`。

## 版本与 tag

`src-tauri/tauri.conf.json` 是手工版本源。先修改稳定 SemVer，再运行：

```sh
pnpm version:sync
pnpm version:check
```

`version:sync` 只把该值投影到 `package.json`、根 `Cargo.toml`，以及 `Cargo.lock` 中
`ai-router-app`、`router-core` 两个本地 workspace package；不要再手工修改这些投影。
同步会保留其他字节和依赖版本。审查完整 diff，确认只有上述版本字段变化，再由
`version:check` 对所有投影做关系校验。

合并并确认 required checks 后，在该不可变提交创建完全相同的 `v<version>` tag。workflow 会再次比较
repository、`GITHUB_REF`、tag、应用版本、checkout `HEAD` 和 tag commit；prerelease、build metadata、
移动 tag 或版本漂移全部失败关闭。

## Workflow 顺序

1. `release:validate` 验证版本、tag、commit 与 updater 公钥形状。
2. `release:draft` 创建 draft；重跑只清理同 commit 的 unpublished draft，已发布 Release 会硬失败。
3. `release:build` 用临时 `0600` 配置注入公钥，生成 ad-hoc-signed DMG 和 Tauri updater 归档/签名。
4. `release:prepare` 检查 app 与 DMG 中的 identifier、版本、macOS 13、arm64、`Signature=adhoc`、无
   Developer ID authority，验证 updater 签名，生成 `latest.json` 与 `SHA256SUMS`，上传后再下载逐字节
   比较 draft 资产。
5. 固定 SHA 的 GitHub attestation action 为完整资产目录生成 provenance。
6. `release:publish` 再次回读并验证 draft，最后一次性切换为 published。

基础 `tauri.conf.json`、QA 配置和 `pnpm tauri:source:build` 都不创建 updater 资产，也不消费 release
secret。只有临时合并的 `tauri.release.conf.json` 启用 DMG、updater artifact 和 `signingIdentity: "-"`。

## 发布后检查

- Release 不再是 draft，tag 仍指向 workflow 的 source SHA；
- 五个资产名称、大小与 `SHA256SUMS` 一致；
- GitHub 显示每个发布资产的 provenance；
- Release 说明把 DMG 标为首次安装入口，并明确没有 Apple 验证或公证；
- 用隔离的一次性 updater QA 根验证 current/newer/malformed/offline/signature-failure/install/restart；
- QA 前后读取生产 PID 与 bundle 路径，确认生产 `AI Router.app` 未被退出、替换、启动或重启。

首个 updater-capable 版本只验证手动 DMG 桥接。第一次真实应用内升级必须从该桥接版本升级到更高的
稳定 QA 版本，并继续使用 `com.relax.airouter.qa` 做破坏性生命周期验收。

## 失败处理

失败运行留下 draft 是预期安全状态。修复代码或环境后，在 tag 未移动且 source commit 未变化时重跑；
脚本会拒绝 published Release、不同 target commit 或不完整远端状态。不要删除/覆盖已发布资产，也不要
移动旧 tag。已公开版本有任何错误时，修复后提升 patch 版本并发布新 tag。

updater 私钥疑似泄露时停止 workflow，移除 environment secret 并按
[发布与应用更新](./application-updates.md) 的泄露路径要求手动 DMG 升级。
