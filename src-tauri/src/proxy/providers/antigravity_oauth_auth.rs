//! Antigravity (Google Cloud Code) OAuth authentication manager.
//!
//! Antigravity IDE uses the Google OAuth 2.0 authorization-code flow with a
//! localhost loopback redirect (RFC 8252): a temporary HTTP server listens on a
//! random 127.0.0.1 port at path `/oauth-callback`, the browser completes the
//! Google consent and is redirected back to that URL, then the code is exchanged
//! at `https://oauth2.googleapis.com/token`.
//!
//! Parameters were extracted from the official IDE bundle
//! (`out-build/vs/platform/cloudCode/common/oauthClient.js` and
//! `out-build/vs/platform/antigravityAuthNew/electron-main/antigravityAuthService.js`),
//! see `docs/antigravity-protocol.md`.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, RwLock};

use super::copilot_auth::GitHubDeviceCodeResponse;

/// Default Antigravity desktop client (non GCP-TOS accounts).
pub const ANTIGRAVITY_CLIENT_ID: &str =
    "1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com";
pub const ANTIGRAVITY_CLIENT_SECRET: &str = "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";
/// Client used by the IDE when the account accepted the GCP Terms of Service.
pub const ANTIGRAVITY_GCP_TOS_CLIENT_ID: &str =
    "884354919052-36trc1jjb3tguiac32ov6cod268c5blh.apps.googleusercontent.com";
pub const ANTIGRAVITY_GCP_TOS_CLIENT_SECRET: &str = "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";

pub const GOOGLE_AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
pub const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

pub const SCOPE_CLOUD_PLATFORM: &str = "https://www.googleapis.com/auth/cloud-platform";
pub const SCOPE_USERINFO_EMAIL: &str = "https://www.googleapis.com/auth/userinfo.email";
pub const SCOPE_USERINFO_PROFILE: &str = "https://www.googleapis.com/auth/userinfo.profile";
pub const SCOPE_CCLOG: &str = "https://www.googleapis.com/auth/cclog";
pub const SCOPE_EXPERIMENTS_AND_CONFIGS: &str =
    "https://www.googleapis.com/auth/experimentsandconfigs";

pub const OAUTH_CALLBACK_PATH: &str = "/oauth-callback";
const LOGIN_SUCCESS_REDIRECT: &str = "https://antigravity.google/auth-success?app=Antigravity";
const LOGIN_TIMEOUT_MS: i64 = 10 * 60 * 1000;
const TOKEN_REFRESH_BUFFER_MS: i64 = 60_000;
const DEFAULT_TOKEN_LIFETIME_SECS: i64 = 3_600;
const MAX_OAUTH_RESPONSE_BYTES: usize = 64 * 1024;
const HTTP_READ_TIMEOUT: Duration = Duration::from_secs(30);
const HTTP_HEADER_LIMIT: usize = 32 * 1024;

#[derive(Debug, Clone, thiserror::Error)]
pub enum AntigravityOAuthError {
    #[error("等待用户授权中")]
    AuthorizationPending,
    #[error("用户取消或拒绝了授权")]
    AccessDenied,
    #[error("登录会话已过期，请重新发起登录")]
    ExpiredToken,
    #[error("OAuth Token 获取失败: {0}")]
    TokenFetchFailed(String),
    #[error("Refresh Token 失效或已过期，请重新登录 Antigravity")]
    RefreshTokenInvalid,
    #[error("账号需要重新登录: {0}")]
    ReauthRequired(String),
    #[error("网络错误: {0}")]
    NetworkError(String),
    #[error("解析错误: {0}")]
    ParseError(String),
    #[error("IO 错误: {0}")]
    IoError(String),
    #[error("账号不存在: {0}")]
    AccountNotFound(String),
}

impl From<reqwest::Error> for AntigravityOAuthError {
    fn from(err: reqwest::Error) -> Self {
        Self::NetworkError(err.to_string())
    }
}

impl From<std::io::Error> for AntigravityOAuthError {
    fn from(err: std::io::Error) -> Self {
        Self::IoError(err.to_string())
    }
}

#[derive(Debug, Clone, Deserialize)]
struct OAuthTokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    id_token: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct GoogleIdTokenClaims {
    #[serde(default)]
    sub: Option<String>,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Debug, Clone)]
struct CachedAccessToken {
    token: String,
    expires_at_ms: i64,
}

