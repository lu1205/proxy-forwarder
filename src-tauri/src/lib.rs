use std::{
    collections::HashMap,
    net::SocketAddr,
    time::{Duration, Instant},
};

use anyhow::{anyhow, Context};
use axum::{
    body::Body,
    extract::State,
    http::{
        header::{CONTENT_DISPOSITION, CONTENT_TYPE},
        HeaderMap, Method, StatusCode,
    },
    response::Response,
    routing::{get, post},
    Json, Router,
};
use reqwest::header::{HeaderName, HeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager, WindowEvent,
};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};
use tokio::net::TcpListener;
use tower_http::cors::{Any, CorsLayer};
use url::Url;

const DEFAULT_BIND_HOST: &str = "127.0.0.1";
const DEFAULT_PORT: u16 = 39291;
const MENU_SHOW: &str = "show";
const MENU_HIDE: &str = "hide";
const MENU_QUIT: &str = "quit";

#[derive(Clone)]
struct AppState {
    client: reqwest::Client,
    app_handle: tauri::AppHandle,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ForwardRequest {
    method: Option<String>,
    url: String,
    headers: Option<HashMap<String, String>>,
    query: Option<HashMap<String, String>>,
    body: Option<Value>,
    timeout_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileDataRequest {
    #[serde(alias = "fileUrl", alias = "link")]
    url: String,
    headers: Option<HashMap<String, String>>,
    timeout_ms: Option<u64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ForwardResponse {
    status: u16,
    headers: HashMap<String, String>,
    body: String,
    body_json: Option<Value>,
    elapsed_ms: u128,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ServiceInfo {
    bind_host: String,
    port: u16,
    base_url: String,
    forward_url: String,
    file_data_url: String,
    health_url: String,
}

#[tauri::command]
fn get_service_info() -> ServiceInfo {
    service_info(DEFAULT_BIND_HOST, DEFAULT_PORT)
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main_window(app);
        }))
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            None,
        ))
        .setup(|app| {
            setup_tray(app)?;
            enable_autostart(app);

            let app_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                if let Err(error) =
                    run_forward_server(app_handle.clone(), DEFAULT_BIND_HOST, DEFAULT_PORT).await
                {
                    let _ = app_handle.emit("forward-server-error", error.to_string());
                }
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![get_service_info])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

fn enable_autostart(app: &tauri::App) {
    let app_handle = app.handle();
    let autostart = app_handle.autolaunch();

    match autostart.enable() {
        Ok(()) => {
            let _ = app_handle.emit("autostart-enabled", true);
        }
        Err(error) => {
            let _ = app_handle.emit("autostart-error", error.to_string());
        }
    }
}

fn setup_tray(app: &mut tauri::App) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, MENU_SHOW, "显示窗口", true, None::<&str>)?;
    let hide = MenuItem::with_id(app, MENU_HIDE, "隐藏窗口", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, MENU_QUIT, "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &hide, &quit])?;
    let icon = app.default_window_icon().cloned();

    let mut tray = TrayIconBuilder::with_id("main")
        .menu(&menu)
        .tooltip("接口请求转发客户端")
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            MENU_SHOW => show_main_window(app),
            MENU_HIDE => hide_main_window(app),
            MENU_QUIT => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| match event {
            TrayIconEvent::DoubleClick {
                button: MouseButton::Left,
                ..
            }
            | TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } => show_main_window(tray.app_handle()),
            _ => {}
        });

    if let Some(icon) = icon {
        tray = tray.icon(icon);
    }

    tray.build(app)?;
    Ok(())
}

fn show_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn hide_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.hide();
    }
}

async fn run_forward_server(
    app_handle: tauri::AppHandle,
    host: &str,
    port: u16,
) -> anyhow::Result<()> {
    let address: SocketAddr = format!("{host}:{port}")
        .parse()
        .with_context(|| format!("invalid bind address {host}:{port}"))?;

    let listener = TcpListener::bind(address)
        .await
        .with_context(|| format!("failed to bind local forwarding server on {address}"))?;

    let state = AppState {
        client: reqwest::Client::new(),
        app_handle: app_handle.clone(),
    };

    let router = Router::new()
        .route("/health", get(health))
        .route("/forward", post(forward))
        .route("/file-data", post(file_data))
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
        .with_state(state);

    let _ = app_handle.emit("forward-server-ready", service_info(host, port));
    axum::serve(listener, router).await?;
    Ok(())
}

async fn health() -> Json<Value> {
    // 打印请求参数
    print!("Forward request:");
    Json(json!({
        "ok": true,
        "name": "proxy-forwarder",
        "forwardUrl": service_info(DEFAULT_BIND_HOST, DEFAULT_PORT).forward_url,
        "fileDataUrl": service_info(DEFAULT_BIND_HOST, DEFAULT_PORT).file_data_url
    }))
}

async fn file_data(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<FileDataRequest>,
) -> Result<Response, (StatusCode, Json<Value>)> {
    let started_at = Instant::now();
    let result = execute_file_data(&state.client, &headers, payload, started_at).await;

    match result {
        Ok(response) => {
            let _ = state.app_handle.emit(
                "file-data-request-finished",
                json!({
                    "status": response.status().as_u16(),
                    "elapsedMs": started_at.elapsed().as_millis()
                }),
            );
            Ok(response)
        }
        Err(error) => {
            let _ = state.app_handle.emit(
                "file-data-request-failed",
                json!({
                    "message": error.to_string(),
                    "elapsedMs": started_at.elapsed().as_millis()
                }),
            );
            Err((
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": error.to_string(),
                    "elapsedMs": started_at.elapsed().as_millis()
                })),
            ))
        }
    }
}

