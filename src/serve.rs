use std::sync::Arc;

use anyhow::{Context, Result};
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{header, Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};

use crate::model::{new_memory, Memory};
use crate::params::{get_f32, get_string, get_tags, get_usize};
use crate::search;
use crate::store::Store;

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<Store>,
    pub token: Option<String>,
}

pub async fn run(host: String, port: u16, token: Option<String>) -> Result<()> {
    let store = Store::open()?;
    let state = AppState {
        store: Arc::new(store),
        token,
    };
    let app = Router::new()
        .route("/health", get(health))
        .route("/mem", post(add_mem))
        .route("/mem/search", get(search_mem))
        .route("/mem/list", get(list_mem))
        .route("/mem/{id}", get(get_mem).delete(delete_mem))
        .route("/invoke", post(invoke))
        .layer(middleware::from_fn_with_state(state.clone(), auth))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind((host.as_str(), port))
        .await
        .with_context(|| format!("failed to bind {host}:{port}"))?;
    let addr = listener
        .local_addr()
        .context("failed to read bound address")?;
    // Machine-readable startup line (stderr); integration tests parse the port.
    eprintln!("memorypool listening on http://{addr}");
    axum::serve(listener, app).await.context("server error")?;
    Ok(())
}

/// Bearer-token gate: when a token is configured every endpoint except
/// `/health` requires `Authorization: Bearer <token>`.
async fn auth(
    State(state): State<AppState>,
    req: Request<axum::body::Body>,
    next: Next,
) -> Response {
    if let Some(expected) = &state.token {
        if req.uri().path() != "/health" {
            let provided = req
                .headers()
                .get(header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .map(|v| v.strip_prefix("Bearer ").unwrap_or("") == expected)
                .unwrap_or(false);
            if !provided {
                return (
                    StatusCode::UNAUTHORIZED,
                    Json(json!({"error": "unauthorized"})),
                )
                    .into_response();
            }
        }
    }
    next.run(req).await
}

async fn health(State(state): State<AppState>) -> Response {
    Json(json!({
        "ok": true,
        "service": "memorypool",
        "count": state.store.count()
    }))
    .into_response()
}

/// Create a memory from a JSON body (same fields as `memorypool add`).
async fn add_mem(State(state): State<AppState>, body: Bytes) -> Response {
    let Ok(body) = serde_json::from_slice::<Value>(&body) else {
        return error(
            StatusCode::BAD_REQUEST,
            "request body must be a JSON object",
        );
    };
    let Some(obj) = body.as_object() else {
        return error(
            StatusCode::BAD_REQUEST,
            "request body must be a JSON object",
        );
    };
    match memory_from_params(obj) {
        Ok(memory) => match state.store.add(memory) {
            Ok(m) => (StatusCode::CREATED, Json(m)).into_response(),
            Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
        },
        Err(msg) => error(StatusCode::BAD_REQUEST, &msg),
    }
}

async fn search_mem(State(state): State<AppState>, Query(q): Query<SearchQuery>) -> Response {
    let limit = q.limit.unwrap_or(10);
    let results = search::search(
        &state.store.list(None),
        q.q.as_deref().unwrap_or(""),
        q.tag.as_deref(),
        limit,
    );
    Json(results_json(&results)).into_response()
}

async fn list_mem(State(state): State<AppState>, Query(q): Query<SearchQuery>) -> Response {
    let mut items = state.store.list(q.tag.as_deref());
    if let Some(limit) = q.limit {
        items.truncate(limit);
    }
    Json(Value::Array(
        items
            .into_iter()
            .map(|m| serde_json::to_value(m).unwrap_or(Value::Null))
            .collect(),
    ))
    .into_response()
}

async fn get_mem(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    match state.store.get(&id) {
        Some(m) => Json(m).into_response(),
        None => error(StatusCode::NOT_FOUND, "memory not found"),
    }
}

async fn delete_mem(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    match state.store.delete(&id) {
        Ok(true) => Json(json!({"deleted": id})).into_response(),
        Ok(false) => error(StatusCode::NOT_FOUND, "memory not found"),
        Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

/// BIT Remote-tool entry point. Routes on `params.action` (falls back to the
/// top-level `tool` field); every other field inside `params` is treated as
/// an argument of the action.
async fn invoke(State(state): State<AppState>, body: Bytes) -> Response {
    let Ok(body) = serde_json::from_slice::<Value>(&body) else {
        return error(
            StatusCode::BAD_REQUEST,
            "request body must be a JSON object",
        );
    };
    let Some(obj) = body.as_object() else {
        return error(
            StatusCode::BAD_REQUEST,
            "request body must be a JSON object",
        );
    };
    let params = obj
        .get("params")
        .and_then(|p| p.as_object())
        .cloned()
        .unwrap_or_default();
    let action = get_string(&params, &["action"])
        .or_else(|| get_string(obj, &["tool"]))
        .unwrap_or_default();
    match action.as_str() {
        "add" => match memory_from_params(&params) {
            Ok(memory) => match state.store.add(memory) {
                Ok(m) => Json(m).into_response(),
                Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
            },
            Err(msg) => error(StatusCode::BAD_REQUEST, &msg),
        },
        "search" => {
            let query = get_string(&params, &["query", "q"]).unwrap_or_default();
            let tag = get_string(&params, &["tag"]);
            let limit = get_usize(&params, &["limit"]).unwrap_or(10);
            let results = search::search(&state.store.list(None), &query, tag.as_deref(), limit);
            Json(results_json(&results)).into_response()
        }
        "list" => {
            let tag = get_string(&params, &["tag"]);
            let limit = get_usize(&params, &["limit"]);
            let mut items = state.store.list(tag.as_deref());
            if let Some(limit) = limit {
                items.truncate(limit);
            }
            Json(Value::Array(
                items
                    .into_iter()
                    .map(|m| serde_json::to_value(m).unwrap_or(Value::Null))
                    .collect(),
            ))
            .into_response()
        }
        "delete" => {
            let Some(id) = get_string(&params, &["id"]) else {
                return error(StatusCode::BAD_REQUEST, "missing id");
            };
            match state.store.delete(&id) {
                Ok(true) => Json(json!({"deleted": id})).into_response(),
                Ok(false) => error(StatusCode::NOT_FOUND, "memory not found"),
                Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
            }
        }
        "stats" => {
            Json(serde_json::to_value(state.store.stats()).unwrap_or(Value::Null)).into_response()
        }
        other => error(StatusCode::BAD_REQUEST, &format!("unknown action: {other}")),
    }
}

/// Build a memory from shared params (used by `POST /mem` and the `add`
/// action of `POST /invoke`).
fn memory_from_params(
    params: &serde_json::Map<String, Value>,
) -> std::result::Result<Memory, String> {
    let text = get_string(params, &["text", "value"])
        .ok_or_else(|| "missing required field: text".to_string())?;
    let tags = get_tags(params, &["tags"]).unwrap_or_default();
    let importance = get_f32(params, &["importance"]).unwrap_or(0.5);
    let source = get_string(params, &["source"]);
    Ok(new_memory(text, tags, importance, source))
}

/// Search results carry their relevance score.
fn results_json(results: &[(Memory, f64)]) -> Value {
    Value::Array(
        results
            .iter()
            .map(|(m, score)| {
                let mut v = serde_json::to_value(m).unwrap_or(Value::Null);
                if let Some(obj) = v.as_object_mut() {
                    obj.insert("score".into(), json!((*score * 1000.0).round() / 1000.0));
                }
                v
            })
            .collect(),
    )
}

fn error(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({"error": message}))).into_response()
}

#[derive(Debug, Default, serde::Deserialize)]
struct SearchQuery {
    q: Option<String>,
    tag: Option<String>,
    limit: Option<usize>,
}
