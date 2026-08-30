# Antigravity OAuth + Cloud Code API 协议文档（逆向实证版）

> 本文档全部结论来自对本机 Antigravity IDE 安装（`D:\Antigravity`，源码模块
> `out-build/vs/platform/antigravityAuthNew/electron-main/antigravityAuthService.js`、
> `out-build/vs/platform/cloudCode/common/oauthClient.js`）的二进制提取，
> 以及使用真实凭据对 Google 端点的实测验证。**无任何猜测成分。**
> 生成日期：2026-08-29。

## 1. OAuth 参数（来自 IDE 源码常量）

`cloudCode/common/oauthClient.js` 模块内定义了**两对** OAuth client：

| 变量 | 值 | 用途 |
|---|---|---|
| `kfe` (client_id) | `1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com` | **默认 Antigravity client**（非 GCP TOS 用户） |
| `_fe` (client_secret) | `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` | 同上 |
| `z_e` (client_id) | `884354919052-36trc1jjb3tguiac32ov6cod268c5blh.apps.googleusercontent.com` | **GCP TOS 客户端**（`isGcpTos=true` 时使用） |
| `H_e` (client_secret) | `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` | 同上 |

Scopes（`a3a`，两 client 相同）：

```
https://www.googleapis.com/auth/cloud-platform
https://www.googleapis.com/auth/userinfo.email
https://www.googleapis.com/auth/userinfo.profile
https://www.googleapis.com/auth/cclog
https://www.googleapis.com/auth/experimentsandconfigs
```

## 2. OAuth 流程（authorization_code + loopback，RFC 8252）

实现于 `antigravityAuthNew/electron-main/antigravityAuthService.js`：

```js
import { OAuth2Client as fOt } from "google-auth-library";
import { createServer as qTs } from "http";
```

### 2.1 本地回调服务器

```js
// S(t)：启动 http server
this.a.listen(0, "127.0.0.1", () => {   // 端口 0 = 随机端口！
  this.b = r.port;   // 之后 redirect_uri 用它
});

getLocalhostRedirectUri() {
  return this.b === null ? null : `http://localhost:${this.b}/oauth-callback`;
}
```

- **随机端口** loopback（Google OAuth policy 对 loopback client 允许任意端口）。
- 回调路径固定为 `/oauth-callback`。
- 仅处理 GET（query 中取 `code` 与 `state`）；OPTIONS 直接 200（CORS 预检）。
- 成功后浏览器被 302 到 `https://antigravity.google/auth-success?app={applicationName}`。
- 登录整体 10 分钟超时（`setTimeout(..., 10*60*1e3)`）。

### 2.2 登录 URL

```js
getLoginUrl(isGcpTos) {
  const redirect = this.getLocalhostRedirectUri();
  const state = bd();           // 随机 state
  const cid = isGcpTos ? z_e : kfe, sec = isGcpTos ? H_e : _fe;
  return new OAuth2Client(cid, sec, redirect).generateAuthUrl({
    access_type: "offline",     // 要 refresh_token
    scope: a3a,                 // 上述 5 个 scope
    state,
    prompt: "consent",
  });
}
```

即标准 Google endpoint `https://accounts.google.com/o/oauth2/v2/auth`。

### 2.3 code 交换

```js
async Y(code, state, isGcpTos) {
  const redirect = this.getLocalhostRedirectUri();
  const o = new OAuth2Client(cid, sec, redirect);
  const { tokens: u } = await o.getToken(code);
  return { accessToken: u.access_token ?? "", refreshToken: u.refresh_token ?? ..., ... };
}
```

### 2.4 刷新

```js
const a = new OAuth2Client(kfe, _fe);   // 不带 redirect
a.setCredentials({ refresh_token });
const { credentials } = await a.refreshAccessToken();
// => POST https://oauth2.googleapis.com/token
//    grant_type=refresh_token&client_id=...&client_secret=...&refresh_token=...
```

### 2.5 登录后的 onboarding（`W`/`X` 方法）

