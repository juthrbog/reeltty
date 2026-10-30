//! HTTP for InnerTube: the client context, the visitor ID, and errors.

use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use futures::future::BoxFuture;
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue};
use serde_json::{Map, Value, json};
use tokio::sync::OnceCell;

use crate::source::{Result, SearchPage, SearchQuery, Source, SourceError};

use super::{params, parse};

const BASE_URL: &str = "https://www.youtube.com";

/// Used when the version cannot be read from the home page. Old versions
/// still work, so this does not need to be current.
const FALLBACK_CLIENT_VERSION: &str = "2.20261029.01.00";

const USER_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36";

const TIMEOUT: Duration = Duration::from_secs(20);

/// The home page read is only an improvement, so it must fail fast.
const HOME_PAGE_TIMEOUT: Duration = Duration::from_secs(5);
const HOME_PAGE_MAX_BYTES: usize = 4 * 1024 * 1024;

const VERSION_MARKER: &[u8] = b"\"INNERTUBE_CLIENT_VERSION\":\"";

pub struct InnerTube {
    http: reqwest::Client,
    base_url: String,
    /// Set on first use, from the home page or the fallback.
    client_version: OnceCell<String>,
    /// From `.responseContext.visitorData`. Sent back unchanged.
    visitor_data: Mutex<Option<String>>,
}

impl InnerTube {
    pub fn new() -> Result<Self> {
        Self::with_base_url(BASE_URL)
    }

    /// `base_url` is the site root with no trailing `/`, for example a local
    /// test server.
    pub fn with_base_url(base_url: impl Into<String>) -> Result<Self> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .default_headers(headers)
            .gzip(true)
            .timeout(TIMEOUT)
            .build()
            .map_err(|e| SourceError::Program(format!("cannot build the HTTP client: {}", error_chain(&e))))?;
        Ok(Self { http, base_url: base_url.into(), client_version: OnceCell::new(), visitor_data: Mutex::new(None) })
    }

    async fn client_version(&self) -> &str {
        self.client_version
            .get_or_init(|| async {
                match self.fetch_client_version().await {
                    Ok(version) => {
                        tracing::info!("InnerTube client version {version}");
                        version
                    }
                    Err(e) => {
                        tracing::warn!("cannot read the client version, using {FALLBACK_CLIENT_VERSION}: {e}");
                        FALLBACK_CLIENT_VERSION.to_string()
                    }
                }
            })
            .await
    }

    /// Reads `INNERTUBE_CLIENT_VERSION` from the home page HTML.
    async fn fetch_client_version(&self) -> Result<String> {
        let mut response = self
            .http
            .get(format!("{}/", self.base_url))
            .timeout(HOME_PAGE_TIMEOUT)
            .send()
            .await
            .map_err(|e| network(&e))?;
        let status = response.status();
        if !status.is_success() {
            return Err(SourceError::Http { status: status.as_u16(), message: "home page".into() });
        }
        let mut html = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|e| network(&e))? {
            // Search only the new bytes, plus enough old ones for a marker
            // that is split between two chunks.
            let start = html.len().saturating_sub(VERSION_MARKER.len());
            html.extend_from_slice(&chunk);
            if let Some(version) = find_client_version(&html[start..]) {
                return Ok(version);
            }
            if html.len() >= HOME_PAGE_MAX_BYTES {
                break;
            }
        }
        Err(SourceError::Parse("no INNERTUBE_CLIENT_VERSION in the home page".into()))
    }

    fn context(version: &str) -> Value {
        json!({"client": {"clientName": "WEB", "clientVersion": version, "hl": "en", "gl": "US"}})
    }

    /// POSTs `{context, ...fields}` to `endpoint` and returns the JSON response.
    async fn call(&self, endpoint: &str, fields: Map<String, Value>) -> Result<Value> {
        let version = self.client_version().await;
        let mut body = fields;
        body.insert("context".into(), Self::context(version));

        let mut request = self
            .http
            .post(format!("{}/youtubei/v1/{endpoint}?prettyPrint=false", self.base_url))
            .header("X-Youtube-Client-Name", "1")
            .header("X-Youtube-Client-Version", version)
            .body(Value::Object(body).to_string());
        let visitor = self.visitor_data.lock().unwrap_or_else(PoisonError::into_inner).clone();
        if let Some(value) = visitor.and_then(|v| HeaderValue::from_str(&v).ok()) {
            request = request.header("X-Goog-Visitor-Id", value);
        }

        let response = request.send().await.map_err(|e| network(&e))?;
        let status = response.status();
        let text = response.text().await.map_err(|e| network(&e))?;
        let json: Option<Value> = serde_json::from_str(&text).ok();
        if !status.is_success() {
            let message = json
                .as_ref()
                .and_then(|j| j.pointer("/error/message"))
                .and_then(Value::as_str)
                .map_or_else(|| status.canonical_reason().unwrap_or("error").to_string(), str::to_string);
            return Err(SourceError::Http { status: status.as_u16(), message });
        }
        let json = json.ok_or_else(|| SourceError::Parse(format!("{endpoint}: the response is not JSON")))?;
        if let Some(v) = json.pointer("/responseContext/visitorData").and_then(Value::as_str) {
            *self.visitor_data.lock().unwrap_or_else(PoisonError::into_inner) = Some(v.to_string());
        }
        Ok(json)
    }
}