impl CachedAccessToken {
    fn is_expiring_soon(&self) -> bool {
        self.expires_at_ms - chrono::Utc::now().timestamp_millis() < TOKEN_REFRESH_BUFFER_MS
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AntigravityAccountData {
    account_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    login: Option<String>,
    refresh_token: String,
    authenticated_at: i64,
    #[serde(default)]
    requires_reauth: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AntigravityOAuthAccount {
    pub id: String,
    pub login: String,
    pub avatar_url: Option<String>,
    pub authenticated_at: i64,
    pub github_domain: String,
    pub requires_reauth: bool,
}

impl From<&AntigravityAccountData> for AntigravityOAuthAccount {
    fn from(data: &AntigravityAccountData) -> Self {
        let short_id: String = data.account_id.chars().take(12).collect();
        Self {
            id: data.account_id.clone(),
            login: data
                .login
                .clone()
                .unwrap_or_else(|| format!("Google ({short_id})")),
            avatar_url: None,
            authenticated_at: data.authenticated_at,
            github_domain: "antigravity.google".to_string(),
            requires_reauth: data.requires_reauth,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct AntigravityOAuthStore {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    accounts: HashMap<String, AntigravityAccountData>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    default_account_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AntigravityOAuthStatus {
    pub accounts: Vec<AntigravityOAuthAccount>,
    pub default_account_id: Option<String>,
    pub authenticated: bool,
    pub username: Option<String>,
}

/// Outcome recorded by the loopback server for a login session.
#[derive(Debug, Clone)]
enum LoginOutcome {
    Authorized {
        code: String,
    },
    Failed {
        error: AntigravityOAuthError,
    },
}

#[derive(Debug)]
struct PendingLogin {
    state: String,
    client_id: String,
    client_secret: String,
    redirect_uri: String,
    started_at_ms: i64,
    outcome: Arc<Mutex<Option<LoginOutcome>>>,
    /// Abort handle for the loopback server task.
    shutdown: Arc<tokio::sync::Notify>,
}

pub struct AntigravityOAuthManager {
    accounts: Arc<RwLock<HashMap<String, AntigravityAccountData>>>,
    default_account_id: Arc<RwLock<Option<String>>>,
    access_tokens: Arc<RwLock<HashMap<String, CachedAccessToken>>>,
    refresh_locks: Arc<RwLock<HashMap<String, Arc<Mutex<()>>>>>,
    pending_logins: Arc<RwLock<HashMap<String, Arc<PendingLogin>>>>,
    mutation_lock: Arc<Mutex<()>>,
    storage_path: PathBuf,
}

impl AntigravityOAuthManager {
    pub fn new(data_dir: PathBuf) -> Self {
        let manager = Self {
            accounts: Arc::new(RwLock::new(HashMap::new())),
            default_account_id: Arc::new(RwLock::new(None)),
            access_tokens: Arc::new(RwLock::new(HashMap::new())),
            refresh_locks: Arc::new(RwLock::new(HashMap::new())),
            pending_logins: Arc::new(RwLock::new(HashMap::new())),
            mutation_lock: Arc::new(Mutex::new(())),
            storage_path: data_dir.join("antigravity_oauth_auth.json"),
        };

        if let Err(error) = manager.load_from_disk_sync() {
            log::warn!("[AntigravityOAuth] 加载存储失败: {error}");
        }
        manager
    }

    /// Start a login session: bind the loopback server and build the Google
    /// consent URL. Returns the standard device-code-shaped response so the
    /// shared `useManagedAuth` polling flow can drive it unchanged.
    pub async fn start_login(&self) -> Result<GitHubDeviceCodeResponse, AntigravityOAuthError> {
        self.prune_stale_logins().await;

        let state = uuid::Uuid::new_v4().to_string();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|error| AntigravityOAuthError::IoError(error.to_string()))?;
        let port = listener
            .local_addr()
            .map_err(|error| AntigravityOAuthError::IoError(error.to_string()))?
            .port();
        let redirect_uri = format!("http://localhost:{port}{OAUTH_CALLBACK_PATH}");

        let scopes = [
            SCOPE_CLOUD_PLATFORM,
            SCOPE_USERINFO_EMAIL,
            SCOPE_USERINFO_PROFILE,
            SCOPE_CCLOG,
            SCOPE_EXPERIMENTS_AND_CONFIGS,
        ]
        .join(" ");

        let query = [
            ("client_id", ANTIGRAVITY_CLIENT_ID),
            ("redirect_uri", redirect_uri.as_str()),
            ("response_type", "code"),
            ("scope", scopes.as_str()),
            ("access_type", "offline"),
            ("prompt", "consent"),
            ("state", state.as_str()),
        ];
        let login_url = reqwest::Url::parse_with_params(GOOGLE_AUTH_URL, &query)
            .map_err(|error| AntigravityOAuthError::ParseError(error.to_string()))?
            .to_string();

        let pending = Arc::new(PendingLogin {
            state: state.clone(),
            client_id: ANTIGRAVITY_CLIENT_ID.to_string(),
            client_secret: ANTIGRAVITY_CLIENT_SECRET.to_string(),
            redirect_uri: redirect_uri.clone(),
            started_at_ms: chrono::Utc::now().timestamp_millis(),
            outcome: Arc::new(Mutex::new(None)),
            shutdown: Arc::new(tokio::sync::Notify::new()),
        });

        {
            let mut logins = self.pending_logins.write().await;
            logins.retain(|_, entry| {
                chrono::Utc::now().timestamp_millis() - entry.started_at_ms < LOGIN_TIMEOUT_MS
            });
            logins.insert(state.clone(), pending.clone());
        }

        let server_state = Arc::new(LoopbackServer {
            expected_state: state.clone(),
            outcome: pending.outcome.clone(),
            shutdown: pending.shutdown.clone(),
        });
        tokio::spawn(run_loopback_server(listener, server_state));

        log::info!(
            "[AntigravityOAuth] 登录会话已启动，loopback 回调: {redirect_uri}"
        );

        Ok(GitHubDeviceCodeResponse {
            device_code: state,
            user_code: String::new(),
            verification_uri: login_url,
            expires_in: (LOGIN_TIMEOUT_MS / 1000) as u64,
            interval: 2,
        })
    }

    /// Poll a pending login. Returns `Ok(None)` while the user has not
    /// completed consent (mirrors the device-flow semantics used by the
    /// frontend `useManagedAuth` hook).
    pub async fn poll_login(
        &self,
        session_id: &str,
    ) -> Result<Option<AntigravityOAuthAccount>, AntigravityOAuthError> {
        let pending = {
            let logins = self.pending_logins.read().await;
            logins.get(session_id).cloned()
        }
        .ok_or_else(|| {
            AntigravityOAuthError::TokenFetchFailed(
                "登录会话不存在，请重新发起登录".to_string(),
            )
        })?;

        let now_ms = chrono::Utc::now().timestamp_millis();
        if now_ms - pending.started_at_ms >= LOGIN_TIMEOUT_MS {
            self.abort_login(session_id).await;
            return Err(AntigravityOAuthError::ExpiredToken);
        }

        let outcome = pending.outcome.lock().await.clone();
        let Some(outcome) = outcome else {
            return Err(AntigravityOAuthError::AuthorizationPending);
        };

        // Login finished (one way or another): drop the session.
        self.abort_login(session_id).await;

        match outcome {
            LoginOutcome::Failed { error } => Err(error),
            LoginOutcome::Authorized { code } => {
                let tokens = self
                    .exchange_code(&code, &pending.client_id, &pending.client_secret, &pending.redirect_uri)
                    .await?;
                let refresh_token = tokens
                    .refresh_token
                    .as_deref()
                    .filter(|token| !token.trim().is_empty())
                    .map(ToString::to_string)
                    .ok_or_else(|| {
                        AntigravityOAuthError::TokenFetchFailed(
                            "授权响应缺少 refresh_token（access_type=offline 未生效）".to_string(),
                        )
                    })?;
                let (account_id, login) =
                    extract_identity_from_tokens(&tokens).ok_or_else(|| {
                        AntigravityOAuthError::ParseError(
                            "Google id_token 缺少稳定的 sub claim，未保存账号".to_string(),
                        )
                    })?;
                let cached = CachedAccessToken {
                    token: tokens.access_token,
                    expires_at_ms: compute_expires_at_ms(tokens.expires_in),
                };
                let account = self
                    .add_account_internal(account_id, login, refresh_token, Some(cached))
                    .await?;
                Ok(Some(account))
            }
        }
    }

    pub async fn abort_login(&self, session_id: &str) {
        if let Some(pending) = self.pending_logins.write().await.remove(session_id) {
            pending.shutdown.notify_one();
        }
    }

    pub async fn get_valid_token_for_account(
        &self,
        account_id: &str,
    ) -> Result<String, AntigravityOAuthError> {
        if let Some(token) = self.cached_token_for_usable_account(account_id).await {
            return Ok(token);
        }

        let refresh_lock = self.get_refresh_lock(account_id).await;
        let _refresh_guard = refresh_lock.lock().await;
        if let Some(token) = self.cached_token_for_usable_account(account_id).await {
            return Ok(token);
        }

        let account = self
            .accounts
            .read()
            .await
            .get(account_id)
            .cloned()
            .ok_or_else(|| AntigravityOAuthError::AccountNotFound(account_id.to_string()))?;
        if account.requires_reauth {
            return Err(AntigravityOAuthError::ReauthRequired(account_id.to_string()));
        }

        let tokens = match self.refresh_with_token(&account.refresh_token).await {
            Ok(tokens) => tokens,
            Err(AntigravityOAuthError::RefreshTokenInvalid) => {
                self.mark_reauth_required(account_id).await?;
                return Err(AntigravityOAuthError::ReauthRequired(account_id.to_string()));
            }
            Err(error) => return Err(error),
        };

        self.commit_refreshed_tokens(account_id, &account.refresh_token, tokens)
            .await
    }

    pub async fn get_valid_token(&self) -> Result<String, AntigravityOAuthError> {
        match self.resolve_default_account_id().await {
            Some(account_id) => self.get_valid_token_for_account(&account_id).await,
            None => Err(AntigravityOAuthError::AccountNotFound(
                "无可用的 Antigravity 账号，请登录或重新登录".to_string(),
            )),
        }
    }

    pub async fn default_account_id(&self) -> Option<String> {
        self.resolve_default_account_id().await
    }

    pub async fn get_status(&self) -> AntigravityOAuthStatus {
        let accounts = self.accounts.read().await.clone();
        let default_account_id = self.resolve_default_account_id().await;
        let account_list = Self::sorted_accounts(&accounts, default_account_id.as_deref());
        let username = default_account_id
            .as_ref()
            .and_then(|id| accounts.get(id))
            .and_then(|account| account.login.clone());
        AntigravityOAuthStatus {
            authenticated: default_account_id.is_some(),
            default_account_id,
            accounts: account_list,
            username,
        }
    }

    pub async fn list_accounts(&self) -> Vec<AntigravityOAuthAccount> {
        let accounts = self.accounts.read().await.clone();
        let default_account_id = self.resolve_default_account_id().await;
        Self::sorted_accounts(&accounts, default_account_id.as_deref())
    }

    pub async fn remove_account(&self, account_id: &str) -> Result<(), AntigravityOAuthError> {
        let _mutation_guard = self.mutation_lock.lock().await;
        let mut accounts = self.accounts.read().await.clone();
        if accounts.remove(account_id).is_none() {
            return Err(AntigravityOAuthError::AccountNotFound(account_id.to_string()));
        }
        let stored_default = self.default_account_id.read().await.clone();
        let default_account_id = if stored_default.as_deref() == Some(account_id) {
            Self::fallback_default_account_id(&accounts)
        } else {
            stored_default.filter(|id| Self::is_usable_account(&accounts, id))
        };
        self.persist_and_commit(accounts, default_account_id)
            .await?;
        self.access_tokens.write().await.remove(account_id);
        self.refresh_locks.write().await.remove(account_id);
        Ok(())
    }

    pub async fn set_default_account(&self, account_id: &str) -> Result<(), AntigravityOAuthError> {
        let _mutation_guard = self.mutation_lock.lock().await;
        let accounts = self.accounts.read().await.clone();
        let account = accounts
            .get(account_id)
            .ok_or_else(|| AntigravityOAuthError::AccountNotFound(account_id.to_string()))?;
        if account.requires_reauth {
            return Err(AntigravityOAuthError::ReauthRequired(account_id.to_string()));
        }
        self.persist_and_commit(accounts, Some(account_id.to_string()))
            .await
    }

    pub async fn clear_auth(&self) -> Result<(), AntigravityOAuthError> {
        let _mutation_guard = self.mutation_lock.lock().await;
        if self.storage_path.exists() {
            fs::remove_file(&self.storage_path)?;
        }
        *self.accounts.write().await = HashMap::new();
        *self.default_account_id.write().await = None;
        self.access_tokens.write().await.clear();
        self.refresh_locks.write().await.clear();
        for (_, pending) in self.pending_logins.write().await.drain() {
            pending.shutdown.notify_one();
        }
        Ok(())
    }

    async fn exchange_code(
        &self,
        code: &str,
        client_id: &str,
        client_secret: &str,
        redirect_uri: &str,
    ) -> Result<OAuthTokenResponse, AntigravityOAuthError> {
        let response = crate::proxy::http_client::get()
            .post(GOOGLE_TOKEN_URL)
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", redirect_uri),
                ("client_id", client_id),
                ("client_secret", client_secret),
            ])
            .send()
            .await?;
        let status = response.status();
        let value = read_json_response(response).await?;
        let error_code = oauth_error_code(&value);
        if let Some(code) = error_code.as_deref() {
            return Err(match code {
                "invalid_grant" | "invalid_request" => AntigravityOAuthError::TokenFetchFailed(
                    "授权码无效或已过期，请重新登录".to_string(),
                ),
                "access_denied" => AntigravityOAuthError::AccessDenied,
                _ => AntigravityOAuthError::TokenFetchFailed(format_oauth_error(status, &value)),
            });
        }
        if !status.is_success() {
            return Err(AntigravityOAuthError::TokenFetchFailed(format_oauth_error(
                status, &value,
            )));
        }
        parse_token_response(value)
    }

    async fn refresh_with_token(
        &self,
        refresh_token: &str,
    ) -> Result<OAuthTokenResponse, AntigravityOAuthError> {
        let response = crate::proxy::http_client::get()
            .post(GOOGLE_TOKEN_URL)
            .form(&[
                ("grant_type", "refresh_token"),
                ("client_id", ANTIGRAVITY_CLIENT_ID),
                ("client_secret", ANTIGRAVITY_CLIENT_SECRET),
                ("refresh_token", refresh_token),
            ])
            .send()
            .await?;
        let status = response.status();
        let value_result = read_json_response(response).await;
        if refresh_response_requires_reauth(status, value_result.is_err()) {
            return Err(AntigravityOAuthError::RefreshTokenInvalid);
        }
        let value = value_result?;
        let error_code = oauth_error_code(&value);
        if matches!(error_code.as_deref(), Some("invalid_grant" | "invalid_token")) {
            return Err(AntigravityOAuthError::RefreshTokenInvalid);
        }
        if !status.is_success() || error_code.is_some() {
            return Err(AntigravityOAuthError::TokenFetchFailed(format_oauth_error(
                status, &value,
            )));
        }
        let tokens = parse_token_response(value)?;
        validate_access_token(&tokens.access_token)?;
        Ok(tokens)
    }

    async fn add_account_internal(
        &self,
        account_id: String,
        login: Option<String>,
        refresh_token: String,
        cached_access_token: Option<CachedAccessToken>,
    ) -> Result<AntigravityOAuthAccount, AntigravityOAuthError> {
        let _mutation_guard = self.mutation_lock.lock().await;
        let mut accounts = self.accounts.read().await.clone();
        let data = AntigravityAccountData {
            account_id: account_id.clone(),
            login,
            refresh_token,
            authenticated_at: chrono::Utc::now().timestamp(),
            requires_reauth: false,
        };
        let account = AntigravityOAuthAccount::from(&data);
        accounts.insert(account_id.clone(), data);
        let current_default = self.default_account_id.read().await.clone();
        let default_account_id = match current_default {
            Some(id) if Self::is_usable_account(&accounts, &id) => Some(id),
            _ => Some(account_id.clone()),
        };
        self.persist_and_commit(accounts, default_account_id)
            .await?;
        if let Some(access_token) = cached_access_token {
            self.access_tokens
                .write()
                .await
                .insert(account_id, access_token);
        }
        Ok(account)
    }

    async fn commit_refreshed_tokens(
        &self,
        account_id: &str,
        expected_refresh_token: &str,
        tokens: OAuthTokenResponse,
    ) -> Result<String, AntigravityOAuthError> {
        let _mutation_guard = self.mutation_lock.lock().await;
        let mut accounts = self.accounts.read().await.clone();
        let account = accounts
            .get_mut(account_id)
            .ok_or_else(|| AntigravityOAuthError::AccountNotFound(account_id.to_string()))?;
        if account.requires_reauth {
            return Err(AntigravityOAuthError::ReauthRequired(account_id.to_string()));
        }
        if account.refresh_token != expected_refresh_token {
            return Err(AntigravityOAuthError::TokenFetchFailed(
                "账号认证状态已变化，请重试请求".to_string(),
            ));
        }

        let refresh_token_changed = tokens
            .refresh_token
            .as_deref()
            .filter(|token| !token.trim().is_empty())
            .is_some_and(|refresh_token| {
                if refresh_token == account.refresh_token {
                    false
                } else {
                    account.refresh_token = refresh_token.to_string();
                    true
                }
            });
        if refresh_token_changed {
            let default_account_id = self.default_account_id.read().await.clone();
            self.persist_and_commit(accounts, default_account_id)
                .await?;
        }

        let access_token = tokens.access_token;
        self.access_tokens.write().await.insert(
            account_id.to_string(),
            CachedAccessToken {
                token: access_token.clone(),
                expires_at_ms: compute_expires_at_ms(tokens.expires_in),
            },
        );
        Ok(access_token)
    }

    async fn mark_reauth_required(&self, account_id: &str) -> Result<(), AntigravityOAuthError> {
        let _mutation_guard = self.mutation_lock.lock().await;
        let mut accounts = self.accounts.read().await.clone();
        let account = accounts
            .get_mut(account_id)
            .ok_or_else(|| AntigravityOAuthError::AccountNotFound(account_id.to_string()))?;
        account.requires_reauth = true;
        let default_account_id = Self::fallback_default_account_id(&accounts);
        self.persist_and_commit(accounts, default_account_id)
            .await?;
        self.access_tokens.write().await.remove(account_id);
        Ok(())
    }

    async fn persist_and_commit(
        &self,
        accounts: HashMap<String, AntigravityAccountData>,
        default_account_id: Option<String>,
    ) -> Result<(), AntigravityOAuthError> {
        let store = AntigravityOAuthStore {
            version: 1,
            accounts: accounts.clone(),
            default_account_id: default_account_id.clone(),
        };
        let content = serde_json::to_string_pretty(&store)
            .map_err(|error| AntigravityOAuthError::ParseError(error.to_string()))?;
        self.write_store_atomic(&content)?;
        *self.accounts.write().await = accounts;
        *self.default_account_id.write().await = default_account_id;
        Ok(())
    }

    async fn cached_token(&self, account_id: &str) -> Option<String> {
        self.access_tokens
            .read()
            .await
            .get(account_id)
            .filter(|token| !token.is_expiring_soon())
            .map(|token| token.token.clone())
    }

    async fn cached_token_for_usable_account(&self, account_id: &str) -> Option<String> {
        let account_is_usable = {
            let accounts = self.accounts.read().await;
            Self::is_usable_account(&accounts, account_id)
        };
        if !account_is_usable {
            return None;
        }
        self.cached_token(account_id).await
    }

    async fn get_refresh_lock(&self, account_id: &str) -> Arc<Mutex<()>> {
        if let Some(lock) = self.refresh_locks.read().await.get(account_id).cloned() {
            return lock;
        }
        Arc::clone(
            self.refresh_locks
                .write()
                .await
                .entry(account_id.to_string())
                .or_insert_with(|| Arc::new(Mutex::new(()))),
        )
    }

    async fn resolve_default_account_id(&self) -> Option<String> {
        let stored = self.default_account_id.read().await.clone();
        let accounts = self.accounts.read().await;
        match stored {
            Some(id) if Self::is_usable_account(&accounts, &id) => Some(id),
            _ => Self::fallback_default_account_id(&accounts),
        }
    }

    async fn prune_stale_logins(&self) {
        let now_ms = chrono::Utc::now().timestamp_millis();
        let mut logins = self.pending_logins.write().await;
        logins.retain(|_, entry| {
            let fresh = now_ms - entry.started_at_ms < LOGIN_TIMEOUT_MS;
            if !fresh {
                entry.shutdown.notify_one();
            }
            fresh
        });
    }

    fn fallback_default_account_id(
        accounts: &HashMap<String, AntigravityAccountData>,
    ) -> Option<String> {
        accounts
            .iter()
            .filter(|(_, account)| !account.requires_reauth)
            .max_by(|(id_a, account_a), (id_b, account_b)| {
                account_a
                    .authenticated_at
                    .cmp(&account_b.authenticated_at)
                    .then_with(|| id_b.cmp(id_a))
            })
            .map(|(id, _)| id.clone())
    }

    fn is_usable_account(accounts: &HashMap<String, AntigravityAccountData>, id: &str) -> bool {
        accounts
            .get(id)
            .is_some_and(|account| !account.requires_reauth)
    }

    fn sorted_accounts(
        accounts: &HashMap<String, AntigravityAccountData>,
        default_account_id: Option<&str>,
    ) -> Vec<AntigravityOAuthAccount> {
        let mut result: Vec<_> = accounts.values().map(AntigravityOAuthAccount::from).collect();
        result.sort_by(|a, b| {
            let a_default = default_account_id == Some(a.id.as_str());
            let b_default = default_account_id == Some(b.id.as_str());
            b_default
                .cmp(&a_default)
                .then_with(|| a.requires_reauth.cmp(&b.requires_reauth))
                .then_with(|| b.authenticated_at.cmp(&a.authenticated_at))
                .then_with(|| a.login.cmp(&b.login))
        });
        result
    }

    fn load_from_disk_sync(&self) -> Result<(), AntigravityOAuthError> {
        if !self.storage_path.exists() {
            return Ok(());
        }
        let content = fs::read_to_string(&self.storage_path)?;
        let store: AntigravityOAuthStore = serde_json::from_str(&content)
            .map_err(|error| AntigravityOAuthError::ParseError(error.to_string()))?;
        if let Ok(mut accounts) = self.accounts.try_write() {
            *accounts = store.accounts;
        }
        if let Ok(mut default_account_id) = self.default_account_id.try_write() {
            *default_account_id = store.default_account_id;
        }
        Ok(())
    }

    fn write_store_atomic(&self, content: &str) -> Result<(), AntigravityOAuthError> {
        let parent = self
            .storage_path
            .parent()
            .ok_or_else(|| AntigravityOAuthError::IoError("无效的存储路径".to_string()))?;
        fs::create_dir_all(parent)?;
        let file_name = self
            .storage_path
            .file_name()
            .ok_or_else(|| AntigravityOAuthError::IoError("无效的存储文件名".to_string()))?
            .to_string_lossy();
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let temporary_path = parent.join(format!("{file_name}.tmp.{nonce}"));

        #[cfg(unix)]
        {
            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
            let result = (|| -> Result<(), std::io::Error> {
                let mut file = fs::OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .mode(0o600)
                    .open(&temporary_path)?;
                file.write_all(content.as_bytes())?;
                file.flush()?;
                fs::rename(&temporary_path, &self.storage_path)?;
                fs::set_permissions(&self.storage_path, fs::Permissions::from_mode(0o600))?;
                Ok(())
            })();
            if result.is_err() {
                let _ = fs::remove_file(&temporary_path);
            }
            result?;
        }

        #[cfg(windows)]
        {
            let result = (|| -> Result<(), std::io::Error> {
                let mut file = fs::OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(&temporary_path)?;
                file.write_all(content.as_bytes())?;
                file.flush()?;
                if self.storage_path.exists() {
                    fs::remove_file(&self.storage_path)?;
                }
                fs::rename(&temporary_path, &self.storage_path)?;
                Ok(())
            })();
            if result.is_err() {
                let _ = fs::remove_file(&temporary_path);
            }
            result?;
        }
        Ok(())
    }
}

struct LoopbackServer {
    expected_state: String,
    outcome: Arc<Mutex<Option<LoginOutcome>>>,
    shutdown: Arc<tokio::sync::Notify>,
}

/// Minimal loopback HTTP server for the OAuth redirect (mirrors the IDE:
/// OPTIONS preflight → 200, `/oauth-callback?code&state` → 302 to the
/// Antigravity success page, anything else → 404).
async fn run_loopback_server(listener: tokio::net::TcpListener, server: Arc<LoopbackServer>) {
    loop {
        let accept = tokio::select! {
            accepted = listener.accept() => accepted,
            _ = server.shutdown.notified() => break,
        };
        let Ok((stream, _)) = accept else {
            break;
        };
        let server = Arc::clone(&server);
        tokio::spawn(async move {
            let _ = handle_loopback_connection(stream, server).await;
        });
    }
}

async fn handle_loopback_connection(
    mut stream: tokio::net::TcpStream,
    server: Arc<LoopbackServer>,
) -> std::io::Result<()> {
    stream
        .readable()
        .await
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::Other, error))?;

    let mut buffer = Vec::with_capacity(4096);
    let mut chunk = [0u8; 4096];
    loop {
        match stream.try_read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buffer.extend_from_slice(&chunk[..n]);
                if buffer.windows(4).any(|window| window == b"\r\n\r\n")
                    || buffer.len() > HTTP_HEADER_LIMIT
                {
                    break;
                }
            }
            Err(ref error)
                if error.kind() == std::io::ErrorKind::WouldBlock
                    || error.kind() == std::io::ErrorKind::Interrupted =>
            {
                continue
            }
            Err(error) => return Err(error),
        }
    }