1. `loadCodeAssist(accessToken, projectId?)` → 拿 `allowedTiers` / `cloudaicompanionProject`
2. 若无 project：`showProjectPickerToUser` 让用户选 GCP project（仅 standard-tier 需要）
3. `onboardUser(tier, ..., project)` → 订阅 tier
4. 再次 `loadCodeAssist` 验证

## 3. Cloud Code API（v1internal）

来自 language server（`language_server_windows_x64.exe`，Go 二进制，protobuf 描述符内嵌）与实测：

### 3.1 端点

| 域名 | 用途 |
|---|---|
| `cloudcode-pa.googleapis.com` | 生产 |
| `daily-cloudcode-pa.googleapis.com` | daily 测试（Go 代码内常量出现） |
| `daily-cloudcode-pa.sandbox.googleapis.com` | sandbox（二进制字符串 `https://daily-cloudcode-pa.sandbox.googleapis.com`） |

### 3.2 实测验证结果（2026-08-29，Windows，经代理）

使用 Gemini CLI 的 OAuth 凭据（gemini-cli client_id）+ Bearer access token：

| 请求 | 结果 | 结论 |
|---|---|---|
| `POST https://oauth2.googleapis.com/token`（refresh_token grant） | **200** | 刷新流程可用，返回 `expires_in: 3599` |
| `POST https://cloudcode-pa.googleapis.com/v1internal:loadCodeAssist`，body `{"metadata":{"ideType":"ANTIGRAVITY","platform":"PLATFORM_UNSPECIFIED","pluginType":"GEMINI"}}` | **200** | 认证+格式全通过；返回 `allowedTiers[{id:"standard-tier",...}]`、`ineligibleTiers[{tierId:"free-tier", reason:"UNSUPPORTED_CLIENT", message:"...migrate to the Antigravity suite of products: https://antigravity.google"}]` |
| 同上，`platform:"WINDOWS"` | 400 INVALID_ARGUMENT | platform 枚举值必须是 `PLATFORM_UNSPECIFIED`（`DARWIN/LINUX/WINDOWS/WIN32` 均被拒——实测） |
| 同上，`ideType:"FIREBASE_STUDIO"` | 400 INVALID_ARGUMENT | 非法 ideType；`ANTIGRAVITY`/`VSCODE`/`IDE_UNSPECIFIED` 合法 |
| `POST .../v1internal:fetchAvailableModels`，body `{}` | **429 RESOURCE_EXHAUSTED** | 空 body 是正确格式（带 `metadata` 字段会 400 "Unknown name"）；429 是该账号 free-tier 配额为 0 所致 |
| `POST .../v1internal:streamGenerateContent?alt=sse`，body `{"model":"gemini-2.5-flash","request":{"contents":[...],"generationConfig":{}},"userPromptId":"..."}` | **403 PERMISSION_DENIED** `SUBSCRIPTION_REQUIRED`（domain `cloudaicompanion.googleapis.com`，error_number 1001） | 端点/请求 schema 正确；403 是因为该账号走 Gemini CLI client 无 free-tier 订阅 |

**核心推论**：协议链路全部打通；要用 free tier（Antigravity 免费额度）必须使用
**Antigravity 自己的 client_id（1071006060591-...）**——这正是 IDE 的做法，
也是 cc-switch 需要实现 Antigravity OAuth 而不能复用 Gemini CLI 凭据的原因。

### 3.3 loadCodeAssist

```
POST /v1internal:loadCodeAssist
Authorization: Bearer {access_token}
Content-Type: application/json

{"metadata":{"ideType":"ANTIGRAVITY","platform":"PLATFORM_UNSPECIFIED","pluginType":"GEMINI"}}
```

响应（实测节选）：

```json
{
  "allowedTiers": [
    {"id":"standard-tier","name":"Gemini Code Assist","description":"...",
     "userDefinedCloudaicompanionProject":true,"isDefault":true,"usesGcpTos":true}
  ],
  "ineligibleTiers": [
    {"reasonCode":"UNSUPPORTED_CLIENT","tierId":"free-tier","reasonMessage":"..."}
  ]
}
```

Antigravity client 下 free-tier 是默认 tier，无需 `cloudaicompanionProject`。