fn search_fields(text: &str, params: Option<String>) -> Map<String, Value> {
    let mut fields = Map::new();
    fields.insert("query".into(), text.into());
    if let Some(params) = params {
        fields.insert("params".into(), params.into());
    }
    fields
}

/// A continuation sends only the token, never the query or params.
fn continuation_fields(token: &str) -> Map<String, Value> {
    let mut fields = Map::new();
    fields.insert("continuation".into(), token.into());
    fields
}

fn find_client_version(html: &[u8]) -> Option<String> {
    let at = html.windows(VERSION_MARKER.len()).position(|w| w == VERSION_MARKER)?;
    let rest = &html[at + VERSION_MARKER.len()..];
    let end = rest.iter().position(|&b| b == b'"')?;
    let version = std::str::from_utf8(&rest[..end]).ok()?;
    let valid = !version.is_empty() && version.bytes().all(|b| b.is_ascii_digit() || b == b'.');
    valid.then(|| version.to_string())
}

fn network(err: &reqwest::Error) -> SourceError {
    SourceError::Network(error_chain(err))
}

/// The error and all its causes. reqwest's own message hides the cause,
/// such as "connection refused".
fn error_chain(err: &dyn std::error::Error) -> String {
    let mut message = err.to_string();
    let mut source = err.source();
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    message
}

