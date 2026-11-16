# 模型加载、嵌套滚动与 Antigravity 资格说明验证

日期：2026-11-16。基线：f69d86e。任务：01a1e453-0b56-7432-8be0-cfdc307cb70f。

## 证据边界

首轮的 IAB、桌面与浏览器 access host 不可用，未取得 UI 数据。返工时先查询 lesson_search（没有适用浏览器故障记录），完整读取 browser-use 与 computer-use skill，重新执行入口检查：IAB 仍返回 `Failed to connect to browser-use backend "iab". No Codex IAB backends were discovered.`；`@oai/sky` 导入成功，但 `sky.list_windows()` 返回 `Computer Use native pipe is unavailable: failed to connect native pipe: 系统找不到指定的文件。 (os error 2)`。

随后使用仓库已有 `scripts/screenshot.mjs` / Electron 隐藏窗口途径成功渲染 GUI，并建立本地 Vite + Electron 回归入口。**本轮取得了真实 Chromium 渲染的 mock UI 和原生滚轮实测**，详见下文；不是用户生产窗口、真实供应商时延或真实账号测试。首次截图命令打印了 `Downloading Electron binary...`；检查依赖入口确认 `require('electron')` 会调用其 `install.js` 自动补齐缺失二进制。这是未预判到的依赖环境变更，超出本轮“不安装软件”的约束；没有运行系统安装器或更改账号/出口。新增回归入口改为直接读取已存在的 `path.txt` 和二进制，缺失即失败，禁止自动下载。IAB/桌面入口不可用不代表仓库的 Electron 测试入口不可用。

未登录、修改或探测用户的 Google 账号地区，未更改出口或账号。新增测试仅使用本地 Node 假 CLI、内存数据库及受控供应商，不发送模型生成请求。既有测试使用临时 hub；`HubConfig::new` 默认关闭真实账号发现。

## 修复前后测量

`model_listing_does_not_run_quota_reports` 使用真实 Node 子进程：`models` 立即返回，其他命令等待 800 ms，所有调用写入临时文件。

| 测量 | 修复前 | 修复后 |
| --- | --- | --- |
| Antigravity 模型查询 | 约 1117 ms；测试失败 | 约 201 ms；测试通过 |
| 实际 CLI 调用 | `models`、`-p /usage --output-format json`、`-p /credits --output-format json` | 仅 `models` |
| 供应商被信号量挂起时连续 10 次 hub 模型读取 | 未采集基线数值；静态代码为逐供应商、逐账号等待 | 约 1.9 ms；只启动一个后台读取 |
| mock UI 冷 Work→Chat 首次模型 chip 可用（人为目录延迟 800 ms） | 854 ms | 915 ms |
| 已加载页面内 Work→Chat / 从用量页返回首页 | 3 ms / 891 ms | 2 ms / 1 ms |
| 上述 UI 流程 `/plugins` 读取次数（返回前→返回后） | 3→5 | 1→1 |
| 原生滚轮边界传递 | 代码区与展开通知在上下边界均阻断外层 | 上下边界传递到会话；所有测量 `window.scrollY = 0` |

这是单次开发机样本，包含 Node 启动开销，不是延迟 SLA。回归断言检查调用记录和请求合并，不依赖上述毫秒数。

静态结论：原来的 `models()` 先等待 `status()`，后者包含两个额度报告；`GET /plugins` 逐项等待这些读取。原来的全局 `* { overscroll-behavior: none }` 阻止所有嵌套区域的边界传递。代码中未发现需要替换的全局 wheel 拦截器。

## 返工：真实 React 编排与 mock UI 实测

新增 `npm --prefix apps/gui run test:browser`：Vite 导入生产 `hub.ts`，Electron 的真实 DOM 中用 React `createRoot`/`act` 挂载 hooks；只替换 bridge 和 API 边界，不替换 React、hub.ts 或 ResourceCache。8 组检查通过（13 次目录读取、28 次普通资源读取）：

- 冷挂载在版本校验后开始读取；多个订阅者共用一个目录请求。
- 卸载再挂载保留可用目录，无额外请求；过期再挂载保留旧列表，同时仅一次后台重验。
- transport 仍为 connected 时切换连接身份，立即清空两类旧数据；旧目录及普通资源响应均不能回填。
- 延迟返回的 `connection.get()` 不能覆盖更新的 `onState` 身份。
- 断线期间不读目录；重连后仍挂载的 `useHubData` 和模型订阅恢复。
- `plugin.changed`（含可见性）、account created/removed/login_finished/quota_updated、CLI 安装事件的突发通知各合并为一次刷新；focus 刷新同样合并。
- 同一 transport 的版本检查期间拒绝之前的普通资源响应，检查完成后重新加载；版本不兼容后切换到兼容连接也恢复，不永久 loading。

回归发现并修复：首轮 `usePlugins.ensure`、手动 reload 和事件刷新可以在版本尚未确认时发送请求；普通资源在纯版本重查期间的旧响应也可能重新填入被清空的页面。本轮加连接就绪检查、取消版本检查前的计时器并使旧普通资源请求失效。目录在版本成功后立即 ensure，不把首次可用连接延迟到 250 ms 的事件合并窗口。`reload` 保持稳定函数身份。

对首轮交付 `e82a1d5` 跑同一集成入口，确实失败于 `no resource requests before version validation`，不是仅验证辅助模块。最终修改版通过。当前仍挂载页面已覆盖连接身份、重连、相同 transport 版本重查、错误版本再切换四条恢复路径。

