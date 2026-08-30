<div align="center">

# cc-switch · Antigravity Integration

### 在 Claude Code / Codex CLI 中使用 Google Antigravity（Cloud Code）免费额度

基于 [farion1231/cc-switch](https://github.com/farion1231/cc-switch) 的集成分支 ——
为 cc-switch 新增 **Antigravity (Google)** 托管 OAuth 供应商：
浏览器授权登录 → 本地代理自动把 Claude Code 的请求转换为 Google Cloud Code 协议 → 免费额度直接跑 Claude / Gemini 全系模型。

**协议与实现均经真机验证**（真实 Google 账号 + 真实端点），无任何猜测成分。

</div>

---

## ✨ 这个分支新增了什么

| 能力 | 说明 |
|---|---|
| **Antigravity OAuth 登录** | Google 授权码 + 本地 loopback 回调（与官方 IDE 完全同构）。多账号管理、自动刷新、凭据失效自动标记重新登录 |
| **协议转换** | Anthropic Messages ⇄ Google Cloud Code `v1internal` 双向转换：system / tools / thinking / 多轮工具调用全支持，流式 SSE 与非流式都处理 |
| **模型发现** | 一键拉取真实模型目录（`fetchAvailableModels`），自动过滤上游内部 ID |
| **GUI 全集成** | 供应商预设卡片、登录区块、认证中心、模型获取按钮、中/英/繁/日四语言 |
| **测试覆盖** | Rust 单元测试 15+ 项、前端组件测试、Playwright GUI E2E；协议链路经真实凭据逐环验证 |

上游 cc-switch 的全部功能（Claude/Codex/Gemini 供应商切换、代理接管等）**原样保留**。

## 🧩 工作原理

```
Claude Code ──Anthropic /v1/messages──▶ cc-switch 本地代理 (127.0.0.1:15721)
                                          │ 1. OAuth Bearer 注入（Antigravity client）
                                          │ 2. Anthropic → Cloud Code 信封转换
                                          │    {model, request:{sessionId, contents,
                                          │      tools, toolConfig, generationConfig}}
                                          ▼
                              daily-cloudcode-pa.googleapis.com (生成)
                              cloudcode-pa.googleapis.com  (登录/模型目录)
                                          │ 3. SSE 响应解包 → Anthropic 事件流
Claude Code ◀──Anthropic SSE / JSON────── ┘
```

关键事实（均实测验证，详见 [docs/antigravity-protocol.md](docs/antigravity-protocol.md)）：

- 生成请求必须走 **daily** 域名（prod 一律 429）；登录与模型目录走 prod
- `claude-*` 模型必须带 `toolConfig: VALIDATED`
- 模型名发上游必须是裸名（cc-switch 的 `[1m]` 标记在发送前剥除，仅本地使用）

## 📦 构建教程

### 前置要求

- Node.js ≥ 20、pnpm ≥ 10
- Rust（stable）
- C++ 工具链：
  - Windows：**Visual Studio Build Tools**（勾选 "使用 C++ 的桌面开发"）—— 推荐
  - 或 w64devkit / MSYS2（`build.rs` 已适配 GNU 工具链，自动用 windres 嵌入 manifest）

### 步骤

```bash
git clone https://github.com/zzummy6/cc-switch-antigravity-integration.git
cd cc-switch-antigravity-integration

pnpm install

# 开发模式（带热重载）
pnpm tauri dev

# 正式打包（Windows 产出 .msi / .exe 安装包）
pnpm tauri build
```

> 构建产物在 `src-tauri/target/release/bundle/`。装好后可在设置里把旧版 cc-switch 直接替换。

## 🚀 使用教程（约 2 分钟）

### 1. 添加 Antigravity 供应商

打开 cc-switch → 选择 **Claude Code** → 左上角 **「+」添加新供应商** →
预设列表搜索 **Antigravity** → 点击 **「Antigravity (Google)」** 卡片。

### 2. Google 登录

表单顶部出现「Antigravity OAuth 认证」区块 → 点击 **「使用 Google 登录」**：

1. 程序在 `127.0.0.1:<随机端口>` 启动回调服务，并打开浏览器
2. 在 Google 授权页登录你的账号并点击同意
3. 浏览器自动跳转到 `antigravity.google/auth-success`，回调自动完成
4. 回到 cc-switch，区块徽标变绿并显示你的 Google 邮箱 —— 登录完成

> 凭据保存在 `~/.cc-switch/antigravity_oauth_auth.json`，access token 只存内存、自动刷新。
> 支持「添加账号」登录多个 Google 账号并随时切换默认账号。

### 3. 获取模型列表

在模型输入框旁点击 **获取按钮**，即可拉取当前账号可用的全部模型
（免费层实测 20+ 个，含 `claude-sonnet-4-6`、`gemini-3.7-flash-high`、`gpt-oss-120b-medium` 等）。

### 4. 开启代理接管

勾选/开启 **本地代理接管** 并保存。cc-switch 会把 Claude Code 的
`ANTHROPIC_BASE_URL` 指到本地代理，之后的转换、认证、重试全部自动完成。

### 5. 在 Claude Code 中使用

正常启动 `claude` 即可。模型映射参考（免费层实测可用）：

| Claude Code 槽位 | 推荐上游模型 | 备注 |
|---|---|---|
| Sonnet（主力） | `claude-sonnet-4-6` | 实测最稳，工具调用/长对话正常 |
| Opus | `gemini-3.1-pro-low` | pro 的 `high` 档免费层不可用（400） |
| Haiku（快答） | `gemini-3-flash` | |
| 想要 1M 上下文 | `gemini-3.7-flash-high[1m]` | 带 `[1m]` 标记即可，发上游前自动剥离 |

Codex CLI 同理：Codex 页签下把 API 格式指到本代理即可（Responses→Chat 转换路径与上游一致）。

## 🔧 故障排查

| 现象 | 原因 | 处理 |
|---|---|---|
| 登录回调页 400 | state 不匹配或授权码超时（10 分钟会话） | 重新点「登录」 |
| 登录页报 `redirect_uri_mismatch` | 理论不应出现（Google 对 loopback 任意端口放行） | 带截图提 issue |
| 获取模型 429 | 账号免费额度未开通/耗尽 | 确认 [antigravity.google](https://antigravity.google) 可正常登录使用 |
| 生成 403 `SUBSCRIPTION_REQUIRED (#3501)` | token 不是 Antigravity client 签发（如导入了 gemini-cli 凭据） | 删除账号后走本插件重新登录 |
| 生成 429 `INSUFFICIENT_G1_CREDITS_BALANCE` | 模型 Credits 用尽 | 等待重置或换模型 |
| 生成 503 `No capacity` | 上游该模型临时满载（如 `claude-opus-4-6-thinking`） | 稍后重试或换模型 |
| 400 且消息含 signature | thoughtSignature 回放缺失 | 附 `~/.cc-switch/logs/cc-switch.log` 提 issue（日志已脱敏） |
| Claude Code 报 401 | 路由接管未开启或选错了供应商 | 确认 cc-switch 里已切换到 Antigravity 供应商且代理已接管 |

更多：[完整验证指南](docs/antigravity-e2e-verification.md) · [协议实证文档](docs/antigravity-protocol.md) · [实现分析](docs/antigravity-gap-analysis.md)

## ⚠️ 免责声明

- 本项目使用 Google Antigravity 官方 OAuth 客户端公开凭据与官方授权流程，
  **不接触账号密码/Cookie、不绕过任何同意页**；额度政策由 Google 单方决定，随时可能变化
- 仅用于个人学习研究，请遵守 Google 服务条款；商用或滥用风险自负

## 🙏 致谢

- [farion1231/cc-switch](https://github.com/farion1231/cc-switch) —— 本分支的全部基座（供应商管理/代理/转换框架）
- [antigravity-bridge](https://github.com/yxw.ucar/antigravity-bridge)（社区同类项目）的协议笔记交叉验证了信封格式与 UA 指纹
- Google Antigravity IDE —— OAuth 参数与协议形态的第一手来源

## 📄 License

与上游一致（MIT）。上游 README 见 [README-upstream_ZH.md](README-upstream_ZH.md)。
