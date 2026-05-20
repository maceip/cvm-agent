//! Shared Hyper/Rustls service glue for LLM Attested binaries.
//!
//! The binaries own routing and product behavior. This module owns HTTP body
//! collection, response construction, bounded concurrency, and optional direct
//! Rustls termination so those security-sensitive details do not drift.

use crate::llm_attested_net::{retry_after_secs, RateLimitDecision};
use anyhow::{anyhow, Context, Result};
use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Incoming;
use hyper::header::{HeaderName, HeaderValue, CONTENT_LENGTH};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use rustls::pki_types::CertificateDer;
use rustls::server::WebPkiClientVerifier;
use rustls::{RootCertStore, ServerConfig};
use serde::Serialize;
use std::collections::BTreeMap;
use std::convert::Infallible;
use std::future::Future;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use tokio_rustls::TlsAcceptor;

pub type HandlerFuture = Pin<Box<dyn Future<Output = Result<BufferedResponse>> + Send>>;
pub type Handler<S> = fn(Arc<S>, SocketAddr, HttpRequest) -> HandlerFuture;

#[derive(Clone)]
pub struct HttpServerConfig {
    pub service_name: &'static str,
    pub listen: String,
    pub max_connections: usize,
    pub read_timeout_ms: u64,
    pub body_limit_bytes: usize,
    pub cors: CorsHeaders,
    pub tls_acceptor: Option<TlsAcceptor>,
}

#[derive(Clone)]
pub struct CorsHeaders {
    pub allow_headers: String,
    pub expose_headers: Option<String>,
}

impl CorsHeaders {
    pub fn new(allow_headers: impl Into<String>) -> Self {
        Self {
            allow_headers: allow_headers.into(),
            expose_headers: None,
        }
    }

    pub fn with_expose(mut self, expose_headers: impl Into<String>) -> Self {
        self.expose_headers = Some(expose_headers.into());
        self
    }
}

#[derive(Debug)]
pub struct HttpRequest {
    pub method: String,
    pub path: String,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

#[derive(Default)]
pub struct BufferedResponse {
    response: Option<HttpResponseParts>,
}

struct HttpResponseParts {
    status: u16,
    content_type: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

pub async fn serve_hyper<S>(cfg: HttpServerConfig, state: Arc<S>, handler: Handler<S>) -> Result<()>
where
    S: Send + Sync + 'static,
{
    let listener = TcpListener::bind(&cfg.listen)
        .await
        .with_context(|| format!("bind {}", cfg.listen))?;
    let concurrency = Arc::new(Semaphore::new(cfg.max_connections.max(1)));
    loop {
        let permit = concurrency
            .clone()
            .acquire_owned()
            .await
            .context("acquire HTTP connection permit")?;
        let (stream, addr) = listener.accept().await?;
        let cfg = cfg.clone();
        let state = state.clone();
        tokio::spawn(async move {
            let _permit = permit;
            if let Err(e) = serve_connection(stream, cfg.clone(), state, addr, handler).await {
                eprintln!("[{}] request from {addr} failed: {e}", cfg.service_name);
            }
        });
    }
}

pub fn rustls_acceptor_from_paths(
    cert_path: Option<&PathBuf>,
    key_path: Option<&PathBuf>,
    client_ca_path: Option<&PathBuf>,
) -> Result<Option<TlsAcceptor>> {
    match (cert_path, key_path) {
        (None, None) => Ok(None),
        (Some(cert_path), Some(key_path)) => {
            let config = load_rustls_server_config(cert_path, key_path, client_ca_path)?;
            Ok(Some(TlsAcceptor::from(Arc::new(config))))
        }
        _ => Err(anyhow!(
            "--tls-cert and --tls-key must be provided together"
        )),
    }
}

fn load_rustls_server_config(
    cert_path: &PathBuf,
    key_path: &PathBuf,
    client_ca_path: Option<&PathBuf>,
) -> Result<ServerConfig> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let cert_pem = std::fs::read(cert_path)
        .with_context(|| format!("read TLS cert {}", cert_path.display()))?;
    let key_pem =
        std::fs::read(key_path).with_context(|| format!("read TLS key {}", key_path.display()))?;
    let certs: Vec<CertificateDer<'static>> =
        rustls_pemfile::certs(&mut &*cert_pem).collect::<Result<Vec<_>, _>>()?;
    if certs.is_empty() {
        return Err(anyhow!("TLS cert file contains no certificates"));
    }
    let key = rustls_pemfile::private_key(&mut &*key_pem)?
        .ok_or_else(|| anyhow!("TLS key file contains no private key"))?;

