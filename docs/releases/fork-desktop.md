# Fork 桌面端发布

`16Ui/CrossAgents` 的 Windows Electron 安装包从 fork 的 GitHub Releases 检查更新。在 fork 的 Actions 页面选择 **Fork desktop release**，从 `main` 运行。构建会检查 GUI、Hub 和 Link 的版本一致，验证 `latest.yml` 对应的安装包和差分文件，然后一次发布。已发布的版本不会覆盖。修订版需要先提交新的版本号和锁文件。

已安装且更新源指向这个 fork 的 Windows Electron 客户端会在启动、定时检查或手动检查时发现新版本。更新下载结束后，可以在设置页重启安装。也可以从 [fork 发布页](https://github.com/16Ui/CrossAgents/releases) 下载安装包。

启用 Tauri 更新时，在 fork 的 Actions Variables 设置 `CROSSAGENTS_TAURI_UPDATE_PUBKEY`，在 Actions Secrets 设置配对的 `TAURI_SIGNING_PRIVATE_KEY` 和可选的 `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`。同一流程会加入 Windows x64 和 macOS Apple Silicon 的 Tauri 安装包，以及签名的 `latest.json`。用此 fork 的签名安装包安装一次，后续才能从此源获取签名更新。后续发布需继续使用同一密钥，才能让已安装的客户端验签。
