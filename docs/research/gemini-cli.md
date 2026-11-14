# gemini-cli 记录

2026-11-15，对接 `gemini-native` 驱动前的协议梳理。依据是 Gemini CLI **0.68.0**（`npm @google/gemini-cli`）装到临时目录后的实际文件与命令输出：`bundle/*.js` 里的 stream-json 写入点、随包文档（`docs/` 下 `headless.md`、`cli/configuration.md`、`cli/trusted-folders.md`）、`gemini --help` / `--list-models` / `--version` 的实测输出。本文标「实测」的是这台机器上跑过的；标「bundle 证据」的是从打包代码里读出来的。

## 结论

走 `gemini --prompt=<text> -o stream-json`，每轮一个进程，会话靠 `--session-id` / `--resume` 在 Gemini 侧续上。输出是 NDJSON，一行一个事件。能拿到的比 Claude 侧少：没有逐次权限询问（headless 下 `ask_user` 一律按 `deny` 处理，只剩 `--approval-mode` 的几档），没有 `--system-prompt`（开场说明得前置进第一条 prompt），没有逐 token 事件，但终端 `result` 带 `stats`，驱动直接上报用量。因此能力表里 `permission_callback` 报 false，`usage` 为 true，`move_cwd` 为 false（会话按项目目录存放）。

## 命令行

| 参数 | 作用 |
|---|---|
| `-p` / `--prompt <text>` | 非交互，跑一轮、打印结果后退出；空值时追加 stdin 的内容 |
| `-o stream-json` | NDJSON 事件流，一行一个 JSON |
| `--session-id <uuid>` | 用指定 id 开一个新会话（hub 自己选 id，第一轮用它） |
| `--resume <id>` | 接着某个会话跑，上下文延续 |
| `--model <id>` | 本轮用哪个模型 |
| `--approval-mode <mode>` | `default` / `auto_edit` / `yolo` / `plan`，见下文 |
| `--skip-trust` | 非交互下跳过「是否信任这个文件夹」的提问，否则新目录里第一轮直接停住 |
| `--list-models` | 打印可用模型，文本格式 |
| `--version` | 版本号，一行 |

没有 `--system-prompt`、`--user-rules`、`--mcp-config`，也没有 cancel 子命令。

### 长提示走 stdin

Windows 命令行上限 32767 个 UTF-16 单元，多附件或长开场说明容易顶到。CLI 的规则是：`--prompt` 为空字符串时，把 stdin 的内容当作 prompt（有值则追加）。驱动因此把超过 16000 字节的消息写成 `--prompt=` + stdin 文本，其余照常放进 argv。

`--prompt=` 而不是 `-p <text>`：以 `-` 开头的消息会被当成另一个 flag。

## stream-json 的输出

每行一个 JSON 对象，`type` 区分种类：

| 事件 | 内容 |
|---|---|
| `init` | 开场信息：`session_id`、`model` |
| `message` | `role` 为 `user`（我们发的那条回显）或 `assistant`；`content` 是**增量**文本，`delta: true` |
| `tool_use` | `tool_name`、`tool_id`、`parameters`（工具自己的参数对象） |
| `tool_result` | `tool_id`、`status`（`success` / `error`）、`output`、`error.message` |
| `error` | `severity`（`warning` / `error`）、`message`；轮内错误，之后仍会有 `result` |
| `result` | 一轮的收尾：`status`（`success` / `error`）、`stats`（token 计数） |

要点：

- **答案只有增量**：`assistant` 的 `message` 是逐段推的，没有「最终全文再发一遍」，所以驱动按顺序拼接，不能用末尾 `result` 补文本（它不带文本）。
- **思考过程不在这个流里**：thinking 只在 TUI 显示，headless 的 stream-json 没有对应事件。
- **`user` 回显要丢**：把 `message` 里 `role == "user"` 的行当输入回显跳过，否则第一条 prompt 会在对话里出现两次。
- **工具输出与状态分两行**：`tool_use` 起卡片，`tool_result` 补输出并收尾。

### 退出码

| 码 | 含义 | 驱动处理 |
|---|---|---|
| 0 | 正常结束 | 看 `result.status` |
| 1 | 一般错误（认证、参数、启动失败） | 无 `result` 时按 `Error` 上报，消息取 stderr / 非 JSON 行 |
| 53 | 本轮请求数超过上限 | `StopReason::MaxTurnRequests` |

有 `result` 事件时以它为准；进程退出码只在没有 `result` 时决定成败。

## 会话

- 第一轮 `--session-id=<hub 选的 uuid>` 开新会话；之后每轮（以及 `resume`）用 `--resume=<同一个 id>`。
- CLI 会把会话按**项目目录的哈希**存到 `~/.gemini/tmp/<project hash>/chats/`，所以换 cwd 就找不到会话：`capabilities().move_cwd = false`。
- hub 不存对话内容，会话在 Gemini 侧；hub 重启时正在跑的那一轮丢失，会话本身可以续。
- 如果 `init` 回报的 `session_id` 与我们给的不一致，驱动发一条 `Warning`（按我们的 id 续会失败），但不偷偷改用它的 id。

## 权限

headless 没有逐次询问的通道：策略引擎在非交互模式下把 `ask_user` 一律变成 `deny`。所以只有整轮的档位：

| hub 的 `PermissionPolicy` | `--approval-mode` |
|---|---|
| `Bypass` / `Guard` | `yolo` |
| `Deny` | `default` |

- `Guard` 落到 `yolo`：headless 里没有「问一下」的中间档，只能整轮放行；会话会给一条 `Warning` 说明这件事，`permission_callback` 能力报 false。
- `Deny` 用 `default`：凡是要问的动作都被拒，文件读取这类不问的照常。
- `auto_edit` / `plan` 这两个档位驱动不用：前者只放行编辑，后者只出计划，都不对应 hub 的语义。