    let mut config = if let Some(client_ca_path) = client_ca_path {
        let ca_pem = std::fs::read(client_ca_path)
            .with_context(|| format!("read TLS client CA {}", client_ca_path.display()))?;
        let ca_certs: Vec<CertificateDer<'static>> =
            rustls_pemfile::certs(&mut &*ca_pem).collect::<Result<Vec<_>, _>>()?;
        if ca_certs.is_empty() {
            return Err(anyhow!("TLS client CA file contains no certificates"));
        }
        let mut roots = RootCertStore::empty();
        for cert in ca_certs {
            roots.add(cert).context("add TLS client CA certificate")?;
        }
        let verifier = WebPkiClientVerifier::builder(Arc::new(roots))
            .build()
            .context("build TLS client CA verifier")?;
        ServerConfig::builder()
            .with_client_cert_verifier(verifier)
            .with_single_cert(certs, key)?
    } else {
        ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key)?
    };
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(config)
}

async fn serve_connection<S>(
    stream: TcpStream,
    cfg: HttpServerConfig,
    state: Arc<S>,
    peer: SocketAddr,
    handler: Handler<S>,
) -> Result<()>
where
    S: Send + Sync + 'static,
{
    let service_cfg = cfg.clone();
    let service = service_fn(move |req| {
        hyper_request(req, service_cfg.clone(), state.clone(), peer, handler)
    });
    match cfg.tls_acceptor {
        Some(acceptor) => {
            let stream = acceptor.accept(stream).await.context("rustls accept")?;
            http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service)
                .await
                .context("serve rustls hyper connection")?;
        }
        None => {
            http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service)
                .await
                .context("serve hyper connection")?;
        }
    }
    Ok(())
}

async fn hyper_request<S>(
    req: Request<Incoming>,
    cfg: HttpServerConfig,
    state: Arc<S>,
    peer: SocketAddr,
    handler: Handler<S>,
) -> Result<Response<Full<Bytes>>, Infallible>
where
    S: Send + Sync + 'static,
{
    match request_from_hyper(req, cfg.read_timeout_ms, cfg.body_limit_bytes).await {
        Ok(req) => match handler(state, peer, req).await {
            Ok(response) => Ok(response.into_hyper_response(&cfg.cors)),
            Err(e) => {
                eprintln!("[{}] request from {peer} failed: {e}", cfg.service_name);
                Ok(error_response(
                    500,
                    serde_json::json!({"error": "internal server error"})
                        .to_string()
                        .into_bytes(),
                    &cfg.cors,
                ))
            }
        },
        Err(e) => Ok(error_response(
            400,
            serde_json::json!({"error": e.to_string()})
                .to_string()
                .into_bytes(),
            &cfg.cors,
        )),
    }
}

