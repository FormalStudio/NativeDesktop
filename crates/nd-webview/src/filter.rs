//! A per-request network filter for Chromium pages: an ad blocker's hook. Chromium asks the
//! app's [`RequestFilter`] about every http(s) and ws(s) request on its IO thread, synchronously
//! and without a hop to the main thread, and the answer allows, blocks, redirects or answers the
//! request with a body of the app's. A document's response headers come back through
//! [`RequestFilter::response`], which can add a Content-Security-Policy to the page.
//!
//! One filter serves every page and profile, set once with [`set_request_filter`] and swapped
//! whenever the app's rules change. Requests a Chrome extension makes and requests on schemes
//! other than http(s) and ws(s) never reach it. WebKit pages ignore it.

use std::sync::{Arc, RwLock};

/// What a request loads, as Chromium reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ResourceKind {
    /// A top-level document.
    Document,
    /// An iframe's document.
    Subdocument,
    Stylesheet,
    Script,
    Image,
    Font,
    Object,
    Media,
    /// `fetch` and `XMLHttpRequest`.
    Xhr,
    /// `navigator.sendBeacon` and `<a ping>`.
    Ping,
    CspReport,
    Worker,
    Prefetch,
    Favicon,
    Other,
}

impl ResourceKind {
    /// The request type name filter lists use (`$script`, `$xmlhttprequest`, ...), which is
    /// what adblock-rust's `Request::new` takes.
    pub fn name(self) -> &'static str {
        match self {
            Self::Document => "document",
            Self::Subdocument => "subdocument",
            Self::Stylesheet => "stylesheet",
            Self::Script => "script",
            Self::Image => "image",
            Self::Font => "font",
            Self::Object => "object",
            Self::Media => "media",
            Self::Xhr => "xmlhttprequest",
            Self::Ping => "ping",
            Self::CspReport => "csp_report",
            Self::Worker | Self::Prefetch | Self::Favicon | Self::Other => "other",
        }
    }

    pub fn is_document(self) -> bool {
        matches!(self, Self::Document | Self::Subdocument)
    }
}

/// One request, as the filter sees it.
#[derive(Clone, Copy, Debug)]
pub struct FilterRequest<'a> {
    pub url: &'a str,
    /// The top-level page the request belongs to (its own URL for a top-level navigation);
    /// empty when Chromium names none.
    pub first_party: &'a str,
    /// The origin of the document that made the request; empty for a navigation the user
    /// started.
    pub initiator: &'a str,
    pub kind: ResourceKind,
    pub method: &'a str,
}

/// What happens to a request.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Verdict {
    #[default]
    Allow,
    /// Fails it as blocked by the client (`net::ERR_BLOCKED_BY_CLIENT`).
    Block,
    /// Loads this http(s) URL instead. Chromium follows no redirect to `data:`: answer with
    /// [`Verdict::Respond`] for a body of the app's.
    Redirect(String),
    /// Answers with this body and MIME type, status 200, readable from any origin: a filter
    /// list's `redirect=` resource.
    Respond { mime: String, body: Arc<[u8]> },
}

/// A document's response, for [`RequestFilter::response`].
#[derive(Clone, Copy, Debug)]
pub struct FilterResponse<'a> {
    pub status: u16,
    pub mime: &'a str,
    pub headers: &'a [(String, String)],
}