    let request = String::from_utf8_lossy(&buffer);
    let mut lines = request.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_ascii_uppercase();
    let target = parts.next().unwrap_or_default().to_string();

    let response = if method == "OPTIONS" {
        http_response(
            200,
            &[("Access-Control-Allow-Origin", "*"), ("Content-Length", "0")],
            "",
        )
    } else if method != "GET" {
        http_response(405, &[("Content-Length", "0")], "")
    } else {
        let path = target.split('?').next().unwrap_or_default();
        if path != OAUTH_CALLBACK_PATH {
            http_response(404, &[("Content-Length", "5")], "Not Found")
        } else {
            let query = target.split_once('?').map(|(_, query)| query).unwrap_or_default();
            let params: HashMap<String, String> = query
                .split('&')
                .filter_map(|pair| {
                    let (key, value) = pair.split_once('=')?;
                    Some((urldecode(key)?, urldecode(value)?))
                })
                .collect();
            let code = params.get("code").cloned().unwrap_or_default();
            let state = params.get("state").cloned().unwrap_or_default();
            if code.is_empty() || state != server.expected_state {
                let _ = server
                    .outcome
                    .lock()
                    .await
                    .insert(LoginOutcome::Failed {
                        error: AntigravityOAuthError::TokenFetchFailed(
                            "回调参数缺失或 state 校验失败".to_string(),
                        ),
                    });
                http_response(
                    400,
                    &[("Content-Type", "text/plain; charset=utf-8")],
                    "Bad Request: invalid code or state",
                )
            } else {
                let _ = server
                    .outcome
                    .lock()
                    .await
                    .insert(LoginOutcome::Authorized { code });
                http_response(
                    302,
                    &[("Location", LOGIN_SUCCESS_REDIRECT), ("Content-Length", "0")],
                    "",
                )
            }
        }
    };

