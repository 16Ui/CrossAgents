# 模型加载、嵌套滚动与 Antigravity 资格说明验证

日期：2026-11-16。基线：f69d86e。任务：01a1e453-0b56-7432-8be0-cfdc307cb70f。

## 证据边界

尝试了 browser-use 的 IAB 初始化，返回 `No Codex IAB backends were discovered`；随后检查桌面入口，原生管道与浏览器 access host 均不可用。因此 **未能实际操作 Work→Chat，也未取得滚轮前后实测数据**。下面的 CLI/缓存测量是真实执行的受控测试，不能当作用户账号或真实 GUI 的端到端测量。

未登录、修改或探测用户的 Google 账号地区，未更改出口或账号。新增测试仅使用本地 Node 假 CLI、内存数据库及受控供应商，不发送模型生成请求。既有测试使用临时 hub；`HubConfig::new` 默认关闭真实账号发现。

## 修复前后测量

`model_listing_does_not_run_quota_reports` 使用真实 Node 子进程：`models` 立即返回，其他命令等待 800 ms，所有调用写入临时文件。

| 测量 | 修复前 | 修复后 |
| --- | --- | --- |
| Antigravity 模型查询 | 约 1117 ms；测试失败 | 约 201 ms；测试通过 |
| 实际 CLI 调用 | `models`、`-p /usage --output-format json`、`-p /credits --output-format json` | 仅 `models` |
| 供应商被信号量挂起时连续 10 次 hub 模型读取 | 未采集基线数值；静态代码为逐供应商、逐账号等待 | 约 1.9 ms；只启动一个后台读取 |
| Work→Chat 首次显示时间、回到首页显示时间 | 浏览器不可用，未测量 | 浏览器不可用，未测量 |
| 嵌套滚轮与窗口回弹 | 浏览器不可用，未测量 | 浏览器不可用，未测量 |

这是单次开发机样本，包含 Node 启动开销，不是延迟 SLA。回归断言检查调用记录和请求合并，不依赖上述毫秒数。

静态结论：原来的 `models()` 先等待 `status()`，后者包含两个额度报告；`GET /plugins` 逐项等待这些读取。原来的全局 `* { overscroll-behavior: none }` 阻止所有嵌套区域的边界传递。代码中未发现需要替换的全局 wheel 拦截器。

## 实现与自动回归覆盖

- Antigravity 模型查询单独缓存并合并并发，不再调用额度报告。缓存包含完整账号身份与失效代数；登录/插件刷新使先前读取失效。资格错误追加解释，原始 `/usage`、`/credits` 错误完整保留；网络错误不追加资格判断。
- hub 每个实例独立保存账号模型读取，按完整账号配置隔离；冷读立即提供驱动现有列表，后台读取按账号并行。每账号一个在途读取，30 秒上限，60 秒有效期，后台模型变化沿现有 `account.quota_updated` 事件通知客户端。票号阻止账号/插件变更前的结果回填。
- 冷启动账号发现改为并行探测、按完成顺序发布 `account.created`；一个供应商的状态/额度查询挂起时，其他供应商的账号和模型仍可先更新。受控测试挂起字母序靠前的供应商，验证后面的快速供应商先被发现。
- 所有 GUI 模型列表调用统一为 `usePlugins`。同一连接共享一次读取，事件合并 250 ms；页面返回保留列表，30 秒后返回会后台重验。连接身份变更与重连清空旧数据并拒绝旧响应。普通资源读取也拒绝前一连接的晚响应。
- 可见性保存现在发送 `plugin.changed`，其他窗口和所有选择器可刷新。账号创建/移除/登录完成、插件变化、CLI 安装事件均进入共享刷新路径。
- 窗口重新聚焦或恢复可见时统一合并读取目录，Home 也可发现终端里安装/卸载的 CLI；移除设置页重复的 focus 刷新监听。这里只读列表，不自动执行完整版本探测命令。
- CSS 仅在 `html, body` 限制 overscroll，继续保留其 `overflow: hidden`；移除展开通知和通知列表内多余的 `contain`。代码块、输入框、面板恢复浏览器默认滚动链。未添加 wheel 监听或 `preventDefault`。

| 场景 | 自动验证范围 | 仍需端到端验证 |
| --- | --- | --- |
| 冷启动、返回页面 | 共享缓存首次读取、并发合并、返回保留、过期后台重验 | 真实 Work→Chat 首屏时间 |
| 慢供应商 | 挂起一个供应商，另一个仍能报告模型 | 真实供应商延迟分布 |
| 模型可见性 | 磁盘过滤规则、API 保存事件、GUI 事件路由 | 已打开菜单与多窗口同步 |
| 账号变更 | 完整账号配置失效、旧响应丢弃、登录完成、移除、代理配置缓存失效 | 真实登录后的菜单与账号标签更新 |
| 插件安装/卸载 | 插件事件清理模型缓存、安装事件刷新路径；既有安装 API 回归 | 实际安装/卸载后的菜单表现；未执行用户软件安装/卸载 |
| hub 重连/切换 | reset 丢弃旧列表和晚响应，独立 hub 实例不共用缓存；既有连接回归 | 真实隧道断连和两台 hub 的 UI 隔离 |