impl Source for InnerTube {
    fn name(&self) -> &'static str {
        "InnerTube"
    }

    fn search<'a>(&'a self, query: &'a SearchQuery) -> BoxFuture<'a, Result<SearchPage>> {
        Box::pin(async move {
            let fields = search_fields(&query.text, params::encode(&query.filters));
            let response = self.call("search", fields).await?;
            parse::search_page(&response)
        })
    }

    fn search_more<'a>(&'a self, continuation: &'a str) -> BoxFuture<'a, Result<SearchPage>> {
        Box::pin(async move {
            let response = self.call("search", continuation_fields(continuation)).await?;
            parse::search_continuation(&response)
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::*;
    use crate::source::filters::SearchFilters;

    #[derive(Debug, Clone)]
    struct Request {
        method: String,
        target: String,
        /// Names are lower case.
        headers: Vec<(String, String)>,
        body: String,
    }

    impl Request {
        fn header(&self, name: &str) -> Option<&str> {
            self.headers.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
        }

        fn json(&self) -> Value {
            serde_json::from_str(&self.body).unwrap()
        }
    }

    struct Reply {
        status: u16,
        body: String,
    }

    fn reply(status: u16, body: impl Into<String>) -> Reply {
        Reply { status, body: body.into() }
    }

    type Log = Arc<Mutex<Vec<Request>>>;

    /// A tiny HTTP/1.1 server. It records every request and answers with
    /// `handler`. The connection closes after each reply.
    async fn serve(handler: impl Fn(&Request) -> Reply + Send + Sync + 'static) -> (String, Log) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let log: Log = Arc::default();
        let handler = Arc::new(handler);
        let server_log = log.clone();
        tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let handler = handler.clone();
                let log = server_log.clone();
                tokio::spawn(async move {
                    let request = read_request(&mut stream).await;
                    let reply = handler(&request);
                    log.lock().unwrap().push(request);
                    let head = format!(
                        "HTTP/1.1 {} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        reply.status,
                        reply.body.len()
                    );
                    stream.write_all(head.as_bytes()).await.unwrap();
                    stream.write_all(reply.body.as_bytes()).await.unwrap();
                    stream.shutdown().await.unwrap();
                });
            }
        });
        (base_url, log)
    }

    async fn read_request(stream: &mut tokio::net::TcpStream) -> Request {
        let mut buf = Vec::new();
        let head_end = loop {
            if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break i;
            }
            let mut chunk = [0; 4096];
            let n = stream.read(&mut chunk).await.unwrap();
            assert!(n > 0, "connection closed before the headers ended");
            buf.extend_from_slice(&chunk[..n]);
        };
        let head = String::from_utf8(buf[..head_end].to_vec()).unwrap();
        let mut lines = head.split("\r\n");
        let mut start = lines.next().unwrap().split(' ');
        let method = start.next().unwrap().to_string();
        let target = start.next().unwrap().to_string();
        let headers: Vec<(String, String)> = lines
            .filter_map(|l| l.split_once(':'))
            .map(|(n, v)| (n.trim().to_ascii_lowercase(), v.trim().to_string()))
            .collect();
        let length: usize = headers.iter().find(|(n, _)| n == "content-length").map_or(0, |(_, v)| v.parse().unwrap());
        let mut body = buf[head_end + 4..].to_vec();
        while body.len() < length {
            let mut chunk = [0; 4096];
            let n = stream.read(&mut chunk).await.unwrap();
            assert!(n > 0, "connection closed before the body ended");
            body.extend_from_slice(&chunk[..n]);
        }
        Request { method, target, headers, body: String::from_utf8(body).unwrap() }
    }

    fn posts(log: &Log) -> Vec<Request> {
        log.lock().unwrap().iter().filter(|r| r.method == "POST").cloned().collect()
    }

    /// No home page, so the fallback version is used.
    async fn api(body: &'static str) -> (InnerTube, Log) {
        let (url, log) = serve(move |r| if r.method == "GET" { reply(404, "") } else { reply(200, body) }).await;
        (InnerTube::with_base_url(url).unwrap(), log)
    }

    #[tokio::test]
    async fn posts_the_first_page() {
        let (tube, log) = api("{}").await;
        tube.call("search", search_fields("rust programming", Some("EgIQAQ==".into()))).await.unwrap();

        let posts = posts(&log);
        assert_eq!(posts.len(), 1);
        let r = &posts[0];
        assert_eq!(r.target, "/youtubei/v1/search?prettyPrint=false");
        assert!(!r.target.contains("key="));
        assert_eq!(r.header("content-type"), Some("application/json"));
        assert!(r.header("user-agent").unwrap().starts_with("Mozilla/5.0"));
        assert_eq!(r.header("x-youtube-client-name"), Some("1"));
        assert_eq!(r.header("x-youtube-client-version"), Some(FALLBACK_CLIENT_VERSION));
        assert_eq!(r.header("x-goog-visitor-id"), None);
        assert_eq!(
            r.json(),
            json!({
                "context": {"client": {
                    "clientName": "WEB", "clientVersion": FALLBACK_CLIENT_VERSION, "hl": "en", "gl": "US"
                }},
                "query": "rust programming",
                "params": "EgIQAQ==",
            })
        );
    }

    #[tokio::test]
    async fn leaves_out_params_with_no_filters() {
        let (tube, log) = api("{}").await;
        tube.call("search", search_fields("cats", None)).await.unwrap();
        let body = posts(&log)[0].json();
        assert_eq!(body["query"], "cats");
        assert!(body.get("params").is_none());
    }

    #[tokio::test]
    async fn posts_a_continuation_with_only_the_token() {
        let (tube, log) = api("{}").await;
        tube.call("search", continuation_fields("EpIDEg%3D%3D")).await.unwrap();
        let body = posts(&log)[0].json();
        assert_eq!(
            body,
            json!({
                "context": {"client": {
                    "clientName": "WEB", "clientVersion": FALLBACK_CLIENT_VERSION, "hl": "en", "gl": "US"
                }},
                "continuation": "EpIDEg%3D%3D",
            })
        );
    }

    #[tokio::test]
    async fn sends_back_the_visitor_id() {
        let (url, log) = serve(|r| {
            if r.method == "GET" {
                return reply(404, "");
            }
            // Only the first response has a visitor ID, as on the real server.
            if r.header("x-goog-visitor-id").is_none() {
                reply(200, r#"{"responseContext":{"visitorData":"CgtWaXNpdG9y%3D%3D"}}"#)
            } else {
                reply(200, r#"{"responseContext":{}}"#)
            }
        })
        .await;
        let tube = InnerTube::with_base_url(url).unwrap();
        for _ in 0..3 {
            tube.call("search", search_fields("x", None)).await.unwrap();
        }
        let ids: Vec<_> = posts(&log).iter().map(|r| r.header("x-goog-visitor-id").map(str::to_string)).collect();
        let id = Some("CgtWaXNpdG9y%3D%3D".to_string());
        assert_eq!(ids, [None, id.clone(), id]);
    }

    #[tokio::test]
    async fn reads_the_client_version_once() {
        let (url, log) = serve(|r| {
            if r.method == "GET" {
                reply(200, r#"<html><script>ytcfg.set({"INNERTUBE_API_KEY":"k","INNERTUBE_CLIENT_VERSION":"2.20991231.07.00","X":1})</script>"#)
            } else {
                reply(200, "{}")
            }
        })
        .await;
        let tube = InnerTube::with_base_url(url).unwrap();
        tube.call("search", search_fields("x", None)).await.unwrap();
        tube.call("search", continuation_fields("t")).await.unwrap();

        let requests = log.lock().unwrap().clone();
        let gets: Vec<_> = requests.iter().filter(|r| r.method == "GET").collect();
        assert_eq!(gets.len(), 1);
        assert_eq!(gets[0].target, "/");
        for r in posts(&log) {
            assert_eq!(r.json()["context"]["client"]["clientVersion"], "2.20991231.07.00");
            assert_eq!(r.header("x-youtube-client-version"), Some("2.20991231.07.00"));
        }
    }

    #[tokio::test]
    async fn falls_back_when_the_home_page_has_no_version() {
        let (url, log) =
            serve(|r| if r.method == "GET" { reply(200, "<html>consent</html>") } else { reply(200, "{}") }).await;
        let tube = InnerTube::with_base_url(url).unwrap();
        tube.call("search", search_fields("x", None)).await.unwrap();
        assert_eq!(posts(&log)[0].json()["context"]["client"]["clientVersion"], FALLBACK_CLIENT_VERSION);
    }

    #[tokio::test]
    async fn falls_back_when_the_home_page_fails() {
        let (tube, log) = api("{}").await;
        tube.call("search", search_fields("x", None)).await.unwrap();
        assert_eq!(posts(&log)[0].json()["context"]["client"]["clientVersion"], FALLBACK_CLIENT_VERSION);
    }

    #[test]
    fn finds_the_client_version() {
        assert_eq!(find_client_version(br#"a"INNERTUBE_CLIENT_VERSION":"2.1.0"b"#).as_deref(), Some("2.1.0"));
        assert_eq!(find_client_version(br#""INNERTUBE_CLIENT_VERSION":"""#), None);
        assert_eq!(find_client_version(br#""INNERTUBE_CLIENT_VERSION":"<x>""#), None);
        assert_eq!(find_client_version(br#""INNERTUBE_CLIENT_VERSION":"2.1"#), None);
        assert_eq!(find_client_version(b"nothing"), None);
    }

    #[tokio::test]
    async fn maps_an_http_error_with_its_message() {
        let (url, _) = serve(|r| {
            if r.method == "GET" {
                reply(404, "")
            } else {
                reply(
                    400,
                    r#"{"error":{"code":400,"message":"Precondition check failed.","status":"FAILED_PRECONDITION"}}"#,
                )
            }
        })
        .await;
        let tube = InnerTube::with_base_url(url).unwrap();
        let err = tube.call("search", search_fields("x", None)).await.unwrap_err();
        assert!(
            matches!(&err, SourceError::Http { status: 400, message } if message == "Precondition check failed."),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn maps_an_http_error_without_json() {
        let (url, _) = serve(|_| reply(503, "<html>down</html>")).await;
        let tube = InnerTube::with_base_url(url).unwrap();
        let err = tube.call("search", search_fields("x", None)).await.unwrap_err();
        assert!(
            matches!(&err, SourceError::Http { status: 503, message } if message == "Service Unavailable"),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn maps_a_body_that_is_not_json() {
        let (tube, _) = api("not json").await;
        let err = tube.call("search", search_fields("x", None)).await.unwrap_err();
        assert!(matches!(err, SourceError::Parse(_)), "{err:?}");
    }

    #[tokio::test]
    async fn maps_a_refused_connection() {
        // Bind a port, then close it, so nothing listens there.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        let tube = InnerTube::with_base_url(url).unwrap();
        let err = tube.call("search", search_fields("x", None)).await.unwrap_err();
        assert!(matches!(&err, SourceError::Network(m) if !m.is_empty()), "{err:?}");
    }

    #[test]
    fn is_send_and_sync() {
        fn check<T: Send + Sync>() {}
        check::<InnerTube>();
    }

    #[tokio::test]
    #[ignore = "uses the network"]
    async fn live_client_version_and_visitor_id() {
        let tube = InnerTube::new().unwrap();
        let version = tube.fetch_client_version().await.unwrap();
        assert!(version.starts_with("2."), "{version}");
        let response = tube.call("search", search_fields("rust programming", None)).await.unwrap();
        assert!(response.pointer("/contents/twoColumnSearchResultsRenderer").is_some());
        assert!(tube.visitor_data.lock().unwrap().is_some());
    }

    #[tokio::test]
    #[ignore = "uses the network"]
    async fn live_search() {
        let tube = InnerTube::new().unwrap();
        let query = SearchQuery { text: "rust programming".into(), filters: SearchFilters::default() };
        let page = tube.search(&query).await.unwrap();
        assert!(!page.items.is_empty());
        let token = page.continuation.expect("a continuation token");
        let more = tube.search_more(&token).await.unwrap();
        assert!(!more.items.is_empty());
    }
}