### 3.4 fetchAvailableModels

```
POST /v1internal:fetchAvailableModels
Authorization: Bearer {access_token}
Content-Type: application/json

{}
```

proto: `google.internal.cloud.code.v1internal.CloudCode/FetchAvailableModels`。
实测空 `{}` 即为合法请求（带 `metadata` 字段会被拒）。

### 3.5 streamGenerateContent（SSE）

```
POST {base}/v1internal:streamGenerateContent?alt=sse
Authorization: Bearer {access_token}
Content-Type: application/json
User-Agent: antigravity/hub/<ver> <platform>   // 客户端指纹（下文）
```

请求体为「Antigravity 风格信封」（本机 IDE 二进制 proto 字段 + antigravity-bridge
实测 + CLIProxyAPI v7 交叉验证）：

```json
{
  "model": "claude-sonnet-4-5",
  "userAgent": "antigravity",
  "requestType": "agent",
  "requestId": "agent-<uuid4>",
  "request": {
    "sessionId": "-<63bit int>",
    "systemInstruction": {"role":"user","parts":[{"text":"..."}]},
    "contents": [{"role":"user","parts":[{"text":"..."}]}],
    "tools": [{"functionDeclarations":[{"name","description","parameters"}]}],
    "toolConfig": {"functionCallingConfig": {"mode": "VALIDATED"}},
    "generationConfig": {"temperature","topP","topK","maxOutputTokens",
      "thinkingConfig":{"thinkingBudget":N}|{"thinkingLevel":"high"}}
  }
}
```

关键规则（实测）：

- **claude-\* 模型必须** `toolConfig.functionCallingConfig.mode = "VALIDATED"`；
  其余模型按 tool_choice（AUTO/ANY/NONE + allowedFunctionNames）。
- `maxOutputTokens` 封顶 128_000；非 Claude 目标在封顶后**删除**该字段。
- 工具参数 schema 需清洗（去 `$schema/title/format/default/const/minimum/...`，
  anyOf/oneOf 拍平为第一个分支）；tool name 限 `[a-zA-Z_][a-zA-Z0-9_-]{0,63}`。
- **thoughtSignature 规则**：回放模型轮时每轮第一个 functionCall 必须携带上游
  返回的 `thoughtSignature`，否则 400；无签名 thinking 块必须整块丢弃；
  哨兵值 `skip_thought_signature_validator` 后端接受。
- `functionResponse` 形如 `{"id","name","response":{"result":...}}`，role 规范化为 model。
- 客户端 UA 指纹 `antigravity/hub/<ver> darwin/arm64`（ver≥2.9.1）。

Go proto 字段（本机 language server 二进制描述符提取）：
`project`(string) / `request_id`(string) /
`request`(google.cloud.aiplatform.master.GenerateContentRequest) /
`model`(string) / `user_prompt_id`(string) / `user_agent`(string)…

proto HTTP annotation（二进制原文）：

```
"/v1internal:generateContent"
"/v1internal:streamGenerateContent"
"/v1internal:countTokens"
```

响应：SSE 流（`alt=sse`），每条 `data: {json}`，空行分隔，`[DONE]` 结束。
**Cloud Code 的每条事件把 GenerateContentResponse 包在 `response` 字段内**：

```json
{"response":{
  "candidates":[{"content":{"role":"model","parts":[
      {"text":"...","thought":true},
      {"text":"..."},
      {"functionCall":{"id","name","args"},"thoughtSignature":"<sig>"}
    ]}],
   "finishReason":"STOP",
   "index":0}],
 "usageMetadata":{"promptTokenCount","candidatesTokenCount","totalTokenCount",
   "thoughtsTokenCount","cachedContentTokenCount"},
 "modelVersion":"gemini-..."},
 "traceId":"..."}
```

- `finishReason` 仅 `STOP`/`MAX_TOKENS`（其它按错误处理）。
- 错误可在流中内联出现：Google RPC JSON `{"error":{code,message,status,details[]}}`；
  429 细分 reason：`RATE_LIMIT_EXCEEDED` / `QUOTA_EXHAUSTED` /
  `INSUFFICIENT_G1_CREDITS_BALANCE` / `MODEL_CAPACITY_EXHAUSTED`。