## 待补充的浏览器检查

可使用现有 `preview:mock` 场景（不连接真实模型）：`diagram`、`plan-long-search`、`model-search`、`relay-needyou-steps`，以及正常会话的输入框和左右栏。启动命令也须经过 `crossagents exec --resource build -- npm --prefix apps/gui run preview:mock`。如需人工前后比较，在另一份 f69d86e checkout 使用相同场景、窗口尺寸和系统滚动设置。

每项分别从中部、顶部、底部向两个方向滚动，并记录内外滚动位置或前后截图：

| 区域 | 要确认的行为 |
| --- | --- |
| 长代码块（横向与纵向） | 可滚方向内层优先；达到边界后传递到会话；不能卡住外层 |
| 展开面板/通知 | 面板中部内层优先；边界回到所属外层列表/会话 |
| 模型菜单与其他弹层 | 自身可滚，边界不移动无关栏；菜单定位不被裁切 |
| 多行输入框 | 文本内部先滚，边界保持浏览器原生行为，不跳到无关栏 |
| 左侧栏、主会话、右侧独立栏 | 指针所在栏滚动；到边界不驱动兄弟栏 |
| 所有区域的最外边界 | 整个窗口无橡皮筋回弹、无页面位移 |

浏览器入口恢复之前，上表均为待验证项目，不能宣称通过。

## 官方资料与资格错误解释

已读取 [Antigravity FAQ](https://antigravity.google/docs/faq) 和 [Google 服务条款](https://policies.google.com/terms)。FAQ 明确要求账号关联支持的国家，且提示在条款页查看关联国家并申请更正；年龄及组织账号权限也可能影响资格。美国代理出口不能据此证明账号地区资格。

[国家关联更正表单](https://policies.google.com/country-association-form) 重定向到 Google 登录页，未登录或提交。公开条款页显示的地区不是用户已登录账号的地区证据。应用提示明确说明未验证账号地区、不能保证解决拒绝。

## 检查记录

所有构建/测试经过 `crossagents exec --resource build --`；Windows 使用已安装的 crossagents.exe 和 npm.cmd 完整路径。初次基线用例按预期失败。最终检查如下：

- 第二次 `cargo test --workspace` 全部通过；需要真实供应商账号或外部网络的 smoke 按仓库默认忽略。
- 完成最后的冷启动改动后，`cargo test -p crossagents-hub -p crossagents-plugins --lib -- --nocapture` 通过：hub 236 项，plugins 73 项。含冷启动快速账号先发布、慢供应商独立、失效旧响应、Antigravity 查询合并及原文错误保留回归。
- 最终 GUI `npm run typecheck`、`npm test`、`npm run build` 均通过；包含新的缓存/事件回归及既有 Electron 101 项测试。
- 修改的五个 Rust 文件 `rustfmt --edition 2024 --check` 通过，`git diff --check` 通过。
- `cargo build -q -p crossagents-mock-agent` 及 `cargo run -q -p crossagents-cli --bin crossagents -- hub sandbox smoke` 通过：临时 hub 启动 2.56 s，模拟 agent 创建 0.02 s，消息回复 0.36 s，整体约 3.1 s。

`cargo fmt --all -- --check` 检出基线 `crates/api-types/src/lib.rs` 等处已有格式差异；未修改范围外文件，仅格式化本次修改的 Rust 文件。既有 ts-rs serde 属性解析警告和 Vite 大 chunk 警告不影响上述通过结果。

第一次 `cargo test --workspace` 在既有 `extensions_cancelling_and_stopping_the_hub_end_calls` 失败：`crates/hub/tests/extensions_runner.rs:588`，shutdown 后 `Running` 而非 `Cancelled`。此前驱动单测和 hub API 25 项集成测试（含新增可见性测试）均通过。该失败路径不经过模型目录；尚不能据此断定基线必现或本次引入。单独复跑及最终检查结果另记在任务报告，不将首次 workspace 检查记录为通过。

该关闭测试随后单独复跑通过，第二次 workspace 的整组扩展测试也通过；保留首次失败记录，未修改无关扩展实现。一次排队等待格式化的 executor 达到 600 秒等待上限，命令未执行；整套测试释放资源后重新执行成功。