    use tokio::io::AsyncWriteExt;
    stream
        .writable()
        .await
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::Other, error))?;
    stream.write_all(response.as_bytes()).await?;
    stream.flush().await?;
    Ok(())
}

fn http_response(status: u16, headers: &[(&str, &str)], body: &str) -> String {
    let reason = match status {
        200 => "OK",
        302 => "Found",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "OK",
    };
    let mut response = format!("HTTP/1.1 {status} {reason}\r\n");
    for (key, value) in headers {
        response.push_str(key);
        response.push_str(": ");
        response.push_str(value);
        response.push_str("\r\n");
    }
    response.push_str("Connection: close\r\n\r\n");
    response.push_str(body);
    response
}

fn urldecode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' => {
                if index + 2 >= bytes.len() + 1 {
                    return None;
                }
                let hex = bytes.get(index + 1..index + 3)?;
                let hex = std::str::from_utf8(hex).ok()?;
                let byte = u8::from_str_radix(hex, 16).ok()?;
                out.push(byte);
                index += 3;
            }
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

fn compute_expires_at_ms(expires_in: Option<i64>) -> i64 {
    chrono::Utc::now().timestamp_millis().saturating_add(
        expires_in
            .unwrap_or(DEFAULT_TOKEN_LIFETIME_SECS)
            .max(1)
            .saturating_mul(1_000),
    )
}

