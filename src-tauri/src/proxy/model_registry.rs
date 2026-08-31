//! Universal Gateway：请求级 provider 路由（Phase 5）。
//!
//! 目标：客户端不再依赖 GUI 的 "current provider" 全局状态——
//! 通过 model 字段的 `provider/model` 前缀、别名表或模型注册表，
//! 网关在每次请求实时决定上游 provider，并按客户端协议合成
//! 归一化的 Provider 形状喂给现有转换管线。
//!
//! 解析优先级（需求 §2）：
//!   1. 显式 provider 前缀（`antigravity/gemini-3-flash`、`deepseek:deepseek-chat`）
//!   2. 别名（provider 候选名 + meta.routingAliases + 内置映射）
//!   3. 模型注册表精确命中（bare model 在所有 provider 的目录中唯一/同 app 优先）
//!   4. 未命中 → 保持旧行为（GUI current provider）
//!
//! Session 亲和（需求 §5）：解析命中时记录 `session → label/model`；
//! 裸模型歧义命中时优先选择会话此前绑定的 provider，避免对话中途漂移。

use crate::database::Database;
use crate::provider::{Provider, ProviderMeta};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

/// 归一化后的可路由上游模型
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteModel {
    pub id: String,
    pub display_name: Option<String>,
}

/// 注册表中的一个 provider 条目（跨 app 归一化）
#[derive(Debug, Clone)]
pub struct ProviderRoute {
    pub provider_id: String,
    pub provider_name: String,
    /// 所属 app（合成时决定用哪个客户端管线读取）
    pub app_type: String,
    pub base_url: String,
    pub api_key: String,
    /// anthropic | openai_chat | openai_responses | gemini_native
    pub wire: String,
    /// 托管 OAuth 类型（antigravity_oauth / codex_oauth / xai_oauth / github_copilot）
    pub managed: Option<String>,
    /// 托管账号绑定（provider meta.authBinding 原样透传）
    pub auth_binding: Option<serde_json::Value>,
    pub models: Vec<RouteModel>,
    /// 全部可匹配 label（小写）：providerType 规范名、名称首词、routingAliases
    pub labels: Vec<String>,
}

/// 一次路由决议
#[derive(Debug, Clone)]
pub struct RouteDecision {
    pub route: ProviderRoute,
    /// 发往上游的裸模型名
    pub upstream_model: String,
    /// 客户端请求的 label（亲和记录用）
    pub matched_label: Option<String>,
}

/// 内置 providerType → 规范 label 映射
fn canonical_label(provider_type: &str) -> Option<&'static str> {
    match provider_type {
        "antigravity_oauth" => Some("antigravity"),
        "codex_oauth" => Some("chatgpt"),
        "xai_oauth" => Some("xai"),
        "github_copilot" => Some("copilot"),
        _ => None,
    }
}

/// 供应商名 → 可匹配 label 集合：ASCII 词 + meta.routingAliases + providerType 规范名
pub fn provider_labels(name: &str, provider_type: Option<&str>, aliases: &[String]) -> Vec<String> {
    let mut labels: Vec<String> = Vec::new();
    let mut push = |value: String| {
        let value = value.to_lowercase();
        if value.len() >= 2 && !labels.contains(&value) {
            labels.push(value);
        }
    };
    // 名称按非字母数字切词（保留中文连续段整体作为一个候选）
    for word in name
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
    {
        if word.chars().all(|c| c.is_ascii_alphanumeric()) {
            push(word.to_string());
        }
    }
    for alias in aliases {
        push(alias.clone());
    }
    if let Some(t) = provider_type.and_then(canonical_label) {
        push(t.to_string());
    }
    labels
}

/// 从 DB 构建注册表。
///
/// 每个 app 的 settings_config 形态不同，这里按 app 归一化提取
/// base_url / api_key / wire / models；无法归一化的（opencode/openclaw
/// 等复杂形态）跳过注册。
pub fn build_registry(db: &Database) -> Vec<ProviderRoute> {
    let mut routes = Vec::new();
    for app in ["claude", "claude-desktop", "codex", "gemini", "hermes"] {
        let Ok(providers) = db.get_all_providers(app) else {
            continue;
        };
        for (_, provider) in providers {
            if let Some(route) = normalize_provider(app, &provider) {
                routes.push(route);
            }
        }
    }
    routes
}

