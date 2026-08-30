# Antigravity 集成 Gap Analysis（cc-switch 现状 vs 目标）

> 配合 `docs/antigravity-protocol.md`（协议实证）阅读。
> 日期：2026-08-29。

## 1. 目标

在 cc-switch 中新增 "Antigravity" provider：

1. **OAuth 登录**：Google authorization_code + loopback（Antigravity client），
   多账号管理、refresh token 持久化、自动刷新。
2. **协议转换**：把 Claude Code / Codex CLI 的请求（本地代理入口）转换为
   Cloud Code `v1internal:streamGenerateContent`（Gemini/aiplatform 格式），SSE 回转。
3. **模型发现**：`fetchAvailableModels` → 动态模型列表。
4. **GUI**：Provider 卡片（OAuth section、账号选择、配额展示）、Doctor 诊断。
5. **测试**：Rust 单元测试 + 真实端到端 + Playwright E2E。

## 2. cc-switch 现状（关键结论）

### 2.1 架构

- Rust 后端：Tauri v2 + SQLite（providers 表）+ 本地 HTTP 代理（`proxy/server.rs`）。
- 前端：React + TypeScript，`src/components/providers/forms/` 表单体系。
- Provider 抽象：`ProviderType`（枚举）+ `ProviderAdapter`（trait）+ `AuthStrategy`（header 形状）。
- OAuth 现有实现：**全部是 Device Code 流**（Copilot / Codex / xAI），无本地回调端口先例。
- 凭据存储：`~/.cc-switch/{codex_oauth_auth.json,xai_oauth_auth.json,copilot_auth.json}`，
  明文 JSON + 原子写盘（tmp+rename+0600），access_token 仅内存缓存。
- 前端 OAuth：provider 无关的 `auth_*` 命令族 + `useManagedAuth` 轮询 hook
  （`startLogin → openExternal → poll`），新增 provider 基本零改动。

### 2.2 与 Antigravity 需求的差距

| # | 差距 | 说明 | 难度 |
|---|---|---|---|
| G1 | **OAuth 流程不同** | Antigravity 是 authorization_code + loopback server（随机端口、`/oauth-callback`）；现有代码无本地 HTTP listener 先例 | 中 |
| G2 | **上游 API 格式全新** | Cloud Code v1internal（Gemini aiplformat 变体）；cc-switch 现有 Claude/Codex/Gemini 三种 wire format 均不匹配，需要新的 transform 层（Anthropic→GeminiContents 双向转换，含 tool use/tool result/SSE 事件映射） | 高 |
| G3 | **模型目录动态** | 现有 provider 模型多为静态映射；Antigravity 需要 `fetchAvailableModels` 动态拉取 | 低 |
| G4 | **ProviderType/命令注册分散** | 新 provider 需要在 mod.rs/auth.rs/claude.rs/forwarder.rs/commands/lib.rs 多处 match 分支中注册（无注册表模式） | 中（量大但机械） |
| G5 | **配额/用量** | Antigravity free tier 有配额（模型Credits）；cc-switch 已有 codex/xai quota 命令模式可循 | 低 |
| G6 | **Google API 走代理** | 本机网络需代理访问 googleapis.com；cc-switch 的 `http_client::get()` 已支持全局出站代理 | 低 |

## 3. 实现方案（对应任务要求）

### 3.1 OAuth（新建 `src-tauri/src/proxy/providers/antigravity_oauth_auth.rs`）

- `AntigravityOAuthManager`：模仿 `xai_oauth_auth.rs` 的 Manager/Store/账号模型
  （`{version, accounts, default_account_id}`，文件 `antigravity_oauth_auth.json`）。
- **新增 loopback 组件** `antigravity_loopback.rs`：
  - `start(port=0)` → 返回 `redirect_uri = http://localhost:{port}/oauth-callback`
  - 等待 GET `/oauth-callback?code=...&state=...`（校验 state），成功后
    302 → `https://antigravity.google/auth-success?app=Antigravity`
  - 10 分钟超时；端口占用时自动换端口（随机端口本来就不会冲突）。
- 登录 URL：`accounts.google.com/o/oauth2/v2/auth`，`access_type=offline&prompt=consent` + 5 scopes + state(PKCE 可选——IDE 未用 PKCE，保持一致)。
- 交换/刷新：POST `oauth2.googleapis.com/token`（form）。
- 命令族：`auth_start_login`（返回打开的 URL + redirect_uri）/ `auth_poll_for_account`
  改为「等待 loopback 结果」的异步等待（对前端语义不变）。
- GCP TOS client（884354919052）作为高级选项，默认不用。

### 3.2 Transport（新建 `proxy/providers/antigravity.rs` + `proxy/gemini_cloudcode.rs`）

- `AntigravityAdapter`：
  - `extract_base_url` → 固定 `https://cloudcode-pa.googleapis.com`
  - `extract_auth` → 占位符 + `AuthStrategy::AntigravityOAuth`
  - `build_url` → `/v1internal:streamGenerateContent?alt=sse`
  - `get_auth_headers` → `Authorization: Bearer {token}`
  - `transform_request`：Anthropic Messages → CloudCode GenerateContentRequest
    （system → systemInstruction；tool_use/tool_result → functionCall/functionResponse parts；
    max_tokens → generationConfig.maxOutputTokens(int32 上限内)）
  - `transform_response`：SSE GenerateContentResponse 流 → Anthropic SSE
    （message_start / content_block_delta(functionCall→tool_use) / message_stop）
  - metadata 固定注入 `{"ideType":"ANTIGRAVITY","platform":"PLATFORM_UNSPECIFIED","pluginType":"GEMINI"}`

