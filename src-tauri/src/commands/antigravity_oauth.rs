//! Antigravity OAuth state and Antigravity-specific commands.

use crate::proxy::providers::antigravity_oauth_auth::AntigravityOAuthManager;
use crate::proxy::providers::{ANTIGRAVITY_CLOUDCODE_BASE_URL, ANTIGRAVITY_USER_AGENT};
use crate::services::model_fetch::FetchedModel;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use tauri::State;
use tokio::sync::RwLock;

pub struct AntigravityOAuthState(pub Arc<RwLock<AntigravityOAuthManager>>);

/// Cloud Code v1internal:fetchAvailableModels 的宽松解析。
///
/// 上游返回 map（models.<id>）或数组两种形态都能处理；拿不到结构时
/// 回退到 language server 二进制中确认过的模型名。
fn parse_available_models(payload: &Value) -> Vec<FetchedModel> {
    let mut models: Vec<FetchedModel> = Vec::new();

    let push_model = |models: &mut Vec<FetchedModel>, id: String, entry: Option<&Value>| {
        let id = id
            .strip_prefix("models/")
            .unwrap_or(&id)
            .trim()
            .to_string();
        if id.is_empty() || SKIPPED_MODEL_IDS.contains(&id.as_str())
            || models
                .iter()
                .any(|existing| existing.id.eq_ignore_ascii_case(&id))
        {
            return;
        }
        let display_name = entry
            .and_then(|entry| entry.get("displayName"))
            .and_then(Value::as_str)
            .map(ToString::to_string);
        models.push(FetchedModel {
            id,
            owned_by: display_name.or_else(|| Some("antigravity".to_string())),
        });
    };

    match payload.get("models") {
        Some(Value::Object(entries)) => {
            for (id, entry) in entries {
                push_model(&mut models, id.clone(), Some(entry));
            }
        }
        Some(Value::Array(items)) => {
            for item in items {
                let id = item
                    .get("name")
                    .or_else(|| item.get("id"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                push_model(&mut models, id, Some(item));
            }
        }
        _ => {}
    }

    if models.is_empty() {
        for id in FALLBACK_ANTIGRAVITY_MODELS {
            push_model(&mut models, (*id).to_string(), None);
        }
    }

    models.sort_by(|a, b| a.id.cmp(&b.id));
    models
}

/// 上游内部/不可用 ID，不应对用户展示（协议文档 §3.6）
const SKIPPED_MODEL_IDS: &[&str] = &[
    "chat_20706",
    "chat_23310",
    "tab_flash_lite_preview",
    "tab_jump_flash_lite_preview",
    "gemini-2.5-flash-thinking",
];

const FALLBACK_ANTIGRAVITY_MODELS: &[&str] = &[
    "gemini-2.5-flash",
    "gemini-2.5-pro",
    "gemini-3.1-flash-image-preview",
    "claude-sonnet-4-5",
];

/// 列出 Antigravity (Cloud Code) 可用模型
#[tauri::command(rename_all = "camelCase")]
pub async fn get_antigravity_oauth_models(
    account_id: Option<String>,
    state: State<'_, AntigravityOAuthState>,
) -> Result<Vec<FetchedModel>, String> {
    let manager = state.0.read().await;
    let resolved = match account_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
    {
        Some(id) => Some(id.to_string()),
        None => manager.default_account_id().await,
    };
    let account_id = resolved.ok_or_else(|| "No usable Antigravity account available".to_string())?;
    let token = manager
        .get_valid_token_for_account(&account_id)
        .await
        .map_err(|error| format!("Antigravity OAuth token unavailable: {error}"))?;

    let response = crate::proxy::http_client::get()
        .post(format!("{ANTIGRAVITY_CLOUDCODE_BASE_URL}/v1internal:fetchAvailableModels"))
        .bearer_auth(token)
        .header("User-Agent", ANTIGRAVITY_USER_AGENT)
        .json(&serde_json::json!({}))
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|error| format!("Antigravity models request failed: {error}"))?;
    let status = response.status();
    let payload: Value = response
        .json()
        .await
        .map_err(|_| "Antigravity models response was not valid JSON".to_string())?;
    if !status.is_success() {
        // 429/403 等业务错误直接透出，便于前端提示配额/订阅状态
        let message = payload
            .get("error")
            .and_then(|error| error.get("message"))
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        return Err(format!(
            "Antigravity models request failed: HTTP {status} ({message})"
        ));
    }
    Ok(parse_available_models(&payload))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_models_map() {
        let payload = serde_json::json!({
            "models": {
                "gemini-2.5-pro": {"displayName": "Gemini 2.5 Pro"},
                "models/claude-sonnet-4-5": {"displayName": "Claude Sonnet 4.5"}
            }
        });
        let models = parse_available_models(&payload);
        let ids: Vec<&str> = models.iter().map(|model| model.id.as_str()).collect();
        assert_eq!(ids, vec!["claude-sonnet-4-5", "gemini-2.5-pro"]);
    }

    #[test]
    fn parses_models_array() {
        let payload = serde_json::json!({
            "models": [
                {"name": "models/gemini-2.5-flash"},
                {"id": "gemini-2.5-pro"}
            ]
        });
        let models = parse_available_models(&payload);
        let ids: Vec<&str> = models.iter().map(|model| model.id.as_str()).collect();
        assert_eq!(ids, vec!["gemini-2.5-flash", "gemini-2.5-pro"]);
    }

    #[test]
    fn falls_back_when_payload_has_no_models() {
        let models = parse_available_models(&serde_json::json!({}));
        assert!(!models.is_empty());
        assert!(models.iter().all(|model| model.id.starts_with("gemini-")
            || model.id.starts_with("claude-")));
    }
}
