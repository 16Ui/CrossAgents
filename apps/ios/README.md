# CrossAgents iOS 客户端（MVP）

原生 SwiftUI + 内嵌 Rust `crossagents-link`（iroh QUIC 打洞，无子进程）的 iOS 客户端，让 iPhone 随时随地直连你的 Windows / Linux 中心 Hub，查看并指挥所有机器（Windows 中心、MacBook-Pro 等）上的 Agent。

## 已包含功能

1. **扫码 / 配对码一键连接**：调用 iPhone 相机扫描桌面端「设置 → 连接 → 让别的电脑连进来」生成的配对二维码（也支持粘贴 `ca1-...` 配对码），凭据存入 iOS Keychain，并向中心上报 `ios` 设备图标。
2. **多机实时对话**：按机器（Windows 中心 / MacBook-Pro 等）分组列出所有会话，SSE 实时流式接收输出、思考过程和可折叠工具调用卡片；支持发送消息、打断停止、插队发送、切换模型/推理力度，以及**新建对话**（选目标机器 + 远程浏览目录 + 选 CLI/模型）。
3. **手机传图 & 待办审批收件箱**：聊天框原生接入相册 `PhotosPicker`（自动压缩上传图片给 Agent 分析）；独立「待办」Tab 带未处理红点角标，一键点击选项或自定义回复所有 `human_ask` 提问与审批。
4. **订阅额度 & 在线节点概览**：查看各订阅账号剩余配额与重置倒计时、在线执行节点与已安装 CLI、当前隧道路径（直连 / 经中继）及手动重连。

## 在 MacBook-Pro 上编译运行

### 前置依赖（一次性安装）

1. **安装 Xcode**：从 Mac App Store 安装 Xcode，安装完成后执行：
   ```bash
   sudo xcode-select -s /Applications/Xcode.app/Contents/Developer
   sudo xcodebuild -license accept
   ```
2. **安装 Rust 工具链**（如尚未安装）：
   ```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
   source "$HOME/.cargo/env"
   ```
3. **安装 XcodeGen**：
   ```bash
   brew install xcodegen
   ```

### 一键编译与运行

```bash
cd ~/CrossAgents
git pull
./apps/ios/scripts/setup-and-build.sh
open apps/ios/CrossAgents.xcodeproj
```

打开 Xcode 后：
- **先跑模拟器**：顶部设备选任意 iPhone 模拟器（如 iPhone 16 Pro），直接按 `⌘R`。
- **装到真机 iPhone**：
  1. USB 连接 iPhone，在 iPhone 上开启「设置 → 隐私与安全性 → 开发者模式」。
  2. 在 Xcode 左侧点 `CrossAgents` 项目 → `Signing & Capabilities` → `Team` 选择你的免费个人 Apple ID。
  3. 如果提示 Bundle Identifier 被占用，把 `dev.crossagents.ios` 改成带自己后缀的唯一标识（例如 `dev.crossagents.ios.link`）。
  4. 顶部设备选你的 iPhone，按 `⌘R` 安装；首次运行需在 iPhone「设置 → 通用 → VPN 与设备管理」信任你的开发者证书。

> 提示：免费个人 Apple ID 签名的真机 App 有效期为 7 天，到期后连上 Mac 在 Xcode 重新按一次 `⌘R` 即可续期，Keychain 中的配对信息会保留。
