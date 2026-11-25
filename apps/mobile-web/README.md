# CrossAgents 移动版 PWA（WASM `iroh` + 免费 iOS 锁屏推送）

无需 Apple 开发者年费、无需 7 天续签、手机和电脑两端都**不占 VPN** 的 CrossAgents iPhone/移动端客户端。

## 架构概览

三件事完全解耦：
1. **主屏 PWA 负责打开**：托管在专属固定 HTTPS 静态源（如 GitHub Pages），iPhone Safari「添加到主屏幕」后全屏独立运行、永不过期。
2. **浏览器 WASM 版 `iroh` 负责端到端加密连接（`apps/mobile-web/wasm`）**：直接编译现有 `iroh 1.4.1` 协议到 WebAssembly，复用桌面端「设置 → 连接」生成的 `ca1-...` 一次性配对二维码与 `cad_...` 凭据，经加密中继直连中心 Hub（不向公网暴露本地端口，也不占用手机 VPN）。
3. **Hub 直发 Web Push（VAPID）负责锁屏叫人**：中心收到 `human.asked` 时，通过纯 Rust `web-push-native` 向 iOS 主屏 PWA 推送脱敏锁屏提醒（锁屏只显示「有一项待处理」，不泄露命令/路径），点开通知后实时核验中心状态再确认批准。

> 同时内置 **HTTPS 直连备选通道**（例如 Tailscale Funnel / Cloudflare Tunnel），若个别网络下公共中继延迟偏高，可不改一行界面直接切换。

## 首次使用（iPhone）

1. 在 iPhone Safari 打开部署好的固定 HTTPS 地址（如 GitHub Pages）。
2. 点底部 **分享 → 添加到主屏幕**。
3. 从桌面新生成的 **CrossAgents** 图标打开 → 点「扫描电脑端配对二维码」扫描中心电脑「设置 → 连接 → 让别的电脑连进来」生成的二维码。
4. 配对完成后进入 **概览 → 开启免费锁屏推送**，点「发一条测试通知」验证锁屏横幅提醒。

## 本地构建

```bash
# 1. 编译 Rust WASM（需要 clang 支持 ring wasm32）
cd apps/mobile-web/wasm
cargo build --release --target wasm32-unknown-unknown
wasm-bindgen target/wasm32-unknown-unknown/release/crossagents_link_wasm.wasm \
  --target web --out-dir ../src/wasm-pkg --no-typescript

# 2. 构建前端 PWA
cd ..
npm ci
npm run build
```