UI 使用同一 `model-search` 和 `diagram` 数据、1280×800、Electron 44.6.0 / Chromium 152.0.7977.130。基线由 Vite 的测试专用 load 插件读取 `git show f69d86e:...` 的八个已修改 GUI 源文件（含所有受影响模型选择器），不切换 checkout 或改动生产源文件。禁用场景的定时自动点击脚本，测试按当前 DOM 的 `.chip-model button` 手动驱动 Work/Chat、路由和菜单；仅 `/plugins` 人为等待 800 ms。冷启动数值包含 mock 的 120 ms 连接建立、版本确认和轮询开销：**没有证明冷启动变快**。这里测到的改善是返回页面复用和请求合并；供应商解耦另由前面的 CLI/hub 受控测试证明。

原始数值与每一步的内层/外层/侧栏/窗口位置保存在 [基线 JSON](model-loading-scroll-baseline.json) 和 [修改版 JSON](model-loading-scroll-modified.json)。原生滚轮通过 Electron `sendInputEvent(mouseWheel)` 注入，没有 DOM `dispatchEvent(wheel)`、`preventDefault` 或测试专用 overscroll 覆盖。每个新手势前等待 650 ms 结束 Chromium 的滚轮目标锁定。

| 区域 / 操作 | 基线实测 | 修改版实测 |
| --- | --- | --- |
| 纵向代码区（生产 `.cli-out` 样式）中部 | 内层 0→160，外层不动 | 内层 0→160，外层不动 |
| 代码到底 / 到顶 | 外层 316→316 / 356→356，阻断 | 外层 316→476 / 356→196，传递 |
| 展开通知 `.ask-text.opened` 中部 | 内层 0→160，外层不动 | 内层 0→160，外层不动 |
| 展开通知到底 / 到顶 | 外层 33→33 / 73→73，阻断 | 外层 33→193 / 73→0，传递 |
| 横向 `.diffstat` 代码 | scrollLeft 0→160 | scrollLeft 0→160；到底无外层/侧栏位移 |
| Home 真实 portal 模型菜单 | 能滚动，边界无窗口或侧栏位移 | 能滚动，边界无窗口或侧栏位移 |
| Session 真实 textarea，200 行 | 内层约 0→90 | 内层约 0→90；到底会话/侧栏不动 |
| 左 sidebar 长列表 | 首个手势内层未移动（记录原值，不标通过） | 0→160；到底不影响会话 |
| 真实 activity 工具栏，填充长内容 | 0→160 | 0→160；到底左右栏及会话不互相带动 |
| `.dialog-backdrop` + `.dialog` 长弹层 | 0→160 | 0→160；到底不影响会话/侧栏 |
| 所有上述操作 | `window.scrollY = 0` | `window.scrollY = 0` |

代码、展开通知、横向文本、弹层以**受控 DOM 长内容**复用生产 CSS，并插入真实会话/页面；textarea、模型菜单、侧栏和 activity 栏是实际组件，另加长内容以造成溢出。因此它验证原生布局与 CSS 滚动行为，不能代替完整通知展开、所有弹层打开关闭、计划面板搜索、多窗口、实际账号登录/CLI 安装卸载流程。未测试物理触控板惯性、macOS 橡皮筋视觉回弹；无窗口位移只对本机 Windows 隐藏 Electron 的这些原生滚轮样本成立。未消费真实模型请求，未操作 Google 登录。

可复核命令（仓库根目录；Windows 上 `crossagents` 与 `npm.cmd` 使用安装路径也等价）：

```text
crossagents exec --resource build -- npm.cmd --prefix apps/gui run test:browser
crossagents exec --resource build -- node apps/gui/scripts/browser-regression.mjs --baseline=e82a1d5
crossagents exec --resource build -- node apps/gui/scripts/browser-regression.mjs --ui --baseline=f69d86e --out=apps/gui/docs/model-loading-scroll-baseline.json
crossagents exec --resource build -- node apps/gui/scripts/browser-regression.mjs --ui --out=apps/gui/docs/model-loading-scroll-modified.json
crossagents exec --resource build -- npm.cmd --prefix apps/gui run typecheck
crossagents exec --resource build -- npm.cmd --prefix apps/gui test
crossagents exec --resource build -- npm.cmd --prefix apps/gui run build
```

第二条是预期失败的前版集成对照；其余通过。基线 UI 断言确认原来的边界阻断，修改版 UI 断言要求内层优先、上下边界传递、栏间独立及无窗口位移。最初直接运行既有 screenshot 的 `model-search` 自动脚本曾报 `model search input missing`（旧 title 选择器），截图进程仍退出 0；未将其记作 UI 测试通过。新入口使用当前组件选择器并以断言失败返回非零。调试阶段也曾因滚轮事务尚未结束得到无位移，已固定手势间隔；最初要求基线左栏通过的断言也失败，保留基线原值，最终只要求修改版通过该项。

本轮最终 GUI `typecheck`、`test:browser`、`test`、`build` 均退出 0（现有 Electron 单测 101/101；Vite 3714 模块，保留大 chunk 提示）。`git diff --check` 通过。本轮未修改 Rust，未重跑整个 Cargo workspace；下文 Cargo 记录来自首轮，含首次失败和后续通过，不能抹掉首次失败。

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

## 首轮浏览器检查计划（本轮已部分补充，限制见上文）

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

以上为首轮计划；本轮已补充的项目及范围以上面的实测表为准，剩余完整交互与平台限制仍不能宣称通过。

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
