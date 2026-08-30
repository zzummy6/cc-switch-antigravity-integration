# Antigravity 集成 · 真实端到端验证指南

> 自动化已覆盖：协议验证（真实 Google 端点，见 `antigravity-protocol.md` §3.2）、
> Rust 单元测试（OAuth 存储 / loopback 回调 / Cloud Code 信封 / SSE 解包 / 端点重写）、
> 前端组件测试（4 项）、Playwright GUI E2E（2 项）。
>
> 唯一无法自动化的环节是 **Google 浏览器授权**（consent 需要账号持有者本人完成，
> 自动化不得触碰账号密码/Cookie，见 antigravity-bridge 同类约束）。
> 本文给出用户参与的最终验证步骤（约 2 分钟）。

## 前置条件

- Windows + 本仓库构建的 cc-switch（MSVC 工具链，`pnpm tauri dev` 或 `pnpm tauri build`）。
- 可访问 `accounts.google.com` 与 `cloudcode-pa.googleapis.com` 的网络
  （cc-switch 的全局出站代理已支持，本机为 `127.0.0.1:7897`）。

## 步骤

1. 启动 cc-switch → **Claude Code** → 添加供应商 → 搜索 **Antigravity (Google)** → 选中预设。
2. 表单顶部出现「Antigravity OAuth 认证」区块 → 点击 **使用 Google 登录**：
   - 后端在 `127.0.0.1:<随机端口>` 启动 loopback 回调（`/oauth-callback`）；
   - 浏览器打开 Google 授权页（client `1071006060591-...`，5 个 scope，`access_type=offline&prompt=consent`）；
   - 登录并同意 → 浏览器被 302 到 `https://antigravity.google/auth-success?app=Antigravity`；
   - cc-switch 自动完成 code 交换（`oauth2.googleapis.com/token`）并落盘
     `~/.cc-switch/antigravity_oauth_auth.json`（0600/Windows 等效 ACL）。
3. 验证凭据落盘：区块 Badge 变绿显示「1 个可用账号」，账号名显示 Google 邮箱。
4. **模型发现**：表单中模型列表（或模型获取按钮）触发
   `get_antigravity_oauth_models` → `POST /v1internal:fetchAvailableModels`（空 body）
   → 返回真实模型目录（应包含 `gemini-3-flash`、`claude-sonnet-4-6` 等；
   `chat_20706` 等内部 ID 已过滤）。
5. **启用路由接管**并保存供应商 → Claude Code / Codex CLI 的请求被本地代理转换为
   Cloud Code 信封（`{model, userAgent:"antigravity", requestType:"agent", request:{sessionId, contents, toolConfig...}}`）
   → `POST /v1internal:streamGenerateContent?alt=sse` → SSE 事件解包
   （`{"response":{...}}` → Anthropic 事件流）。
6. **冒烟用例**（Claude Code 中逐条验证）：
   - 纯文本：「你好，自我介绍」→ 正常流式回复；
   - 工具调用：让 Claude Code 列目录/读文件 → `functionCall`/`functionResponse` 往返正常；
   - 多轮 + thinking：连续对话不出现 400（thoughtSignature 回放由 shadow store 处理）。
7. **多账号**（可选）：重复「添加账号或重新登录」用第二个 Google 账号登录；
   「设为默认」切换后新请求使用对应 token。

## 故障速查

| 现象 | 含义 | 处理 |
|---|---|---|
| 回调页 400 invalid code/state | state 不匹配或 code 过期 | 重新点登录（会话 10 分钟有效） |
| 登录页报 `redirect_uri_mismatch` | 理论不应出现（loopback 任意端口被允许） | 附截图反馈 |
| `fetchAvailableModels` 429 | 配额耗尽/未开通 | 确认账号已开通 Antigravity 免费层 |
| 生成 403 SUBSCRIPTION_REQUIRED (#3501) | token 不是 Antigravity client 签发 | 检查是否用了导入的 gemini-cli 凭据，需重新走本插件登录 |
| 生成 429 INSUFFICIENT_G1_CREDITS_BALANCE | 模型 Credits 用尽 | 等待重置或切换模型 |
| 400 且消息含 signature | thoughtSignature 回放缺失 | 抓取 `cc-switch` 日志反馈（日志已脱敏） |
