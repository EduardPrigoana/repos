use axum::{
    extract::{Path, RawQuery, Request},
    http::{HeaderMap, HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use bytes::Bytes;
use moka::sync::Cache;
use reqwest::Client;
use std::{env, sync::Arc, time::Duration};

type AppState = (Arc<Client>, Arc<Cache<String, Bytes>>, Arc<String>);

#[tokio::main]
async fn main() {
    let github_token = env::var("GITHUB_TOKEN")
        .expect("GITHUB_TOKEN environment variable must be set");

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
        .layer(middleware::from_fn(cors_middleware))
        .with_state((Arc::new(client), Arc::new(cache), Arc::new(github_token)));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000")
        .await
        .unwrap();

    println!("CORS proxy running on http://0.0.0.0:3000");
    println!("Allowed origins: *.prigoana.com");
    axum::serve(listener, app).await.unwrap();
}

async fn cors_middleware(request: Request, next: Next) -> Response {
    let origin = request
        .headers()
        .get("origin")
        .and_then(|v| v.to_str().ok());

    if let Some(origin) = origin {
        if !is_allowed_origin(origin) {
            return error_response(StatusCode::FORBIDDEN);
        }
    }

    next.run(request).await
}

#[inline(always)]
fn is_allowed_origin(origin: &str) -> bool {
    if origin == "https://prigoana.com" || origin == "http://prigoana.com" {
        return true;
    }

    if let Some(domain) = origin.strip_prefix("https://") {
        if domain.ends_with(".prigoana.com") {
            return true;
        }
    }

    if let Some(domain) = origin.strip_prefix("http://") {
        if domain.ends_with(".prigoana.com") {
            return true;
        }
    }

    false
}

async fn proxy_handler(
    Path(path): Path<String>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
    axum::extract::State((client, cache, github_token)): axum::extract::State<AppState>,
) -> Response {
    let cache_key = match &query {
        Some(q) => format!("{}?{}", path, q),
        None => path.clone(),
    };

    if let Some(cached) = cache.get(&cache_key) {
        return cors_response(cached, &headers);
    }

    let url = match &query {
        Some(q) => format!("https://api.github.com/repos/{}?{}", path, q),
        None => format!("https://api.github.com/repos/{}", path),
    };

    let response = match client
        .get(&url)
        .header("User-Agent", "rust-cors-proxy/1.0")
        .header("Authorization", format!("Bearer {}", github_token.as_str()))
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
    cors_response(body, &headers)
}

async fn preflight(headers: HeaderMap) -> Response {
    let origin = headers
        .get("origin")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("*");

    let mut response_headers = HeaderMap::new();
    response_headers.insert(
        "access-control-allow-origin",
        HeaderValue::from_str(origin).unwrap_or(HeaderValue::from_static("*")),
    );
    response_headers.insert(
        "access-control-allow-methods",
        HeaderValue::from_static("GET, OPTIONS"),
    );
    response_headers.insert(
        "access-control-allow-headers",
        HeaderValue::from_static("*"),
    );
    response_headers.insert(
        "access-control-max-age",
        HeaderValue::from_static("3600"),
    );
    (StatusCode::OK, response_headers).into_response()
}

#[inline(always)]
fn cors_response(body: Bytes, headers: &HeaderMap) -> Response {
    let origin = headers
        .get("origin")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("*");

    let mut response_headers = HeaderMap::new();
    response_headers.insert(
        "access-control-allow-origin",
        HeaderValue::from_str(origin).unwrap_or(HeaderValue::from_static("*")),
    );
    response_headers.insert(
        "content-type",
        HeaderValue::from_static("application/json"),
    );
    response_headers.insert(
        "cache-control",
        HeaderValue::from_static("public, max-age=10"),
    );
    (StatusCode::OK, response_headers, body).into_response()
}

#[inline(always)]
fn error_response(status: StatusCode) -> Response {
    let mut headers = HeaderMap::new();
    headers.insert(
        "access-control-allow-origin",
        HeaderValue::from_static("*"),
    );
    (status, headers).into_response()
}