### 3.3 模型发现（`services/antigravity_models.rs`）

- 调 `fetchAvailableModels`，映射为 cc-switch 的 `FetchedModel{id, owned_by}`。
- 默认 fallback 列表（协议文档 §3.6）。

### 3.4 GUI

- `OAUTH_PROVIDER_TYPES` + `ManagedAuthProvider` 加 `antigravity_oauth`。
- 预设：`Antigravity (Google)` 卡片，`providerType:"antigravity_oauth"`, `requiresOAuth:true`,
  `apiFormat:"antigravity"`，图标 `antigravity`。
- 登录 UI 复用 `useManagedAuth`（`startLogin` 打开浏览器，轮询 loopback 完成状态）。
- 配额 footer：模型 Credits（state.vscdb 中的 modelCredits 对应 API 字段，可后补）。

### 3.5 测试

- Rust：transform 单元测试（Anthropic↔Gemini 双向，含工具调用、多模态）；
  loopback server 测试（模拟回调）；OAuth 存储测试。
- 真实端到端：本机 Google 账号走完整 OAuth → fetchAvailableModels →
  streamGenerateContent（含 tool use）→ Claude Code / Codex CLI 实连。
- Playwright：GUI 登录流程（mock 上游）+ provider 切换。

## 4. 风险

| 风险 | 缓解 |
|---|---|
| Antigravity client 可能有 client 级限制（UA/originator 校验） | 首次真机 OAuth E2E 验证；必要时补 header 指纹 |
| free-tier 配额策略未知（429/403 语义） | 按实测错误码映射：429→限流提示；403 SUBSCRIPTION_REQUIRED→需登录/订阅 |
| SSE 转换复杂（tool use 分片、thinking 块） | 分层测试；先支持文本+tool call，再补 thinking |
| Google OAuth client 或许绑定包名/哈希（桌面 app 无此约束，Google 对 loopback 不校验哈希） | loopback 是 Google 官方支持的桌面场景 |
| IDE 与 cc-switch 同时刷新导致 token 轮换冲突 | refresh_token 通常不轮换；若轮换则采用 codex 模式（采纳最新 token + 时间戳比较） |


## 5. 实现状态（2026-08-30 更新）

全部落地于 `feat/antigravity-integration` 分支：

| 项 | 状态 | 说明 |
|---|---|---|
| OAuth（loopback 授权码流） | ✅ | `antigravity_oauth_auth.rs`：随机端口 loopback + state 校验 + 302 成功页 + 10 分钟超时；多账号 JSON 存储（原子写盘、每账号刷新锁、requires_reauth 持久化）；`auth_*` 命令族接入（复用 device-flow 前端轮询语义） |
| Transport（Cloud Code 信封） | ✅ | 复用 `transform_gemini`（Anthropic↔Gemini）+ `wrap_gemini_body_for_cloudcode`（sessionId 稳定派生 / claude-* 强制 VALIDATED / maxOutputTokens 封顶 128k）；SSE 解包 `{"response":{...}}`（`unwrap_cloudcode_chunk`）；UA 指纹 `antigravity/hub/2.9.1` |
| 模型发现 | ✅ | `get_antigravity_oauth_models`（fetchAvailableModels 空 body，map/array 双形态解析 + 内部 ID 过滤 + fallback 列表） |
| ProviderType/AuthStrategy 注册 | ✅ | `AntigravityOAuth` 全链路（mod/auth/claude/forwarder） |
| 前端 | ✅ | 类型、预设（apiFormat=gemini_native）、AntigravityOAuthSection、AuthCenterPanel、表单接线、i18n（zh/en/zh-TW/ja） |
| 测试 | ✅ | Rust：15 antigravity 单测 + 信封/重写/流式用例（全量 2723 passed，6 个失败均为环境性且与改动零关联，见下）；前端组件测试 4/4；Playwright E2E 2/2（vite dev + Tauri IPC stub） |
| 真实 OAuth E2E | ⏳ | 浏览器 consent 需账号持有者本人完成，步骤见 `antigravity-protocol.md` 同目录 `antigravity-e2e-verification.md` |

### 测试环境注记

- 本机 MSVC 不可用（VS Build Tools 静默安装被 UAC 阻断），验证使用
  `stable-x86_64-pc-windows-gnu` 工具链 + w64devkit（gcc/windres）。
  `build.rs` 相应适配：`/MANIFEST:EMBED` 仅 MSVC 注入，GNU 走 windres
  COFF 嵌入（best-effort）。
- 全量 lib 测试 6 个失败均为环境性（symlink 特权 ×2、网络/文件监听时序 ×4），
  所在文件对 antigravity 符号零引用，与本次改动无关。
- 实现中参考了本机 `E:\AI\geminintigravity-bridge`（同类工具）的
  `docs/protocol-notes.md` 多来源实测结论（信封格式、UA 指纹、VALIDATED、
  thoughtSignature 规则），并与其 OAuth 结论（client/scope/loopback）交叉一致。
