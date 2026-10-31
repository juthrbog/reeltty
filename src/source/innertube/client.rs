//! HTTP for InnerTube: the client context, the visitor ID, and errors.

use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use futures::future::BoxFuture;
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue};
use serde_json::{Map, Value, json};
use tokio::sync::OnceCell;

use crate::source::{
    ChannelPage, ItemPage, PlaylistPage, Result, SearchPage, SearchQuery, Source, SourceError, VideoDetails,
};

use super::{params, parse};

const BASE_URL: &str = "https://www.youtube.com";

/// Used when the version cannot be read from the home page. Old versions
/// still work, so this does not need to be current.
const FALLBACK_CLIENT_VERSION: &str = "2.20261029.01.00";

pub(crate) const USER_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36";

const TIMEOUT: Duration = Duration::from_secs(20);

/// The home page read is only an improvement, so it must fail fast.
const HOME_PAGE_TIMEOUT: Duration = Duration::from_secs(5);
const HOME_PAGE_MAX_BYTES: usize = 4 * 1024 * 1024;

const VERSION_MARKER: &[u8] = b"\"INNERTUBE_CLIENT_VERSION\":\"";

/// The `browse` params of a channel's Videos tab. If YouTube changes them,
/// the tab list in the response has the new ones.
const VIDEOS_TAB_PARAMS: &str = "EgZ2aWRlb3PyBgQKAjoA";

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
        let mut scanner = VersionScanner::default();
        while let Some(chunk) = response.chunk().await.map_err(|e| network(&e))? {
            if let Some(version) = scanner.push(&chunk) {
                return Ok(version);
            }
            if scanner.html.len() >= HOME_PAGE_MAX_BYTES {
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

    /// Opens the Videos tab of channel `id` and reads it with `parse`. If
    /// `parse` finds no videos and the tab list names other params for the
    /// Videos tab, asks once more with those params.
    async fn channel_with(&self, id: &str, parse: impl Fn(&Value) -> Result<ChannelPage>) -> Result<ChannelPage> {
        let response = self.call("browse", browse_fields(id, Some(VIDEOS_TAB_PARAMS))).await?;
        match parse(&response) {
            Err(SourceError::Parse(first)) => {
                let Some(params) = videos_tab_params(&response, VIDEOS_TAB_PARAMS) else {
                    return Err(SourceError::Parse(first));
                };
                tracing::info!("channel {id}: {first}. Trying the Videos tab params {params}");
                let response = self.call("browse", browse_fields(id, Some(&params))).await?;
                parse(&response)
            }
            other => other,
        }
    }

    /// Calls `next` and `player` at the same time and reads both with
    /// `parse`. Only `next` must work: `player` adds the duration and the
    /// exact date.
    async fn video_with(
        &self,
        id: &str,
        parse: impl FnOnce(&Value, Option<&Value>) -> Result<VideoDetails>,
    ) -> Result<VideoDetails> {
        let (next, player) = futures::join!(self.call("next", video_fields(id)), self.call("player", video_fields(id)));
        let next = next?;
        let player =
            player.map_err(|e| tracing::warn!("player for {id} failed, so some details are missing: {e}")).ok();
        parse(&next, player.as_ref())
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

fn browse_fields(browse_id: &str, params: Option<&str>) -> Map<String, Value> {
    let mut fields = Map::new();
    fields.insert("browseId".into(), browse_id.into());
    if let Some(params) = params {
        fields.insert("params".into(), params.into());
    }
    fields
}

/// A playlist's browse ID is its ID with `VL` in front.
fn playlist_fields(id: &str) -> Map<String, Value> {
    let id = id.strip_prefix("VL").unwrap_or(id);
    browse_fields(&format!("VL{id}"), None)
}

fn video_fields(id: &str) -> Map<String, Value> {
    let mut fields = Map::new();
    fields.insert("videoId".into(), id.into());
    fields
}

/// The params of the Videos tab in a channel `browse` response, when they
/// differ from `sent`. The tab is found by its URL, because titles follow
/// the language.
fn videos_tab_params(response: &Value, sent: &str) -> Option<String> {
    let tabs = response.pointer("/contents/twoColumnBrowseResultsRenderer/tabs")?.as_array()?;
    let tab =
        tabs.iter().filter_map(|t| t.get("tabRenderer").or_else(|| t.get("expandableTabRenderer"))).find(|t| {
            t.pointer("/endpoint/commandMetadata/webCommandMetadata/url")
                .and_then(Value::as_str)
                .is_some_and(|url| url.ends_with("/videos"))
        })?;
    let params = tab.pointer("/endpoint/browseEndpoint/params")?.as_str()?;
    (params != sent).then(|| params.to_string())
}

/// Finds the client version in HTML that arrives in chunks. A marker or a
/// value can be split between two chunks.
#[derive(Default)]
struct VersionScanner {
    html: Vec<u8>,
    /// Where the search goes on. Bytes before it cannot start a match.
    start: usize,
}

impl VersionScanner {
    fn push(&mut self, chunk: &[u8]) -> Option<String> {
        self.html.extend_from_slice(chunk);
        match scan_client_version(&self.html[self.start..]) {
            Scan::Found(version) => return Some(version),
            // The value goes on in the next chunk. Keep the marker.
            Scan::Unfinished(at) => self.start += at,
            // Keep enough bytes for a marker split between two chunks.
            Scan::NotFound => self.start = self.html.len().saturating_sub(VERSION_MARKER.len() - 1),
        }
        None
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Scan {
    Found(String),
    /// A marker at this offset has no closing quote yet.
    Unfinished(usize),
    NotFound,
}

/// Looks for `"INNERTUBE_CLIENT_VERSION":"<digits and dots>"`. A marker with
/// a value that is not a version is skipped.
fn scan_client_version(html: &[u8]) -> Scan {
    let mut from = 0;
    while let Some(i) = html[from..].windows(VERSION_MARKER.len()).position(|w| w == VERSION_MARKER) {
        let at = from + i;
        let rest = &html[at + VERSION_MARKER.len()..];
        let Some(end) = rest.iter().position(|&b| b == b'"') else { return Scan::Unfinished(at) };
        let value = &rest[..end];
        if !value.is_empty() && value.iter().all(|b| b.is_ascii_digit() || *b == b'.') {
            return Scan::Found(String::from_utf8_lossy(value).into_owned());
        }
        from = at + VERSION_MARKER.len();
    }
    Scan::NotFound
}

#[cfg(test)]
fn find_client_version(html: &[u8]) -> Option<String> {
    match scan_client_version(html) {
        Scan::Found(v) => Some(v),
        _ => None,
    }
}

fn network(err: &reqwest::Error) -> SourceError {
    // The URL adds nothing for the user and hides the cause in a short status line.
    let mut message = error_chain(err);
    if let Some(url) = err.url() {
        message = message.replace(&format!(" for url ({url})"), "");
    }
    SourceError::Network(message)
}

/// The error and all its causes. reqwest's own message hides the cause,
/// such as "connection refused".
pub(crate) fn error_chain(err: &dyn std::error::Error) -> String {
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

    fn channel<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<ChannelPage>> {
        Box::pin(self.channel_with(id, parse::channel_page))
    }

    fn playlist<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<PlaylistPage>> {
        Box::pin(async move {
            let response = self.call("browse", playlist_fields(id)).await?;
            parse::playlist_page(&response)
        })
    }

    fn browse_more<'a>(&'a self, continuation: &'a str) -> BoxFuture<'a, Result<ItemPage>> {
        Box::pin(async move {
            let response = self.call("browse", continuation_fields(continuation)).await?;
            parse::browse_continuation(&response)
        })
    }

    fn video<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<VideoDetails>> {
        Box::pin(self.video_with(id, parse::video_details))
    }
}

#[cfg(test)]
mod tests {
    use tokio::net::TcpListener;

    use super::*;
    use crate::source::filters::SearchFilters;
    use crate::test_server::{Log, Request, reply, serve};

    fn posts(log: &Log) -> Vec<Request> {
        log.lock().unwrap().iter().filter(|r| r.method == "POST").cloned().collect()
    }

    /// No home page, so the fallback version is used.
    async fn api(body: impl Into<String>) -> (InnerTube, Log) {
        let body = body.into();
        let (url, log) =
            serve(move |r| if r.method == "GET" { reply(404, "") } else { reply(200, body.clone()) }).await;
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
        // A non-version value is skipped; a later marker can still match.
        let two = br#""INNERTUBE_CLIENT_VERSION":"x" "INNERTUBE_CLIENT_VERSION":"2.5""#;
        assert_eq!(find_client_version(two).as_deref(), Some("2.5"));
    }

    #[test]
    fn version_split_between_chunks() {
        let html = br#"<script>ytcfg.set({"INNERTUBE_CLIENT_VERSION":"2.20261029.01.00","x":1})"#;
        // Every split point: inside the marker, inside the value, and at the quotes.
        for cut in 1..html.len() {
            let mut scanner = VersionScanner::default();
            let first = scanner.push(&html[..cut]);
            let found = first.or_else(|| scanner.push(&html[cut..]));
            assert_eq!(found.as_deref(), Some("2.20261029.01.00"), "cut at {cut}");
        }
        // Many one-byte chunks.
        let mut scanner = VersionScanner::default();
        let found = html.iter().find_map(|b| scanner.push(std::slice::from_ref(b)));
        assert_eq!(found.as_deref(), Some("2.20261029.01.00"));
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
        // A socket that is bound but does not listen keeps the port, so no
        // other test can take it, and refuses connections.
        let socket = tokio::net::TcpSocket::new_v4().unwrap();
        socket.bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let url = format!("http://{}", socket.local_addr().unwrap());
        let tube = InnerTube::with_base_url(url).unwrap();
        let err = tube.call("search", search_fields("x", None)).await.unwrap_err();
        assert!(matches!(&err, SourceError::Network(m) if !m.is_empty()), "{err:?}");
    }

    fn fixture(name: &str) -> Value {
        let path = format!("{}/tests/fixtures/innertube/{name}.json", env!("CARGO_MANIFEST_DIR"));
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    fn channel_page(name: &str) -> ChannelPage {
        use crate::source::model::ChannelInfo;
        ChannelPage {
            info: ChannelInfo {
                id: "UCx".into(),
                name: name.into(),
                handle: None,
                subscribers_text: None,
                videos_text: None,
                description: None,
                verified: false,
                avatar: Vec::new(),
            },
            videos: ItemPage::default(),
        }
    }

    /// A browse response with a tab list. The Videos tab has `params`.
    fn tabs(params: &str) -> String {
        json!({"contents": {"twoColumnBrowseResultsRenderer": {"tabs": [
            {"tabRenderer": {"endpoint": {
                "browseEndpoint": {"params": "home"},
                "commandMetadata": {"webCommandMetadata": {"url": "/@x/featured"}}
            }}},
            {"tabRenderer": {"endpoint": {
                "browseEndpoint": {"params": params},
                "commandMetadata": {"webCommandMetadata": {"url": "/@x/videos"}}
            }}},
            {"expandableTabRenderer": {"endpoint": {
                "browseEndpoint": {"params": "search"},
                "commandMetadata": {"webCommandMetadata": {"url": "/@x/search"}}
            }}}
        ]}}})
        .to_string()
    }

    /// Fails with a parse error unless the response has `videos`.
    fn needs_videos(response: &Value) -> Result<ChannelPage> {
        if response.get("videos").is_some() {
            Ok(channel_page("found"))
        } else {
            Err(SourceError::Parse("no videos".into()))
        }
    }

    #[test]
    fn reads_the_videos_tab_params() {
        let videos = fixture("browse_channel_videos");
        assert_eq!(videos_tab_params(&videos, VIDEOS_TAB_PARAMS), None);
        assert_eq!(videos_tab_params(&videos, "old").as_deref(), Some(VIDEOS_TAB_PARAMS));
        // The Live tab is selected here, but the Videos tab is still listed.
        let lofi = fixture("browse_channel_streams_lofigirl");
        assert_eq!(videos_tab_params(&lofi, "old").as_deref(), Some(VIDEOS_TAB_PARAMS));
        let other: Value = serde_json::from_str(&tabs("EgZuZXc")).unwrap();
        assert_eq!(videos_tab_params(&other, VIDEOS_TAB_PARAMS).as_deref(), Some("EgZuZXc"));
        assert_eq!(videos_tab_params(&json!({}), VIDEOS_TAB_PARAMS), None);
    }

    #[tokio::test]
    async fn channel_posts_the_videos_tab() {
        let (tube, log) = api(r#"{"videos": 1}"#).await;
        let page = tube.channel_with("UCx", needs_videos).await.unwrap();
        assert_eq!(page.info.name, "found");
        let posts = posts(&log);
        assert_eq!(posts.len(), 1);
        assert_eq!(posts[0].target, "/youtubei/v1/browse?prettyPrint=false");
        let body = posts[0].json();
        assert_eq!(body["browseId"], "UCx");
        assert_eq!(body["params"], VIDEOS_TAB_PARAMS);
    }

    #[tokio::test]
    async fn channel_retries_with_the_params_from_the_tab_list() {
        let (url, log) = serve(|r| {
            if r.method == "GET" {
                return reply(404, "");
            }
            if r.json()["params"] == "EgZuZXc" { reply(200, r#"{"videos": 1}"#) } else { reply(200, tabs("EgZuZXc")) }
        })
        .await;
        let tube = InnerTube::with_base_url(url).unwrap();
        let page = tube.channel_with("UCx", needs_videos).await.unwrap();
        assert_eq!(page.info.name, "found");
        let params: Vec<Value> = posts(&log).iter().map(|r| r.json()["params"].clone()).collect();
        assert_eq!(params, [json!(VIDEOS_TAB_PARAMS), json!("EgZuZXc")]);
    }

    #[tokio::test]
    async fn channel_does_not_retry_with_the_same_params() {
        let (tube, log) = api(tabs(VIDEOS_TAB_PARAMS)).await;
        let err = tube.channel_with("UCx", needs_videos).await.unwrap_err();
        assert!(matches!(&err, SourceError::Parse(m) if m == "no videos"), "{err:?}");
        assert_eq!(posts(&log).len(), 1);
    }

    #[tokio::test]
    async fn channel_retries_only_once_and_only_after_a_parse_error() {
        // The second answer also fails to parse. There is no third request.
        let (tube, log) = api(tabs("EgZuZXc")).await;
        let err = tube.channel_with("UCx", needs_videos).await.unwrap_err();
        assert!(matches!(err, SourceError::Parse(_)), "{err:?}");
        assert_eq!(posts(&log).len(), 2);

        let (tube, log) = api(tabs("EgZuZXc")).await;
        let err = tube.channel_with("UCx", |_| Err(SourceError::Unsupported("x".into()))).await.unwrap_err();
        assert!(matches!(err, SourceError::Unsupported(_)), "{err:?}");
        assert_eq!(posts(&log).len(), 1);
    }

    #[test]
    fn playlist_browse_id_has_one_vl() {
        assert_eq!(playlist_fields("PLBCF2DAC6FFB574DE")["browseId"], "VLPLBCF2DAC6FFB574DE");
        assert_eq!(playlist_fields("VLPLBCF2DAC6FFB574DE")["browseId"], "VLPLBCF2DAC6FFB574DE");
        assert!(playlist_fields("PLx").get("params").is_none());
    }

    #[tokio::test]
    async fn posts_a_playlist_and_a_browse_continuation() {
        let (tube, log) = api("{}").await;
        tube.call("browse", playlist_fields("PLx")).await.unwrap();
        tube.call("browse", continuation_fields("4qmFsgI%3D")).await.unwrap();
        let posts = posts(&log);
        assert!(posts.iter().all(|r| r.target == "/youtubei/v1/browse?prettyPrint=false"));
        let context = json!({"client": {
            "clientName": "WEB", "clientVersion": FALLBACK_CLIENT_VERSION, "hl": "en", "gl": "US"
        }});
        assert_eq!(posts[0].json(), json!({"context": context, "browseId": "VLPLx"}));
        assert_eq!(posts[1].json(), json!({"context": context, "continuation": "4qmFsgI%3D"}));
    }

    #[tokio::test]
    async fn video_calls_next_and_player() {
        let (url, log) = serve(|r| match r.target.as_str() {
            "/youtubei/v1/next?prettyPrint=false" => reply(200, r#"{"from": "next"}"#),
            "/youtubei/v1/player?prettyPrint=false" => reply(200, r#"{"from": "player"}"#),
            _ => reply(404, ""),
        })
        .await;
        let tube = InnerTube::with_base_url(url).unwrap();
        let mut seen = None;
        tube.video_with("aircAruvnKk", |next, player| {
            seen = Some((next["from"].clone(), player.map(|p| p["from"].clone())));
            Err(SourceError::Parse("stop".into()))
        })
        .await
        .unwrap_err();
        assert_eq!(seen, Some((json!("next"), Some(json!("player")))));
        let posts = posts(&log);
        assert_eq!(posts.len(), 2);
        for r in &posts {
            assert_eq!(r.json()["videoId"], "aircAruvnKk");
        }
    }

    #[tokio::test]
    async fn video_works_without_player() {
        let (url, _) = serve(|r| match r.target.as_str() {
            "/youtubei/v1/next?prettyPrint=false" => reply(200, r#"{"from": "next"}"#),
            _ => reply(500, ""),
        })
        .await;
        let tube = InnerTube::with_base_url(url).unwrap();
        let mut player_seen = Some(json!("unset"));
        tube.video_with("x", |_, player| {
            player_seen = player.cloned();
            Err(SourceError::Parse("stop".into()))
        })
        .await
        .unwrap_err();
        assert_eq!(player_seen, None);
    }

    #[tokio::test]
    async fn video_fails_when_next_fails() {
        let (url, _) = serve(|r| match r.target.as_str() {
            "/youtubei/v1/player?prettyPrint=false" => reply(200, "{}"),
            _ => reply(503, ""),
        })
        .await;
        let tube = InnerTube::with_base_url(url).unwrap();
        let err = tube.video_with("x", |_, _| unreachable!()).await.unwrap_err();
        assert!(matches!(err, SourceError::Http { status: 503, .. }), "{err:?}");
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