async fn request_from_hyper(
    req: Request<Incoming>,
    read_timeout_ms: u64,
    body_limit_bytes: usize,
) -> Result<HttpRequest> {
    let (parts, body) = req.into_parts();
    if let Some(content_length) = parts.headers.get(CONTENT_LENGTH) {
        let length = content_length
            .to_str()
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0);
        if length > body_limit_bytes as u64 {
            return Err(anyhow!("request body too large"));
        }
    }
    let body = tokio::time::timeout(
        std::time::Duration::from_millis(read_timeout_ms),
        Limited::new(body, body_limit_bytes).collect(),
    )
    .await
    .map_err(|_| anyhow!("request timed out"))?
    .map_err(|e| anyhow!("read request body: {e}"))?
    .to_bytes()
    .to_vec();
    let headers = parts
        .headers
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.as_str().to_ascii_lowercase(), value.to_string()))
        })
        .collect();
    Ok(HttpRequest {
        method: parts.method.as_str().to_string(),
        path: parts
            .uri
            .path_and_query()
            .map(|v| v.as_str().to_string())
            .unwrap_or_else(|| "/".to_string()),
        headers,
        body,
    })
}

pub async fn write_json<T: Serialize>(
    response: &mut BufferedResponse,
    status: u16,
    value: &T,
) -> Result<()> {
    write_json_with_headers(response, status, Vec::new(), value).await
}

pub async fn write_json_with_headers<T: Serialize>(
    response: &mut BufferedResponse,
    status: u16,
    extra_headers: Vec<(String, String)>,
    value: &T,
) -> Result<()> {
    let body = serde_json::to_vec_pretty(value).context("encode json response")?;
    write_response_with_headers(response, status, "application/json", extra_headers, body).await
}

pub async fn write_rate_limited(
    response: &mut BufferedResponse,
    decision: RateLimitDecision,
) -> Result<()> {
    let retry_after = retry_after_secs(decision.retry_after_ms).to_string();
    write_json_with_headers(
        response,
        429,
        vec![
            ("Retry-After".to_string(), retry_after),
            ("x-ratelimit-limit".to_string(), decision.limit.to_string()),
            (
                "x-ratelimit-remaining".to_string(),
                decision.remaining.to_string(),
            ),
        ],
        &serde_json::json!({
            "error": "rate limited",
            "retry_after_ms": decision.retry_after_ms,
        }),
    )
    .await
}

pub async fn write_response(
    response: &mut BufferedResponse,
    status: u16,
    content_type: &str,
    body: Vec<u8>,
) -> Result<()> {
    write_response_with_headers(response, status, content_type, Vec::new(), body).await
}

pub async fn write_response_with_headers(
    response: &mut BufferedResponse,
    status: u16,
    content_type: &str,
    extra_headers: Vec<(String, String)>,
    body: Vec<u8>,
) -> Result<()> {
    response.response = Some(HttpResponseParts {
        status,
        content_type: content_type.to_string(),
        headers: extra_headers,
        body,
    });
    Ok(())
}

impl BufferedResponse {
    fn into_hyper_response(self, cors: &CorsHeaders) -> Response<Full<Bytes>> {
        let parts = self.response.unwrap_or_else(|| HttpResponseParts {
            status: 204,
            content_type: "text/plain".to_string(),
            headers: Vec::new(),
            body: Vec::new(),
        });
        hyper_response(parts, cors)
    }
}

fn error_response(status: u16, body: Vec<u8>, cors: &CorsHeaders) -> Response<Full<Bytes>> {
    hyper_response(
        HttpResponseParts {
            status,
            content_type: "application/json".to_string(),
            headers: Vec::new(),
            body,
        },
        cors,
    )
}

fn hyper_response(parts: HttpResponseParts, cors: &CorsHeaders) -> Response<Full<Bytes>> {
    let mut builder = Response::builder()
        .status(StatusCode::from_u16(parts.status).unwrap_or(StatusCode::OK))
        .header("content-type", parts.content_type)
        .header("access-control-allow-origin", "*")
        .header("access-control-allow-headers", cors.allow_headers.as_str());
    if let Some(expose_headers) = &cors.expose_headers {
        builder = builder.header("access-control-expose-headers", expose_headers.as_str());
    }
    for (name, value) in parts.headers {
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_str(&value),
        ) {
            builder = builder.header(name, value);
        }
    }
    builder
        .body(Full::new(Bytes::from(parts.body)))
        .unwrap_or_else(|_| Response::new(Full::new(Bytes::new())))
}