fn validate_access_token(access_token: &str) -> Result<(), AntigravityOAuthError> {
    if access_token.trim().is_empty() {
        return Err(AntigravityOAuthError::TokenFetchFailed(
            "成功响应缺少 access_token".to_string(),
        ));
    }
    Ok(())
}

fn parse_token_response(
    value: serde_json::Value,
) -> Result<OAuthTokenResponse, AntigravityOAuthError> {
    serde_json::from_value(value)
        .map_err(|_| AntigravityOAuthError::ParseError("OAuth Token 响应字段无效".to_string()))
}

fn refresh_response_requires_reauth(
    status: reqwest::StatusCode,
    response_body_is_invalid: bool,
) -> bool {
    matches!(
        status,
        reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN
    ) || (status == reqwest::StatusCode::BAD_REQUEST && response_body_is_invalid)
}

fn parse_jwt_claims(token: &str) -> Option<GoogleIdTokenClaims> {
    let payload = token.split('.').nth(1)?;
    let decoded = URL_SAFE_NO_PAD.decode(payload).ok()?;
    serde_json::from_slice(&decoded).ok()
}

fn extract_identity_from_tokens(
    tokens: &OAuthTokenResponse,
) -> Option<(String, Option<String>)> {
    let claims = tokens.id_token.as_deref().and_then(parse_jwt_claims)?;
    let account_id = claims.sub?.trim().to_string();
    if account_id.is_empty() {
        return None;
    }
    let login = claims
        .email
        .or(claims.name)
        .filter(|value| !value.trim().is_empty());
    Some((account_id, login))
}

