use axum::{
    extract::{Path, RawQuery},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use bytes::Bytes;
use moka::sync::Cache;
use reqwest::Client;
use std::{sync::Arc, time::Duration};

type AppState = (Arc<Client>, Arc<Cache<String, Bytes>>);

#[tokio::main]
async fn main() {
    let client = Client::builder()
        .pool_max_idle_per_host(100)
        .pool_idle_timeout(Duration::from_secs(90))
        .tcp_keepalive(Duration::from_secs(60))
        .http2_prior_knowledge()
        .build()
        .unwrap();

    let cache = Cache::builder()
        .time_to_live(Duration::from_secs(10))
        .max_capacity(10_000)
        .build();

    let app = Router::new()
        .route("/*path", get(proxy_handler).options(preflight))
        .with_state((Arc::new(client), Arc::new(cache)));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000")
        .await
        .unwrap();

    println!("CORS proxy running on http://0.0.0.0:3000");
    axum::serve(listener, app).await.unwrap();
}

async fn proxy_handler(
    Path(path): Path<String>,
    RawQuery(query): RawQuery,
    axum::extract::State((client, cache)): axum::extract::State<AppState>,
) -> Response {
    // Create cache key with query string
    let cache_key = match &query {
        Some(q) => format!("{}?{}", path, q),
        None => path.clone(),
    };

    // Fast cache lookup (lock-free)
    if let Some(cached) = cache.get(&cache_key) {
        return cors_response(cached);
    }

    // Build GitHub API URL
    let url = match &query {
        Some(q) => format!("https://api.github.com/repos/{}?{}", path, q),
        None => format!("https://api.github.com/repos/{}", path),
    };

    let response = match client
        .get(&url)
        .header("User-Agent", "rust-cors-proxy/1.0")
        .send()
        .await
    {
        Ok(r) => r,
        Err(_) => return error_response(StatusCode::BAD_GATEWAY),
    };

    let body = match response.bytes().await {
        Ok(b) => b,
        Err(_) => return error_response(StatusCode::INTERNAL_SERVER_ERROR),
    };

    cache.insert(cache_key, body.clone());
    cors_response(body)
}

async fn preflight() -> Response {
    let mut headers = HeaderMap::new();
    headers.insert("access-control-allow-origin", HeaderValue::from_static("*"));
    headers.insert("access-control-allow-methods", HeaderValue::from_static("GET, OPTIONS"));
    headers.insert("access-control-max-age", HeaderValue::from_static("3600"));
    (StatusCode::OK, headers).into_response()
}

#[inline(always)]
fn cors_response(body: Bytes) -> Response {
    let mut headers = HeaderMap::new();
    headers.insert("access-control-allow-origin", HeaderValue::from_static("*"));
    headers.insert("content-type", HeaderValue::from_static("application/json"));
    headers.insert("cache-control", HeaderValue::from_static("public, max-age=10"));
    (StatusCode::OK, headers, body).into_response()
}

#[inline(always)]
fn error_response(status: StatusCode) -> Response {
    let mut headers = HeaderMap::new();
    headers.insert("access-control-allow-origin", HeaderValue::from_static("*"));
    (status, headers).into_response()
}