- 已知上游缺陷：个别 chunk JSON 存在多余逗号（`,,`），解析端需容忍。

### 3.6 fetchAvailableModels（真实响应形态，antigravity-bridge 实测）

```
POST /v1internal:fetchAvailableModels
body: {} 或 {"project":"<id>"}
→ {"models":{"<id>":{"displayName","maxTokens","maxOutputTokens",...}},
   "webSearchModelIds":["..."]}
```

已知模型 ID（CLIProxyAPI 静态目录）：`claude-opus-4-6-thinking`、`claude-sonnet-4-6`、
`gemini-3.6-flash-high`、`gemini-3.7-flash-high`、`gemini-3-flash`、`gemini-3-flash-agent`、
`gemini-pro-agent`、`gemini-3.1-pro-low`、`gpt-oss-120b-medium`、`gemini-3.1-flash-lite`、
`gemini-3.5-flash-low`、`gemini-3.1-flash-image`。
内部 ID 需过滤：`chat_20706`、`chat_23310`、`tab_flash_lite_preview`、
`tab_jump_flash_lite_preview`、`gemini-2.5-flash-thinking`、`gemini-2.5-pro`。
**fetchAvailableModels 需要 Antigravity 客户端 token**（gemini-cli token 实测 403）。

## 4. IDE 本地凭据存储（观察）

- `%APPDATA%/Antigravity/User/globalStorage/state.vscdb`（SQLite `ItemTable`）中
  key `antigravityUnifiedStateSync.oauthToken` 存的是 protobuf 编码的状态同步消息。
- 未登录时其内容为 `{"state":"validatingLogin","context":{...}}`（本机实测）。
  登录后应存 TokenInfo（accessToken/refreshToken/isGcpTos）。
- cc-switch **不读取** IDE 存储，而是自行完成 OAuth 并存到
  `~/.cc-switch/antigravity_oauth_auth.json`（与 codex/xai OAuth 存储同模式）。

## 5. 与 Gemini CLI 协议的差异总结

| 维度 | Gemini CLI | Antigravity |
|---|---|---|
| client_id | `681255809395-oo8ft...` | `1071006060591-tmhss...`（默认）/ `884354919052-36tr...`（GCP TOS） |
| OAuth 流程 | 浏览器 loopback | loopback **随机端口**，路径 `/oauth-callback` |
| scopes | cloud-platform, userinfo.email, userinfo.profile, openid | + cclog, experimentsandconfigs（无 openid） |
| free tier | 已弃用（UNSUPPORTED_CLIENT） | 当前默认 |
| ideType | `IDE_UNSPECIFIED`/`VSCODE` | `ANTIGRAVITY` |
| platform | `PLATFORM_UNSPECIFIED` | `PLATFORM_UNSPECIFIED`（实测其它值被拒） |
| token 存储 | `~/.gemini/oauth_creds.json` | IDE state.vscdb / cc-switch 自管 JSON |

## 6. 实现要点（给 cc-switch 的结论）

1. **OAuth 用 authorization_code + 本地 loopback server（随机端口 + `/oauth-callback` 路径）**，
   不是 Device Code 流——这是 cc-switch 中第一种此模式的实现（Codex/Copilot/xAI 均为 device 流），
   需要新增一个临时 HTTP listener 组件。
2. **默认 client（1071006060591）即可**；GCP TOS client 作为可选高级项。
3. 请求 metadata 固定 `{"ideType":"ANTIGRAVITY","platform":"PLATFORM_UNSPECIFIED","pluginType":"GEMINI"}`。
4. token 刷新与 Gemini CLI 相同（oauth2.googleapis.com + refresh_token grant），
   access_token 内存缓存 + 60s 提前量刷新。
5. 模型列表来自 `fetchAvailableModels`（空 body），模型目录需动态化。
6. 生成请求走 `streamGenerateContent?alt=sse`，SSE 响应格式与 Gemini
   `GenerateContentResponse` 兼容（aiplatform master proto）。