async fn read_json_response(
    response: reqwest::Response,
) -> Result<serde_json::Value, AntigravityOAuthError> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_OAUTH_RESPONSE_BYTES as u64)
    {
        return Err(AntigravityOAuthError::ParseError(
            "OAuth 响应超过大小限制".to_string(),
        ));
    }
    let bytes = response.bytes().await?;
    if bytes.len() > MAX_OAUTH_RESPONSE_BYTES {
        return Err(AntigravityOAuthError::ParseError(
            "OAuth 响应超过大小限制".to_string(),
        ));
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| AntigravityOAuthError::ParseError("OAuth 响应不是有效 JSON".to_string()))
}

fn oauth_error_code(value: &serde_json::Value) -> Option<String> {
    value
        .get("error")
        .and_then(serde_json::Value::as_str)
        .map(sanitize_oauth_error_code)
        .filter(|value| !value.is_empty())
}

fn sanitize_oauth_error_code(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || "_.-".contains(*character))
        .take(64)
        .collect()
}

fn format_oauth_error(status: reqwest::StatusCode, value: &serde_json::Value) -> String {
    match oauth_error_code(value) {
        Some(code) => format!("HTTP {status} ({code})"),
        None => format!("HTTP {status}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unsigned_id_token(payload: &serde_json::Value) -> String {
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"none"}"#);
        let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(payload).unwrap());
        format!("{header}.{payload}.")
    }

    #[test]
    fn identity_uses_google_sub_and_email() {
        let tokens = OAuthTokenResponse {
            access_token: "ya29.test".to_string(),
            refresh_token: Some("1//refresh".to_string()),
            id_token: Some(unsigned_id_token(&serde_json::json!(
                {"sub":"1234567890","email":"dev@example.com"}
            ))),
            expires_in: Some(3_600),
        };
        assert_eq!(
            extract_identity_from_tokens(&tokens),
            Some(("1234567890".to_string(), Some("dev@example.com".to_string())))
        );
    }

    #[test]
    fn identity_requires_id_token_sub() {
        let tokens = OAuthTokenResponse {
            access_token: "ya29.test".to_string(),
            refresh_token: Some("1//refresh".to_string()),
            id_token: Some(unsigned_id_token(&serde_json::json!({"email":"a@b.c"}))),
            expires_in: Some(3_600),
        };
        assert!(extract_identity_from_tokens(&tokens).is_none());
    }

    #[test]
    fn oauth_error_never_embeds_upstream_body() {
        let value = serde_json::json!({
            "error": "invalid_grant<script>",
            "error_description": "refresh_token=super-secret"
        });
        let message = format_oauth_error(reqwest::StatusCode::BAD_REQUEST, &value);
        assert_eq!(message, "HTTP 400 Bad Request (invalid_grantscript)");
        assert!(!message.contains("super-secret"));
        assert!(!message.contains("refresh_token"));
    }

    #[test]
    fn refresh_auth_status_is_classified_before_body_parsing() {
        assert!(refresh_response_requires_reauth(
            reqwest::StatusCode::UNAUTHORIZED,
            true,
        ));
        assert!(refresh_response_requires_reauth(
            reqwest::StatusCode::FORBIDDEN,
            true,
        ));
        assert!(!refresh_response_requires_reauth(
            reqwest::StatusCode::BAD_REQUEST,
            false,
        ));
    }

    #[test]
    fn urldecode_handles_percent_and_plus() {
        assert_eq!(urldecode("a%20b+c").unwrap(), "a b c");
        assert_eq!(urldecode("%E4%B8%AD").unwrap(), "中");
        assert!(urldecode("%zz").is_none());
    }

    #[tokio::test]
    async fn account_store_round_trips_and_persists_reauth_state() {
        let data_dir = tempfile::tempdir().unwrap();
        let manager = AntigravityOAuthManager::new(data_dir.path().to_path_buf());
        manager
            .add_account_internal(
                "sub-one".to_string(),
                Some("one@example.com".to_string()),
                "1//refresh-one".to_string(),
                None,
            )
            .await
            .unwrap();
        manager
            .add_account_internal(
                "sub-two".to_string(),
                Some("two@example.com".to_string()),
                "1//refresh-two".to_string(),
                None,
            )
            .await
            .unwrap();
        manager.set_default_account("sub-one").await.unwrap();

        let reloaded = AntigravityOAuthManager::new(data_dir.path().to_path_buf());
        let status = reloaded.get_status().await;
        assert_eq!(status.accounts.len(), 2);
        assert_eq!(status.default_account_id.as_deref(), Some("sub-one"));

        reloaded.mark_reauth_required("sub-one").await.unwrap();
        let after_reauth = AntigravityOAuthManager::new(data_dir.path().to_path_buf())
            .get_status()
            .await;
        assert_eq!(
            after_reauth.default_account_id.as_deref(),
            Some("sub-two")
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(data_dir.path().join("antigravity_oauth_auth.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[tokio::test]
    async fn loopback_server_captures_code_and_redirects() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let state_value = uuid::Uuid::new_v4().to_string();
        let server = Arc::new(LoopbackServer {
            expected_state: state_value.clone(),
            outcome: Arc::new(Mutex::new(None)),
            shutdown: Arc::new(tokio::sync::Notify::new()),
        });
        tokio::spawn(run_loopback_server(listener, Arc::clone(&server)));

        let url = format!(
            "http://127.0.0.1:{port}{OAUTH_CALLBACK_PATH}?code=abc123&state={state_value}"
        );
        let client = reqwest::Client::builder()
            .no_proxy()
            // 不跟随 302：断言 loopback 的原始响应
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let response = client.get(&url).send().await.unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::FOUND);
        assert_eq!(
            response.headers().get("location").unwrap(),
            LOGIN_SUCCESS_REDIRECT
        );

        let outcome = server.outcome.lock().await.clone();
        assert!(matches!(
            outcome,
            Some(LoginOutcome::Authorized { .. })
        ));
    }

    #[tokio::test]
    async fn loopback_server_rejects_state_mismatch() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = Arc::new(LoopbackServer {
            expected_state: "expected".to_string(),
            outcome: Arc::new(Mutex::new(None)),
            shutdown: Arc::new(tokio::sync::Notify::new()),
        });
        tokio::spawn(run_loopback_server(listener, Arc::clone(&server)));

        let url = format!(
            "http://127.0.0.1:{port}{OAUTH_CALLBACK_PATH}?code=abc123&state=evil"
        );
        let client = reqwest::Client::builder()
            .no_proxy()
            // 不跟随 302：断言 loopback 的原始响应
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let response = client.get(&url).send().await.unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
        let outcome = server.outcome.lock().await.clone();
        assert!(matches!(outcome, Some(LoginOutcome::Failed { .. })));
    }

    #[tokio::test]
    async fn loopback_server_serves_options_preflight() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = Arc::new(LoopbackServer {
            expected_state: "s".to_string(),
            outcome: Arc::new(Mutex::new(None)),
            shutdown: Arc::new(tokio::sync::Notify::new()),
        });
        tokio::spawn(run_loopback_server(listener, Arc::clone(&server)));

        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let response = client
            .request(reqwest::Method::OPTIONS, format!("http://127.0.0.1:{port}{OAUTH_CALLBACK_PATH}"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        assert_eq!(response.headers().get("access-control-allow-origin").unwrap(), "*");
    }
}