async fn forward(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<ForwardRequest>,
) -> Result<Json<ForwardResponse>, (StatusCode, Json<Value>)> {
    let started_at = Instant::now();
    let result = execute_forward(&state.client, &headers, payload, started_at).await;

    match result {
        Ok(response) => {
            let _ = state.app_handle.emit(
                "forward-request-finished",
                json!({
                    "status": response.status,
                    "elapsedMs": response.elapsed_ms
                }),
            );
            Ok(Json(response))
        }
        Err(error) => {
            let _ = state.app_handle.emit(
                "forward-request-failed",
                json!({
                    "message": error.to_string(),
                    "elapsedMs": started_at.elapsed().as_millis()
                }),
            );
            Err((
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": error.to_string(),
                    "elapsedMs": started_at.elapsed().as_millis()
                })),
            ))
        }
    }
}

async fn execute_forward(
    client: &reqwest::Client,
    incoming_headers: &HeaderMap,
    payload: ForwardRequest,
    started_at: Instant,
) -> anyhow::Result<ForwardResponse> {
    let method = payload
        .method
        .as_deref()
        .unwrap_or("GET")
        .parse::<Method>()
        .context("invalid HTTP method")?;

    let mut url = Url::parse(&payload.url).context("url must be an absolute http/https URL")?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(anyhow!("only http and https URLs are supported"));
    }

    if let Some(query) = payload.query {
        let mut pairs = url.query_pairs_mut();
        for (key, value) in query {
            pairs.append_pair(&key, &value);
        }
    }

    let timeout = Duration::from_millis(payload.timeout_ms.unwrap_or(30_000).clamp(1_000, 120_000));
    let mut request = client.request(method, url).timeout(timeout);

    if let Some(headers) = payload.headers {
        for (key, value) in headers {
            let name = key
                .parse::<HeaderName>()
                .with_context(|| format!("invalid header name: {key}"))?;
            let value = HeaderValue::from_str(&value)
                .with_context(|| format!("invalid header value for {key}"))?;
            request = request.header(name, value);
        }
    }

    // Keep an optional trace id across the forwarding boundary for easier debugging.
    if let Some(trace_id) = incoming_headers.get("x-request-id") {
        request = request.header("x-request-id", trace_id.clone());
    }

    if let Some(body) = payload.body {
        request = match body {
            Value::String(text) => request.body(text),
            other => request.json(&other),
        };
    }

    let response = request.send().await.context("target request failed")?;
    let status = response.status().as_u16();
    let headers = response
        .headers()
        .iter()
        .filter_map(|(key, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (key.as_str().to_string(), value.to_string()))
        })
        .collect::<HashMap<_, _>>();

    let body = response.text().await.context("failed to read target response")?;
    let body_json = serde_json::from_str(&body).ok();

    Ok(ForwardResponse {
        status,
        headers,
        body,
        body_json,
        elapsed_ms: started_at.elapsed().as_millis(),
    })
}

async fn execute_file_data(
    client: &reqwest::Client,
    incoming_headers: &HeaderMap,
    payload: FileDataRequest,
    _started_at: Instant,
) -> anyhow::Result<Response> {
    let url = Url::parse(&payload.url).context("url must be an absolute http/https URL")?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(anyhow!("only http and https URLs are supported"));
    }

    let timeout = Duration::from_millis(payload.timeout_ms.unwrap_or(60_000).clamp(1_000, 300_000));
    let mut request = client.get(url).timeout(timeout);

    if let Some(headers) = payload.headers {
        for (key, value) in headers {
            let name = key
                .parse::<HeaderName>()
                .with_context(|| format!("invalid header name: {key}"))?;
            let value = HeaderValue::from_str(&value)
                .with_context(|| format!("invalid header value for {key}"))?;
            request = request.header(name, value);
        }
    }

    if let Some(trace_id) = incoming_headers.get("x-request-id") {
        request = request.header("x-request-id", trace_id.clone());
    }

    let target_response = request.send().await.context("target file request failed")?;
    let status = target_response.status();
    let target_headers = target_response.headers().clone();
    let bytes = target_response
        .bytes()
        .await
        .context("failed to read target file response")?;

    let mut response = Response::builder().status(status);

    if let Some(content_type) = target_headers.get(CONTENT_TYPE) {
        response = response.header(CONTENT_TYPE, content_type.clone());
    } else {
        response = response.header(CONTENT_TYPE, "application/octet-stream");
    }

    if let Some(content_disposition) = target_headers.get(CONTENT_DISPOSITION) {
        response = response.header(CONTENT_DISPOSITION, content_disposition.clone());
    }

    response
        .body(Body::from(bytes))
        .context("failed to build file data response")
}

fn service_info(host: &str, port: u16) -> ServiceInfo {
    let base_url = format!("http://{host}:{port}");
    ServiceInfo {
        bind_host: host.to_string(),
        port,
        forward_url: format!("{base_url}/forward"),
        file_data_url: format!("{base_url}/file-data"),
        health_url: format!("{base_url}/health"),
        base_url,
    }
}
