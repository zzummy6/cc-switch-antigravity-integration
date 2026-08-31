//! Universal Gateway 控制面命令（Phase 5）。

use crate::proxy::model_registry::{self, RegistryEntry};
use crate::store::AppState;
use serde::Serialize;
use std::collections::HashMap;
use tauri::State;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UniversalStatus {
    /// 统一网关监听地址（未运行时为 None）
    pub gateway: Option<String>,
    pub running: bool,
    /// 请求级路由表条目
    pub routes: Vec<RegistryEntry>,
    /// 活跃会话亲和：session → label/model
    pub affinity: HashMap<String, (String, String)>,
    /// 各 app 的 GUI current provider（路由未命中时的兜底）
    pub app_defaults: HashMap<String, String>,
}

/// 统一网关状态：路由表 + 亲和 + 兜底
#[tauri::command(rename_all = "camelCase")]
pub async fn universal_status(
    app_state: State<'_, AppState>,
) -> Result<UniversalStatus, String> {
    let db = app_state.db.clone();
    let registry_db = app_state.db.clone();
    let routes = tokio::task::spawn_blocking(move || {
        model_registry::registry_snapshot(&registry_db)
    })
    .await
    .map_err(|e| e.to_string())?;

    let affinity = app_state
        .proxy_service
        .universal_affinity_snapshot()
        .await;

    let mut app_defaults = HashMap::new();
    for app in ["claude", "claude-desktop", "codex", "gemini", "hermes", "opencode", "openclaw"] {
        if let Ok(Some(id)) = db.get_current_provider(app) {
            if let Ok(Some(provider)) = db.get_provider_by_id(&id, app) {
                app_defaults.insert(app.to_string(), provider.name);
            }
        }
    }

    let proxy_status = app_state.proxy_service.get_status().await.unwrap_or_default();
    Ok(UniversalStatus {
        gateway: proxy_status
            .running
            .then(|| format!("{}:{}", proxy_status.address, proxy_status.port)),
        running: proxy_status.running,
        routes,
        affinity,
        app_defaults,
    })
}

/// 设置 provider 的路由别名（追加在名称词与 providerType 规范名之外）。
#[tauri::command(rename_all = "camelCase")]
pub async fn universal_set_route_alias(
    app_state: State<'_, AppState>,
    provider_id: String,
    app_type: String,
    aliases: Vec<String>,
) -> Result<(), String> {
    let mut provider = app_state
        .db
        .get_provider_by_id(&provider_id, &app_type)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "provider not found".to_string())?;
    let mut meta = provider.meta.take().unwrap_or_default();
    let cleaned: Vec<String> = aliases
        .iter()
        .map(|a| a.trim().to_lowercase())
        .filter(|a| (a.len() >= 2) && a.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'))
        .collect();
    meta.routing_aliases = if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    };
    provider.meta = Some(meta);
    app_state
        .db
        .save_provider(&app_type, &provider)
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// 清除会话亲和（传 session 清单条；不传清空全部）
#[tauri::command(rename_all = "camelCase")]
pub async fn universal_clear_affinity(
    app_state: State<'_, AppState>,
    session_id: Option<String>,
) -> Result<(), String> {
    app_state
        .proxy_service
        .clear_universal_affinity(session_id.as_deref())
        .await;
    Ok(())
}