impl FilterResponse<'_> {
    /// Every value of header `name`, matched without regard to case.
    pub fn header(&self, name: &str) -> impl Iterator<Item = &str> {
        self.headers.iter().filter(move |(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

/// The app's filter. Called on Chromium's IO thread, many times a second: no locks held across
/// slow work, no blocking.
pub trait RequestFilter: Send + Sync + 'static {
    fn request(&self, request: &FilterRequest<'_>) -> Verdict;

    /// A document (top-level or iframe, `text/html`) that was allowed has answered. A returned
    /// policy is added to the page's own, as a `<meta http-equiv="Content-Security-Policy">` at
    /// the top of its `<head>`, so it only narrows what the page may load: what a filter list's
    /// `$csp=` rules do. Directives a meta tag cannot carry (`frame-ancestors`, `report-uri`,
    /// `sandbox`) are ignored by Chromium.
    fn response(&self, _request: &FilterRequest<'_>, _response: &FilterResponse<'_>) -> Option<String> {
        None
    }

    /// The same document: returned HTML goes at the top of its `<head>`, after the policy, so a
    /// `<style>` there applies from the first paint and a `<script>` runs before any of the
    /// page's own. The page's own policy still applies to both.
    fn head(&self, _request: &FilterRequest<'_>, _response: &FilterResponse<'_>) -> Option<String> {
        None
    }
}

impl<F: Fn(&FilterRequest<'_>) -> Verdict + Send + Sync + 'static> RequestFilter for F {
    fn request(&self, request: &FilterRequest<'_>) -> Verdict {
        self(request)
    }
}

static FILTER: RwLock<Option<Arc<dyn RequestFilter>>> = RwLock::new(None);

/// Puts `filter` in front of every Chromium page's requests, replacing the previous one. Takes
/// effect for the next request.
pub fn set_request_filter(filter: impl RequestFilter) {
    *FILTER.write().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::new(filter));
}

pub fn clear_request_filter() {
    *FILTER.write().unwrap_or_else(std::sync::PoisonError::into_inner) = None;
}

/// The filter in place, for engines.
pub fn request_filter() -> Option<Arc<dyn RequestFilter>> {
    FILTER.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
}

/// Whether the filter sees a request for `url`, made by `initiator`.
pub fn filters(url: &str, initiator: &str) -> bool {
    let scheme = url.split_once(':').map(|(s, _)| s).unwrap_or_default();
    matches!(scheme, "http" | "https" | "ws" | "wss") && !initiator.starts_with("chrome-extension:")
}

/// `html` with a `<meta>` carrying a policy and the filter's own HTML inserted right after its
/// `<head>` tag, or where the parser opens the head itself when the document has no such tag:
/// before the first tag or text past the doctype and `<html>`. Streaming: [`HeadInsert::feed`] takes the body in chunks and gives the bytes to
/// send so far.
pub struct HeadInsert {
    tag: Vec<u8>,
    /// Bytes held back while looking for the end of `<head ...>`.
    held: Vec<u8>,
    done: bool,
}

impl HeadInsert {
    /// Past this many bytes with neither the document is passed through unchanged.
    const LIMIT: usize = 64 * 1024;

    pub fn new(policy: Option<&str>, html: &str) -> Self {
        let mut tag = String::new();
        if let Some(policy) = policy {
            let policy = policy.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;");
            tag = format!("<meta http-equiv=\"Content-Security-Policy\" content=\"{policy}\">");
        }
        tag.push_str(html);
        Self { tag: tag.into_bytes(), held: Vec::new(), done: false }
    }

    pub fn feed(&mut self, chunk: &[u8], out: &mut Vec<u8>) {
        if self.done {
            out.extend_from_slice(chunk);
            return;
        }
        self.held.extend_from_slice(chunk);
        if let Some(at) = head_end(&self.held).or_else(|| implied_head(&self.held)) {
            self.insert_at(at, out);
        } else if self.held.len() > Self::LIMIT {
            out.append(&mut self.held);
            self.done = true;
        }
    }

    fn insert_at(&mut self, at: usize, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.held[..at]);
        out.extend_from_slice(&self.tag);
        out.extend_from_slice(&self.held[at..]);
        self.held.clear();
        self.done = true;
    }

    /// Whether bytes are held back, to be sent by a later [`Self::feed`] or [`Self::finish`].
    pub fn holding(&self) -> bool {
        !self.held.is_empty()
    }

    /// The body ended: what is still held back, after the insert when the document had nothing
    /// to place it before.
    pub fn finish(&mut self, out: &mut Vec<u8>) {
        if self.done {
            return;
        }
        let at = self.held.len();
        self.insert_at(at, out);
    }
}

/// Where the `<head ...>` start tag of `html` ends, past its `>`.
fn head_end(html: &[u8]) -> Option<usize> {
    let lower = html.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = find(&lower[from..], b"<head") {
        let start = from + i;
        let next = lower.get(start + 5).copied();
        if matches!(next, Some(b'>' | b' ' | b'\t' | b'\n' | b'\r' | b'/')) {
            return find(&lower[start..], b">").map(|j| start + j + 1);
        }
        // The tag's next byte has not arrived yet.
        next?;
        from = start + 5;
    }
    None
}

/// Whether `html` starts with the start tag `name` (`<html`), `None` while that is still to
/// arrive.
fn opens(html: &[u8], name: &[u8]) -> Option<bool> {
    let n = html.len().min(name.len());
    if !html[..n].eq_ignore_ascii_case(&name[..n]) {
        return Some(false);
    }
    let next = html.get(name.len())?;
    Some(matches!(next, b'>' | b' ' | b'\t' | b'\n' | b'\r' | b'/'))
}

/// Where the parser would open the head of a document with no `<head>` tag: the first tag or
/// text past the byte order mark, the doctype, comments and `<html ...>`. `None` while that is
/// still to arrive, or when a `<head` tag comes first.
fn implied_head(html: &[u8]) -> Option<usize> {
    let mut at = if html.starts_with(b"\xEF\xBB\xBF") { 3 } else { 0 };
    loop {
        while html.get(at).is_some_and(u8::is_ascii_whitespace) {
            at += 1;
        }
        let rest = html.get(at..)?;
        if rest.is_empty() {
            return None;
        }
        let skip = if rest.starts_with(b"<!--") {
            find(rest, b"-->").map(|j| j + 3)
        } else if rest.starts_with(b"<!") || rest.starts_with(b"<?") {
            find(rest, b">").map(|j| j + 1)
        } else if opens(rest, b"<html")? {
            find(rest, b">").map(|j| j + 1)
        } else if opens(rest, b"<head")? {
            return None;
        } else {
            return Some(at);
        };
        at += skip?;
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(chunks: &[&str], policy: &str) -> String {
        run_with(chunks, Some(policy), "")
    }

    fn run_with(chunks: &[&str], policy: Option<&str>, html: &str) -> String {
        let mut insert = HeadInsert::new(policy, html);
        let mut out = Vec::new();
        for c in chunks {
            insert.feed(c.as_bytes(), &mut out);
        }
        insert.finish(&mut out);
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn csp_lands_after_head_across_chunks() {
        let meta = "<meta http-equiv=\"Content-Security-Policy\" content=\"script-src 'none'\">";
        assert_eq!(
            run(&["<!doctype html><html><he", "ad lang=x>", "<title>t</title></head>"], "script-src 'none'"),
            format!("<!doctype html><html><head lang=x>{meta}<title>t</title></head>")
        );
        assert_eq!(run(&["<HEAD><x>"], "script-src 'none'"), format!("<HEAD>{meta}<x>"));
        // `<header>` is not `<head>`: the head the parser opens before it takes the policy.
        let x = "<meta http-equiv=\"Content-Security-Policy\" content=\"x\">";
        assert_eq!(run(&["<header>a</header><body>b"], "x"), format!("{x}<header>a</header><body>b"));
        assert!(run(&["<head>"], "a\"b").contains("content=\"a&quot;b\""));
    }

    #[test]
    fn the_filters_html_follows_the_policy_at_the_top_of_head() {
        let html = "<style>.ad{display:none}</style>";
        assert_eq!(run_with(&["<html><head><title>t"], None, html), format!("<html><head>{html}<title>t"));
        let both = run_with(&["<head>"], Some("x"), html);
        assert_eq!(both, format!("<head><meta http-equiv=\"Content-Security-Policy\" content=\"x\">{html}"));
    }

    #[test]
    fn a_document_without_a_head_tag_gets_the_insert_where_its_head_opens() {
        let h = "<style>.ad{display:none}</style>";
        let with = |chunks: &[&str]| run_with(chunks, None, h);
        assert_eq!(with(&["<!doctype html><title>Ads</title>"]), format!("<!doctype html>{h}<title>Ads</title>"));
        assert_eq!(
            with(&["<!DOCTYPE html>\n<!-- a > b -->", "<ht", "ml lang=en>", "\n<script>x</script>"]),
            format!("<!DOCTYPE html>\n<!-- a > b --><html lang=en>\n{h}<script>x</script>")
        );
        assert_eq!(with(&["\u{feff}plain text"]), format!("\u{feff}{h}plain text"));
        assert_eq!(with(&["<!doctype html><he", "ad><title>t"]), format!("<!doctype html><head>{h}<title>t"));
        assert_eq!(with(&["<!doctype html>"]), format!("<!doctype html>{h}"));
        assert_eq!(with(&["<htmlx>"]), format!("{h}<htmlx>"));
    }

    #[test]
    fn kinds_and_schemes() {
        assert_eq!(ResourceKind::Xhr.name(), "xmlhttprequest");
        assert!(filters("https://a.b/x", "https://c.d"));
        assert!(!filters("chrome://settings", ""));
        assert!(!filters("https://a.b/x", "chrome-extension://abc"));
        assert!(!filters("data:text/plain,x", ""));
    }
}