打断：没有 cancel 子命令，`interrupt()` 只能杀进程，按 `Cancelled` 上报。

## MCP

没有 `--mcp-config`，MCP 只从设置文件读：

- 用户级 `~/.gemini/settings.json`；
- 系统级，路径由环境变量 `GEMINI_CLI_SYSTEM_SETTINGS_PATH` 指定，**深度合并并覆盖所有其它层**（bundle 证据：`loadSettings` 先读各层再 merge，系统层最后）。

协作工具（`crossagents mcp`，带会话令牌）走系统层：每个会话生成一个临时目录 `<tmp>/crossagents-gemini-<uuid>/settings.json`，把 `mcpServers` 写进去，再用 `GEMINI_CLI_SYSTEM_SETTINGS_PATH` 指过去，用完即删。

细节：

- 每个 server 写成 `{command, args, env, trust: true}`；`trust: true` 是必须的，headless 下未受信的 MCP 工具调用会被拒。
- 如果环境里（或 hub 自己的环境里）已经有 `GEMINI_CLI_SYSTEM_SETTINGS_PATH` 指向一份文件，驱动先读它、把它当底，只往里加自己的 server（CLI 按 server 名合并），不覆盖管理员设置。
- 那份文件读不出来（不是合法 JSON 对象）时**不写**，会话发一条 `Warning`：宁可不挂协作工具，也不要把别人的配置改坏。

## 模型列表

`gemini --list-models` 输出文本，不是 JSON：

```
Available Gemini Models:

auto (Auto)
  Description:   Let Gemini CLI decide
  Context limit: 1,048,576 input tokens

gemini-2.5-pro (gemini-2.5-pro)
  Context limit: 1,048,576 input tokens
```

驱动取「首个词 + 后面括号」的行，首个词当 id；拿不到就退回 `crates/plugins/agents/gemini.toml` 的 `fallback = ["auto", "gemini-2.5-pro", "gemini-3.5-flash", "gemini-3.1-flash-lite"]`。探测在 hub 启动后台跑（20 秒超时），不挡开会话。

## 开场说明与附件

- 没有 `--system-prompt`，开场说明拼在第一条 prompt 前面（`instructions\n\n<text>`），只拼一次，后续轮次靠会话上下文保留。
- 附件转成 `@<path>` 引用追加在消息末尾，Gemini CLI 会把 `@path` 当文件读。

## 登录（2026-11-15 实测）

Gemini CLI 没有 `login` 子命令（`--help` 里只有 `mcp / extensions / skills / hooks / gemma / models`），Google 登录在交互式首启的 TUI 里；没有 TTY 时它直接以 41 退出：

```
Please set an Auth method in <home>\.gemini\settings.json or specify one of the following
environment variables before running: GEMINI_API_KEY, GOOGLE_GENAI_USE_VERTEXAI, GOOGLE_GENAI_USE_GCA
```

**唯一能在 GUI 里完成的办法是 ACP 的 `authenticate`**（实测）：

- `gemini --acp`，先发 `initialize`，回答里有 `authMethods`：`oauth-personal`（Log in with Google）、`gemini-api-key`、`vertex-ai`、`gateway`。
- 发 `authenticate{"methodId":"oauth-personal"}` 后，**CLI 自己用系统默认浏览器打开 Google 授权页**（Windows 上是 `powershell Start-Process <url>`，所以是新的浏览器窗口），进程留在原地等；`gemini-api-key` 则立刻返回 `{}`（只把 key 记下来）。
- 授权完成后 `authenticate` 返回 `{}`，凭据写到 `<home>/.gemini/oauth_creds.json`。
- 实测时机器上没有登录 Google，所以只验证到「浏览器打开了授权页、进程在等」这一步；授权完成后的返回值和文件写入是按代码路径推断的，第一次真正登录时补测。

凭据文件：

| 文件 | 内容 |
|---|---|
| `<home>/.gemini/oauth_creds.json` | `access_token`、`refresh_token`、`expiry_date` |
| `<home>/.gemini/google_accounts.json` | `active`（当前账号邮箱）、`old` |
| `<home>/.gemini/settings.json` | `security.auth.selectedType`：`oauth-personal` / `gemini-api-key` / `vertex-ai` |

`<home>` 由环境变量 `GEMINI_CLI_HOME` 决定（默认 `os.homedir()`），配置目录是 `<home>/.gemini` —— 多账号隔离靠这一个变量。

驱动侧的做法（`crossagents gemini-login`，隐藏子命令）：跑 `gemini --acp`，发上面两条请求，等 `authenticate` 返回；hub 把这条命令当普通登录命令启动，然后轮询账号状态，`oauth_creds.json` 出现就收尾。自动这条路失败时（老版本没有 ACP、浏览器被关掉），账号页的提示会退回「在终端运行 gemini 登录一次」。

## 安装与定位

- npm：`npm i -g @google/gemini-cli`，bin 是 `gemini`；Windows 上是 `node_modules/.bin/gemini.cmd`，里面用 `%dp0%\..` 拼出真正的入口脚本。`crates/drivers/src/locate.rs` 的 `wrapper_script()` 通用地解析这种 npm shim，CodeBuddy 的已知路径只是兜底。
- 环境变量 `GEMINI_CLI` 可指定可执行文件；`[locate] shim = "node-script"`、`version = ["--version"]`。
- 登录态在 `~/.gemini/`（`oauth_creds.json` 等），本记录没有做多账号隔离的实测。