fn normalize_provider(app: &str, provider: &Provider) -> Option<ProviderRoute> {
    let meta = provider.meta.clone().unwrap_or_default();
    let managed = meta.provider_type.clone();
    let wire_default = match app {
        "gemini" => "gemini_native",
        "codex" => "openai_responses",
        _ => "anthropic",
    };
    let wire = meta
        .api_format
        .clone()
        .unwrap_or_else(|| wire_default.to_string());
    let auth_binding = serde_json::to_value(&meta.auth_binding).ok().filter(|v| !v.is_null());
    let settings = &provider.settings_config;

    let (base_url, api_key, models) = match app {
        "claude" | "claude-desktop" => {
            let env = settings.get("env")?;
            let base_url = env
                .get("ANTHROPIC_BASE_URL")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let api_key = env
                .get("ANTHROPIC_AUTH_TOKEN")
                .or_else(|| env.get("ANTHROPIC_API_KEY"))
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let mut models: Vec<RouteModel> = Vec::new();
            let mut collect = |value: Option<&String>| {
                if let Some(model) = value.map(|s| s.trim()).filter(|s| !s.is_empty()) {
                    let id = strip_one_m(model).to_string();
                    if !models.iter().any(|m| m.id == id) {
                        models.push(RouteModel {
                            id,
                            display_name: None,
                        });
                    }
                }
            };
            // ANTHROPIC_MODEL / *_MODEL 直接值；*_MODEL_NAME 是展示名，作为别名收录
            for (key, value) in env.as_object().into_iter().flat_map(|m| m.iter()) {
                let Some(s) = value.as_str() else { continue };
                if key == "ANTHROPIC_MODEL" || key.ends_with("_MODEL") {
                    collect(Some(&s.to_string()));
                }
            }
            // cd 的模型路由映射（HashMap<role, ClaudeDesktopModelRoute>）
            for entry in meta.claude_desktop_model_routes.values() {
                collect(Some(&entry.model));
            }
            (base_url, api_key, models)
        }
        "codex" => {
            let config = settings.get("config").and_then(|v| v.as_str()).unwrap_or("");
            let base_url = extract_toml_value(config, "base_url").unwrap_or_default();
            let api_key = settings
                .get("auth")
                .and_then(|auth| auth.get("OPENAI_API_KEY"))
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let mut models = Vec::new();
            if let Some(model) = extract_toml_value(config, "model") {
                models.push(RouteModel {
                    id: model,
                    display_name: None,
                });
            }
            if let Some(catalog) = provider.settings_config.get("modelCatalog").and_then(|v| v.as_array()) {
                for entry in catalog {
                    if let Some(model) = entry.get("model").and_then(|v| v.as_str()) {
                        let id = strip_one_m(model).to_string();
                        if !models.iter().any(|m| m.id == id) {
                            models.push(RouteModel {
                                id,
                                display_name: entry
                                    .get("displayName")
                                    .and_then(|v| v.as_str())
                                    .map(ToString::to_string),
                            });
                        }
                    }
                }
            }
            (base_url, api_key, models)
        }
        "gemini" => {
            let env = settings.get("env").cloned().unwrap_or_default();
            let base_url = env
                .get("GOOGLE_GEMINI_BASE_URL")
                .or_else(|| settings.get("base_url"))
                .and_then(|v| v.as_str())
                .unwrap_or("https://generativelanguage.googleapis.com")
                .to_string();
            let api_key = env
                .get("GEMINI_API_KEY")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            // Gemini 官方模型众多，不在 provider 里枚举；仅支持 label/model 显式前缀。
            (base_url, api_key, Vec::new())
        }
        "hermes" => {
            let base_url = settings
                .get("base_url")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let api_key = settings
                .get("api_key")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let wire = match settings.get("api_mode").and_then(|v| v.as_str()) {
                Some("anthropic_messages") => "anthropic",
                Some("codex_responses") => "openai_responses",
                Some("bedrock_converse") => return None,
                _ => "openai_chat",
            }
            .to_string();
            let models = settings
                .get("models")
                .and_then(|v| v.as_array())
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| {
                            let id = item.get("id").and_then(|v| v.as_str())?.to_string();
                            Some(RouteModel {
                                id,
                                display_name: item
                                    .get("name")
                                    .and_then(|v| v.as_str())
                                    .map(ToString::to_string),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            (base_url, api_key, models)
        }
        _ => return None,
    };

    if base_url.is_empty() && managed.is_none() {
        return None;
    }

    let labels = provider_labels(
        &provider.name,
        meta.provider_type.as_deref(),
        &meta.routing_aliases.clone().unwrap_or_default(),
    );
    if labels.is_empty() {
        return None;
    }

    Some(ProviderRoute {
        provider_id: provider.id.clone(),
        provider_name: provider.name.clone(),
        app_type: app.to_string(),
        base_url,
        api_key,
        wire,
        managed,
        auth_binding,
        models,
        labels,
    })
}

/// base_url 是否以版本段结尾（/v1、/v4…）
fn ends_with_version(base_url: &str) -> bool {
    base_url
        .trim_end_matches('/')
        .rsplit_once('/')
        .is_some_and(|(_, last)| {
            last.len() >= 2
                && last.as_bytes()[0] == b'v'
                && last[1..].bytes().all(|b| b.is_ascii_digit())
        })
}

fn strip_one_m(model: &str) -> &str {
    model
        .strip_suffix("[1m]")
        .or_else(|| model.strip_suffix("[1M]"))
        .unwrap_or(model)
}

/// 宽松提取 TOML 里第一个 `key = "value"`（跳过注释行）。
fn extract_toml_value(config: &str, key: &str) -> Option<String> {
    for line in config.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        if k.trim() == key {
            let v = v.trim().trim_matches('"').to_string();
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    None
}

/// 解析 `label/xxx` 或 `label:xxx` 前缀（第一个 `/`/`:` 之前若含字母则是 label）。
pub fn split_provider_prefix(model: &str) -> Option<(&str, &str)> {
    let bytes = model.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'/' || *byte == b':' {
            let (head, tail) = (&model[..index], &model[index + 1..]);
            let head_is_word = !head.is_empty()
                && head
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                // URL 形态（`https://…`）不是 provider 前缀
                && !matches!(head.to_lowercase().as_str(), "http" | "https");
            if head_is_word && !tail.is_empty() {
                return Some((head, tail));
            }
            return None;
        }
    }
    None
}

/// 该 provider 的 wire 是否已被目标客户端协议管线覆盖。
/// Codex（Responses 客户端）没有直连通用 Gemini-native 上游的转换链
/// （antigravity 经 managed 特判走 Responses→Anthropic→CloudCode 组合链），故排除。
fn route_serves_client(route: &ProviderRoute, client_app: &str) -> bool {
    if client_app != "codex" {
        return true;
    }
    let is_managed_antigravity = route.managed.as_deref() == Some("antigravity_oauth");
    route.wire != "gemini_native" || is_managed_antigravity
}

/// 请求级解析（需求 §2 优先级）。
///
/// `affinity_label`：该会话此前绑定的 provider label（歧义消解用）。
pub fn resolve(
    registry: &[ProviderRoute],
    client_app: &str,
    model: &str,
    affinity_label: Option<&str>,
) -> Option<RouteDecision> {
    if model.eq_ignore_ascii_case("unknown") || model.is_empty() {
        return None;
    }

    // 1/2. 显式 provider 前缀（含别名）
    if let Some((label, rest)) = split_provider_prefix(model) {
        let matches: Vec<&ProviderRoute> = registry
            .iter()
            .filter(|route| route_serves_client(route, client_app))
            .filter(|route| route.labels.iter().any(|l| l == &label.to_lowercase()))
            .collect();
        let affinity_lower = affinity_label.map(|label| label.to_lowercase());
        let chosen = match matches.len() {
            0 => return None,
            1 => matches[0],
            _ => matches
                .iter()
                .find(|r| {
                    affinity_lower
                        .as_deref()
                        .is_some_and(|label| r.labels.iter().any(|l| l == label))
                })
                .or_else(|| matches.iter().find(|r| r.app_type == client_app))
                .or_else(|| matches.first())?,
        };
        return Some(RouteDecision {
            route: chosen.clone(),
            upstream_model: rest.to_string(),
            matched_label: Some(label.to_lowercase()),
        });
    }

    // 3. 注册表 bare model 命中：唯一 > 亲和 > 同 app > 第一个
    let needle = model.to_lowercase();
    let matches: Vec<&ProviderRoute> = registry
        .iter()
        .filter(|route| route_serves_client(route, client_app))
        .filter(|route| {
            route.models.iter().any(|entry| {
                strip_one_m(&entry.id).eq_ignore_ascii_case(&needle)
                    || entry
                        .display_name
                        .as_deref()
                        .is_some_and(|d| d.eq_ignore_ascii_case(model))
            })
        })
        .collect();
    let affinity_lower = affinity_label.map(|label| label.to_lowercase());
    let chosen = match matches.len() {
        0 => return None,
        1 => matches[0],
        _ => matches
            .iter()
            .find(|r| {
                affinity_lower
                    .as_deref()
                    .is_some_and(|label| r.labels.iter().any(|l| l == label))
            })
            .or_else(|| matches.iter().find(|r| r.app_type == client_app))
            .or_else(|| matches.first())?,
    };
    Some(RouteDecision {
        route: chosen.clone(),
        upstream_model: strip_one_m(model).to_string(),
        matched_label: chosen.labels.first().cloned(),
    })
}

/// 按客户端 app 协议合成 Provider（喂给现有管线）。
///
/// 保留原 provider 的语义 meta（providerType/authBinding/apiFormat 由
/// 归一化结果决定），settings_config 合成为目标客户端管线的形状。
pub fn synthesize_provider(client_app: &str, decision: &RouteDecision) -> Provider {
    let route = &decision.route;
    // openai 线协议 + claude 客户端：base_url 版本段各不相同（/v4、无版本、/v1），
    // 统一合成为完整 endpoint URL（meta.is_full_url），避免 /v4/v1/… 双拼。
    let (base_url, full_url) = match (client_app, route.wire.as_str()) {
        ("claude" | "claude-desktop", "openai_chat") => {
            let tail = if ends_with_version(&route.base_url) {
                "/chat/completions"
            } else {
                "/v1/chat/completions"
            };
            (
                format!("{}{}", route.base_url.trim_end_matches('/'), tail),
                true,
            )
        }
        ("claude" | "claude-desktop", "openai_responses") => {
            let tail = if ends_with_version(&route.base_url) {
                "/responses"
            } else {
                "/v1/responses"
            };
            (
                format!("{}{}", route.base_url.trim_end_matches('/'), tail),
                true,
            )
        }
        _ => (route.base_url.clone(), false),
    };
    let settings_config = match client_app {
        "claude" | "claude-desktop" => serde_json::json!({
            "env": {
                "ANTHROPIC_BASE_URL": base_url,
                "ANTHROPIC_AUTH_TOKEN": if route.api_key.is_empty() { "PROXY_MANAGED" } else { &route.api_key },
            }
        }),
        "codex" => {
            let toml = format!(
                "model_provider = \"custom\"\nmodel = \"{}\"\n\n[model_providers.custom]\nname = \"universal\"\nbase_url = \"{}\"\nwire_api = \"responses\"",
                decision.upstream_model, route.base_url
            );
            serde_json::json!({
                "auth": { "OPENAI_API_KEY": if route.api_key.is_empty() { "PROXY_MANAGED" } else { &route.api_key } },
                "config": toml,
            })
        }
        "gemini" => serde_json::json!({
            "env": {
                "GOOGLE_GEMINI_BASE_URL": route.base_url,
                "GEMINI_API_KEY": route.api_key,
            }
        }),
        "hermes" => serde_json::json!({
            "name": format!("universal-{}", route.provider_id),
            "base_url": route.base_url,
            "api_key": route.api_key,
            "api_mode": match route.wire.as_str() {
                "anthropic" => "anthropic_messages",
                "openai_responses" => "codex_responses",
                _ => "chat_completions",
            },
        }),
        _ => serde_json::json!({}),
    };

    let wire = match (client_app, route.managed.as_deref()) {
        // Codex 客户端 + Cloud Code：走 Responses→Anthropic→CloudCode 组合链
        // （forwarder 的 is_codex_antigravity 分支要求 apiFormat=anthropic）。
        ("codex", Some("antigravity_oauth")) => "anthropic",
        ("gemini", Some("antigravity_oauth")) => "gemini_native",
        (_, Some("antigravity_oauth")) => "gemini_native",
        (_, Some("codex_oauth")) | (_, Some("xai_oauth")) => "openai_responses",
        (_, Some("github_copilot")) => "openai_chat",
        _ => route.wire.as_str(),
    };

    let mut meta: ProviderMeta = ProviderMeta {
        api_format: Some(wire.to_string()),
        provider_type: route.managed.clone(),
        is_full_url: full_url.then_some(true),
        ..Default::default()
    };
    if let Some(binding) = route.auth_binding.clone() {
        if let Ok(binding) = serde_json::from_value(binding) {
            meta.auth_binding = Some(binding);
        }
    }

    Provider {
        id: format!("universal:{}", route.provider_id),
        name: route.provider_name.clone(),
        settings_config,
        website_url: None,
        category: Some("third_party".to_string()),
        created_at: None,
        sort_index: None,
        notes: None,
        icon: None,
        icon_color: None,
        meta: Some(meta),
        in_failover_queue: false,
    }
}

/// 带 TTL 的注册表缓存（provider CRUD 低频，5 秒足够新鲜；改配置最迟 5s 生效）。
static REGISTRY_CACHE: OnceLock<Mutex<Option<(std::time::Instant, Arc<Vec<ProviderRoute>>)>>> =
    OnceLock::new();

pub fn cached_registry(db: &Database) -> Arc<Vec<ProviderRoute>> {
    let cache = REGISTRY_CACHE.get_or_init(|| Mutex::new(None));
    if let Ok(guard) = cache.lock() {
        if let Some((built_at, registry)) = guard.as_ref() {
            if built_at.elapsed() < std::time::Duration::from_secs(5) {
                return Arc::clone(registry);
            }
        }
    }
    let registry = Arc::new(build_registry(db));
    if let Ok(mut guard) = cache.lock() {
        *guard = Some((std::time::Instant::now(), Arc::clone(&registry)));
    }
    registry
}

/// 亲和表上限（简单 FIFO 淘汰，防止无界增长）
pub const AFFINITY_CAPACITY: usize = 4096;

pub fn affinity_label(
    affinity: &tokio::sync::RwLock<HashMap<String, (String, String)>>,
    session_id: &str,
) -> Option<String> {
    let guard = affinity.try_read().ok()?;
    guard.get(session_id).map(|(label, _)| label.clone())
}

pub fn record_affinity(
    affinity: &tokio::sync::RwLock<HashMap<String, (String, String)>>,
    session_id: &str,
    label: &str,
    model: &str,
) {
    let Ok(mut guard) = affinity.try_write() else {
        return;
    };
    if guard.len() >= AFFINITY_CAPACITY && !guard.contains_key(session_id) {
        // FIFO 粗粒度淘汰；只用于歧义消解，丢最旧无碍正确性
        let victim = guard
            .keys()
            .next()
            .cloned();
        if let Some(victim) = victim {
            guard.remove(&victim);
        }
    }
    guard.insert(
        session_id.to_string(),
        (label.to_string(), model.to_string()),
    );
}

/// 供 GUI 展示的注册表快照
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistryEntry {
    pub provider_id: String,
    pub provider_name: String,
    pub labels: Vec<String>,
    pub app_type: String,
    pub wire: String,
    pub managed: Option<String>,
    pub models: Vec<RouteModel>,
    pub is_current_app_default: bool,
}

pub fn registry_snapshot(db: &Database) -> Vec<RegistryEntry> {
    build_registry(db)
        .into_iter()
        .map(|route| RegistryEntry {
            provider_id: route.provider_id,
            provider_name: route.provider_name,
            labels: route.labels,
            app_type: route.app_type,
            wire: route.wire,
            managed: route.managed,
            models: route.models,
            is_current_app_default: false,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_from_name_and_provider_type() {
        let labels = provider_labels(
            "Antigravity (Google)",
            Some("antigravity_oauth"),
            &["ag".to_string()],
        );
        assert!(labels.contains(&"antigravity".to_string()));
        assert!(labels.contains(&"google".to_string()));
        assert!(labels.contains(&"ag".to_string()));
    }

    #[test]
    fn prefix_split_basic() {
        assert_eq!(split_provider_prefix("antigravity/gemini-3-flash"), Some(("antigravity", "gemini-3-flash")));
        assert_eq!(split_provider_prefix("deepseek:deepseek-chat"), Some(("deepseek", "deepseek-chat")));
        assert_eq!(split_provider_prefix("gemini-2.5-pro"), None);
        assert_eq!(split_provider_prefix("gpt/5"), Some(("gpt", "5")));
        assert_eq!(split_provider_prefix("https://x/y"), None);
    }

    fn route(id: &str, app: &str, labels: &[&str], models: &[&str]) -> ProviderRoute {
        ProviderRoute {
            provider_id: id.into(),
            provider_name: id.into(),
            app_type: app.into(),
            base_url: format!("https://{id}.test"),
            api_key: "sk-test".into(),
            wire: "anthropic".into(),
            managed: None,
            auth_binding: None,
            models: models
                .iter()
                .map(|m| RouteModel {
                    id: m.to_string(),
                    display_name: None,
                })
                .collect(),
            labels: labels.iter().map(|l| l.to_string()).collect(),
        }
    }

    #[test]
    fn resolve_prefix_and_bare() {
        let registry = vec![
            route("antigravity", "claude", &["antigravity", "ag"], &["gemini-3-flash"]),
            route("deepseek", "claude", &["deepseek"], &["deepseek-chat"]),
            route("kimi", "codex", &["kimi"], &["kimi-k3"]),
        ];
        let hit = resolve(&registry, "claude", "ag/gemini-3-flash", None).unwrap();
        assert_eq!(hit.route.provider_id, "antigravity");
        assert_eq!(hit.upstream_model, "gemini-3-flash");

        let hit = resolve(&registry, "claude", "kimi-k3", None).unwrap();
        assert_eq!(hit.route.provider_id, "kimi");

        let miss = resolve(&registry, "claude", "gpt-5.6", None);
        assert!(miss.is_none());
    }

    #[test]
    fn resolve_prefers_same_app_then_affinity() {
        let registry = vec![
            route("ds-codex", "codex", &["dsx"], &["deepseek-chat"]),
            route("ds-claude", "claude", &["dsc"], &["deepseek-chat"]),
        ];
        // 歧义：codex 客户端优先同 app
        let hit = resolve(&registry, "codex", "deepseek-chat", None).unwrap();
        assert_eq!(hit.route.provider_id, "ds-codex");
        // 亲和优先于 app
        let hit = resolve(&registry, "codex", "deepseek-chat", Some("dsc")).unwrap();
        assert_eq!(hit.route.provider_id, "ds-claude");
    }

    #[test]
    fn synthesize_claude_shape() {
        let decision = RouteDecision {
            route: route("ag", "codex", &["antigravity"], &[]),
            upstream_model: "gemini-3-flash".into(),
            matched_label: Some("antigravity".into()),
        };
        let provider = synthesize_provider("claude", &decision);
        assert_eq!(
            provider
                .settings_config
                .pointer("/env/ANTHROPIC_BASE_URL")
                .unwrap()
                .as_str()
                .unwrap(),
            "https://ag.test"
        );
        assert_eq!(provider.meta.unwrap().api_format.unwrap(), "anthropic");
    }

    #[test]
    fn synthesize_codex_shape_carries_model_in_toml() {
        let decision = RouteDecision {
            route: route("ds", "claude", &["deepseek"], &[]),
            upstream_model: "deepseek-chat".into(),
            matched_label: None,
        };
        let provider = synthesize_provider("codex", &decision);
        let config = provider.settings_config.get("config").unwrap().as_str().unwrap();
        assert!(config.contains("model = \"deepseek-chat\""));
        assert!(config.contains("base_url = \"https://ds.test\""));
    }
}
