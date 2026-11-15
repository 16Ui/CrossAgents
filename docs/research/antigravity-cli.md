# antigravity-cli 记录

Google Antigravity CLI：命令是 `agy`，Go 写的多 agent 终端工具。2026-06 起接替**个人版**
Gemini CLI（免费、Google AI Pro、Ultra）；企业 Gemini Code Assist 和按量 API key 仍走 `gemini`。
所以同一个 Google 账号，Pro 档的新模型在 `agy` 里，不在 `gemini` 里。

本记录对应 `agy 1.5.9`（2026-11 安装）。实测过：安装、`--help`、`models`、`mcp`，以及
未登录时跑一轮 stream-json（见「未登录时的一轮」）。**没有**跑通过成功的一轮（这台机器上
没登录 Google），事件形状与退出码另外取自官方
[headless 文档](https://antigravity.google/docs/cli/headless)，标为「文档」。

## 结论

- 一个进程可以吃多轮：`--input-format stream-json --output-format stream-json`，一条
  `{"event":"user","message":{"content":"…"}}` 一行写进 stdin，每轮以一个 `result` 结束。
  这比 `agy -p` 一轮一进程快，也省掉了每轮的启动。
- 会话 id 是 CLI 给的（`init` 与 `result` 的 `conversation_id`），`--conversation <id>`
  续一个旧会话。进程没了（打断、崩溃）就重开一个，带上这个 id。
- headless 没有逐次审批通道，但**不会因此失败**：需要批准而拿不到的工具被「soft-deny」，
  这一轮继续跑、退出码仍是 0，stderr 写一行说明。
- MCP 没有按会话注入的办法：`--settings` 明确拒绝 `mcpServers`（实测），配置只读
  `~/.gemini/config/mcp_config.json`，而 `agy mcp add` 写的就是这一份（对所有会话生效）。
  驱动的能力表因此报 `mcp: false`。
- 凭据在系统钥匙串（Windows 凭据管理器 / macOS Keychain / Linux secret-service），不在
  `~/.gemini` 里；换 `HOME` 不会换账号。`agy models` 未登录时打印
  `Please sign in to view available models.` 并以 0 退出（实测）。

## 命令行（`agy --help`，实测）

```
-p, --print, --prompt            非交互跑一条提示词，结果打到 stdout
    --output-format              text | json | stream-json（默认 text）
    --input-format               text | stream-json；stream-json 从 stdin 一行一条消息，
                                 每条跑一轮，且必须配 --output-format stream-json
    --conversation <id>          按 id 续会话
-c, --continue                   续最近一次会话
    --model <slug>               本次会话的模型（`agy models` 列）
    --effort <level>             low | medium | high | xhigh | max
    --mode <mode>                accept-edits | auto | plan（默认模式即不传）
    --dangerously-skip-permissions   所有工具调用一律批准
    --sandbox                    终端沙箱
    --add-dir <path>             额外工作目录（可重复）
    --settings <json|path>       只对本次会话生效的设置，不落盘（未知键会报错）
    --print-timeout <dur>        print 模式的时限，0 表示不限
```

子命令：`acp`（ACP stdio 服务）、`models`、`agents`、`mcp add|remove|list|enable|disable`、
`plugin`、`install`、`update`、`changelog`。

`--settings` 的键是 `settings.json` 的键：`{"nonsense":1}` 会报
`--settings: unknown setting "nonsense"`，`{"mcpServers":…}` 同样报未知（实测）。

## stream-json 的输出（文档）

一行一个 JSON 对象，顺序是 `init` → 若干 `step_update` → `result`：

```json
{"event":"init","conversation_id":"c3b6…","init":{"cwd":"/w","tools":["run_command"],"permission_mode":"request-review"}}
{"event":"step_update","step_update":{"conversation_id":"c3b6…","step_index":2,"state":"ACTIVE","step_type":"agent_response","text_delta":"Git rebase "}}
{"event":"step_update","step_update":{"conversation_id":"c3b6…","step_index":4,"state":"DONE","step_type":"tool","tool_name":"run_command","tool_info":{"name":"run_command","parameters":{"CommandLine":"echo hi"},"output":"hi\r\n"}}}
{"event":"result","result":{"conversation_id":"c3b6…","status":"SUCCESS","response":"…","duration_seconds":6.9,"num_turns":1,"usage":{"input_tokens":10418,"output_tokens":589,"thinking_tokens":551,"cache_read_tokens":8113,"total_tokens":11007}}}
```

- `step_update.state`：`ACTIVE` 进行中，`DONE` 结束；`step_type`：`user_input`、
  `agent_response`、`tool`、`checkpoint`。回答的分片在 `agent_response` 的 `text_delta`，
  流式时先若干 `ACTIVE` 再一个 `DONE`。
- 工具步骤的 `tool_info` 有 `name`、`parameters`、`output`，失败时 `error:{type,message}`。
- `result.usage` 是**整个会话的累计**，不是这一轮；`response` 才是这一轮的。
- `status`：`SUCCESS`、`ERROR`、`CANCELED`、`INTERRUPTED`、`INVALID`、`WAITING`、`RUNNING`。
- 退出码：成功 0；失败非 0（`ERROR` 时 `error` 里有原因）。stream-json **输入**模式下，
  控制消息或 CLI 自己处理的斜杠命令（`/model`）会让会话以 2 结束，坏 JSON / 缺 `event`
  / 非 `text` 内容块以 1 结束。

## 权限（文档）

- 默认（不传 `--mode`）：需要批准的动作在 headless 下**soft-deny**，这一轮继续。
- `--mode accept-edits`：改文件自动批准；`auto` 还要沙箱，部分账号不可用；`plan` 只读。
- `--dangerously-skip-permissions`：全批准，`init.permission_mode` 变成 `always-proceed`。
- 细粒度规则写在 `~/.gemini/antigravity-cli/settings.json` 的 `permissions.allow`
  （`command(git)`、`write_file(src/)` 这种）。

驱动把 `PermissionPolicy` 映射成：`Bypass` → `--dangerously-skip-permissions`，
`Guard` → `--mode accept-edits`，`Deny` → 不传（默认模式，会问的一律拒）。

## MCP（实测）

- `agy mcp add <name> <command> [args…]` 写到 `<home>/.gemini/config/mcp_config.json`，
  形如 `{"mcpServers":{"<name>":{"command":"…","disabled":false}}}`。
- `agy mcp list` 只列这一份全局配置；工作区 `.agents/mcp_config.json` 不会被它列出来。
- 没有 `--mcp-config` 一类的按会话入口；`--settings` 不接受 `mcpServers`；`--gemini-dir`
  只有 `acp` 子命令有。所以驱动挂不了协作工具，能力表 `mcp: false`，会话里发一条说明。

## 未登录时的一轮（实测）

把一条 `user` 消息写进 stdin 后，stdout 什么都没有，**stderr** 打印：

```
Authentication required. Please visit the URL to log in:
  https://accounts.google.com/o/oauth2/auth?…redirect_uri=https%3A%2F%2Fantigravity.google%2Foauth-callback…
Waiting for authentication (timeout 60s)...
Or, paste the authorization code here and press Enter:
```

然后它**在 stdin 上等授权码 60 秒**；没人贴就以 1 退出，并补一条
`{"event":"result","result":{"status":"ERROR","error":"authentication failed or timed out",…}}`。

两个结论：驱动在没有登录时会卡住一轮（约 60 秒）再报错，这是 CLI 的行为，不是驱动加的；
另外 headless 其实可以收授权码，所以「在 GUI 里登录」这条路是通的（读 stderr 的链接、把码
写回 stdin），只是这一版没做。

## 会话与恢复

- `--conversation <id>` 按 id 续；`-c` 续最近一次。
- 会话文件按项目存（`~/.gemini/projects.json`、`~/.gemini/tmp/<project>/`），但换目录能否
  续没有实测，驱动的 `move_cwd` 保守报 false。
- 进程常驻时不需要 `--conversation`：会话就在这个进程里。

## 模型（实测 + 文档）

- `agy models` 需要登录：未登录打印 `Please sign in to view available models. Launch the CLI
  without arguments to sign in.`，退出码 0。
- 文档里的 slug 形如 `gemini-3.8-flash-medium`（effort 可以并进 slug），也支持
  `--model <slug> --effort <level>` 分开给。`--model` 写错时 headless **不会**回退，
  直接以 1 退出并把可用模型列在 `error` 里。
- 驱动解析 `agy models` 的文本：取像 slug 的 token（小写、含数字、含 `-`/`.`）或 `auto`，
  去重保序；解析不出来就用描述文件里的 fallback。

## 登录与凭据（文档）

- 首次跑 `agy` 走交互式引导：选主题、接受条款、信任目录，然后是 Google OAuth。
- 凭据存在系统钥匙串（Apple Keychain / Linux secret-service / Windows Credential Manager），
  不是 `~/.gemini` 下的文件。所以：换 `HOME`/`USERPROFILE` 不会换账号，也不会丢登录；
  headless 直接用缓存凭据，没有缓存时报 `authentication required` 而不是挂起。
- 配置：`~/.gemini/antigravity-cli/settings.json`（键 `model`、`agentMode`、`toolPermission`
  等）、`~/.gemini/antigravity-cli/updater/`（自更新锁，可用 `AGY_CLI_DISABLE_AUTO_UPDATE=true`
  关掉）、`~/.gemini/config/mcp_config.json`、工作区 `.agents/`（skills、rules、plugins）。

## 安装与定位（实测）

- Windows：`irm https://antigravity.google/cli/install.ps1 | iex`，落到
  `%LOCALAPPDATA%\agy\bin\agy.exe`。脚本从
  `https://antigravity-cli-auto-updater-974169037036.us-central1.run.app/manifests/windows_amd64.json`
  取版本、URL 和 sha512，校验后复制；随后跑 `agy install` 改 PATH。
- macOS / Linux：`curl -fsSL https://antigravity.google/cli/install.sh | bash`，落到
  `~/.local/bin/agy`。
- 自更新：`agy update`，或后台自动（`~/.gemini/antigravity-cli/updater/update.lock`）。
- 国内网络注意：`storage.googleapis.com` 下载二进制这一跳经常握手失败，`antigravity.google`
  和 Cloud Run 的 manifest 却能通；代理要能到 Google Storage 才行。
