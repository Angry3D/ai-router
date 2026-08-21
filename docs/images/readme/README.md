# README 截图来源

<!-- provenance:readme-product-screenshots -->

本目录保存根 README 使用的 AI Router 产品截图。截图中的 AI Router 界面构图属于项目自有
内容，按仓库根目录的 MIT License 发布；界面中渲染的 Lucide 图标仍受 Lucide ISC 许可证
约束。

## 捕获记录

- 捕获日期：2026-08-20 至 2026-08-21
- 源码版本：`9dd77e99df4a5a5845a525a406a21e3b706b7f3c`
- 应用身份：`AI Router QA.app` / `com.relax.airouter.qa`
- 菜单视图：`384 x 364` 像素
- 路由视图：`920 x 640` 像素

菜单图片来自 2026-08-20 的隔离 QA 运行；路由图片来自 2026-08-21 的两次隔离 QA 运行。
每次运行都使用全新的临时 QA 数据目录、仅监听 IPv4 loopback 的仓库 fixture、四条名为
`Synthetic A` 至 `Synthetic D` 的合成路由，以及 320 条合成请求记录。路由截图的主题、
窗口几何、路由名称和排序一致；因为使用独立运行，画面中的临时 loopback 端口不同。
深色菜单截图中的设置按钮处于 hover 状态，浅色菜单没有；该差异只影响按钮背景，不改变
应用状态或数据。

捕获过程只启动 QA bundle，没有操作、退出、重启或读取生产 `AI Router.app`。捕获结束后，
QA 进程和 fixture 已停止，临时数据目录通过仓库的 QA 身份校验器删除。

## 发布前检查

每张图片均完成以下检查：

- 查看完整画面，确认只包含 AI Router QA 窗口和合成数据；
- 检查 API Key、Authorization header、用户路径、生产路由名和真实请求内容等敏感信息；
- 重新编码为 PNG，检查文件类型、尺寸、alpha 行为、Display P3 色彩配置和 PNG 元数据；
- 在最终字节上计算 SHA-256，并由 `scripts/license-policy.json` 绑定。

最终文件：

| 文件                       | SHA-256                                                            |
| -------------------------- | ------------------------------------------------------------------ |
| `menu-overview-light.png`  | `13fd3aed0e73babaf668c1df8615398a62f0b9853ce0817f57b1b9b7325806a9` |
| `menu-overview-dark.png`   | `6d742befee5a4f5cbae058006a2eb621d89f7bd22c5ed76359f2e45aa8c62a6b` |
| `route-overview-light.png` | `4671029bbad6291584a8792ad86d2802833ccc05b34930d83aa051f52d83d9aa` |
| `route-overview-dark.png`  | `65fca16ffdeb0c10c9bcc5fdf50f3695cac9e9b3eff9769e1252125e4819209b` |

Lucide 的上游信息和 ISC 许可证全文见
[第三方声明](../../../THIRD_PARTY_NOTICES.md)与
[`third-party/licenses/ISC-Lucide.txt`](../../../third-party/licenses/ISC-Lucide.txt)。
