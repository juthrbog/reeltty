//! Small helpers to read InnerTube JSON: text objects, thumbnails, handles,
//! durations, and the English text rules.

use serde_json::Value;

use crate::source::model::{ChannelRef, Thumbnail};

/// Unicode direction marks. YouTube puts them around handles and counts.
const BIDI_MARKS: [char; 2] = ['\u{200E}', '\u{200F}'];

/// Unicode direction embeddings, overrides and isolates (U+202A to U+202E,
/// U+2066 to U+2069). In text from strangers, they can show text in a
/// misleading order. Right-to-left text shows without them.
pub fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// Removes direction marks and controls, and spaces at the ends.
pub fn clean(s: &str) -> String {
    s.chars().filter(|&c| !BIDI_MARKS.contains(&c) && !is_bidi_control(c)).collect::<String>().trim().to_string()
}

pub fn str_at<'a>(v: &'a Value, path: &str) -> Option<&'a str> {
    v.pointer(path)?.as_str()
}

/// The array at `path`, or an empty slice.
pub fn array<'a>(v: &'a Value, path: &str) -> &'a [Value] {
    v.pointer(path).and_then(Value::as_array).map_or(&[], Vec::as_slice)
}

/// The text of a text object: `simpleText`, the joined `runs`, or `content`
/// (view models). `None` when it is missing or empty.
pub fn text(v: &Value) -> Option<String> {
    let raw = if let Some(s) = v.get("simpleText").and_then(Value::as_str) {
        s.to_string()
    } else if let Some(runs) = v.get("runs").and_then(Value::as_array) {
        runs.iter().filter_map(|r| r.get("text").and_then(Value::as_str)).collect()
    } else if let Some(s) = v.get("content").and_then(Value::as_str) {
        s.to_string()
    } else {
        v.as_str()?.to_string()
    };
    Some(clean(&raw)).filter(|s| !s.is_empty())
}

pub fn text_at(v: &Value, path: &str) -> Option<String> {
    text(v.pointer(path)?)
}

/// The thumbnails in the array at `path`. Protocol-relative URLs get `https:`.
pub fn thumbnails(v: &Value, path: &str) -> Vec<Thumbnail> {
    let size =
        |t: &Value, key: &str| t.get(key).and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok()).unwrap_or(0);
    array(v, path)
        .iter()
        .filter_map(|t| {
            let url = t.get("url")?.as_str()?;
            let url = if url.starts_with("//") { format!("https:{url}") } else { url.to_string() };
            Some(Thumbnail { url, width: size(t, "width"), height: size(t, "height") })
        })
        .collect()
}

/// `@name` from `/@name`. `None` for other URLs, such as `/channel/UC...`.
pub fn handle(url: &str) -> Option<String> {
    let s = clean(url);
    let s = s.strip_prefix('/').unwrap_or(&s);
    s.starts_with('@').then(|| s.to_string())
}

/// A channel with the ID and handle from a `browseEndpoint` object.
pub fn channel_ref(name: String, browse: Option<&Value>) -> ChannelRef {
    let browse = browse.unwrap_or(&Value::Null);
    ChannelRef {
        id: str_at(browse, "/browseId").map(str::to_string),
        name,
        handle: str_at(browse, "/canonicalBaseUrl").and_then(handle),
    }
}

/// The channels in a collaboration dialog. `command` is the object that holds
/// `showDialogCommand`.
pub fn collab_channels(command: &Value) -> Vec<ChannelRef> {
    let items = array(
        command,
        "/showDialogCommand/panelLoadingStrategy/inlineContent/dialogViewModel/customContent/listViewModel/listItems",
    );
    items
        .iter()
        .filter_map(|item| {
            let item = item.get("listItemViewModel")?;
            let name = text_at(item, "/title")?;
            let browse = item.pointer("/rendererContext/commandContext/onTap/innertubeCommand/browseEndpoint");
            let mut channel = channel_ref(name, browse);
            // The subtitle is "@handle • 1.29K subscribers".
            if channel.handle.is_none() {
                channel.handle = text_at(item, "/subtitle").and_then(|s| s.split('•').next().and_then(handle));
            }
            Some(channel)
        })
        .collect()
}

/// "1:02:03" to 3723. One number alone is seconds. The first part can have
/// thousands separators, as in "20,772:51:34" for a very long stream. `None`
/// for other text, such as "SHORTS" or "LIVE".
pub fn duration_secs(s: &str) -> Option<u32> {
    let parts: Vec<&str> = s.trim().split(':').collect();
    if parts.len() > 3 {
        return None;
    }
    let mut total: u32 = 0;
    for (i, part) in parts.iter().enumerate() {
        let part = if i == 0 && parts.len() > 1 { thousands(part)? } else { (*part).to_string() };
        if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let n: u32 = part.parse().ok()?;
        // Only the first part can be 60 or more.
        if i > 0 && n >= 60 {
            return None;
        }
        total = total.checked_mul(60)?.checked_add(n)?;
    }
    Some(total)
}

/// "20,772" to "20772". `None` when a comma is not between groups of three digits.
fn thousands(s: &str) -> Option<String> {
    let mut groups = s.split(',');
    let first = groups.next()?;
    let mut out = first.to_string();
    for group in groups {
        if first.is_empty() || first.len() > 3 || group.len() != 3 {
            return None;
        }
        out.push_str(group);
    }
    Some(out)
}

/// What a line of lockup text means. These rules need `hl=en`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Views,
    /// Live viewers, "32 watching".
    Watching,
    /// "3 years ago", "Streamed 1 day ago".
    Published,
    Other,
}

pub fn classify(s: &str) -> Kind {
    if s.contains(" watching") {
        Kind::Watching
    } else if s.ends_with(" views") || s.ends_with(" view") {
        Kind::Views
    } else if s.ends_with(" ago") || s.starts_with("Streamed") {
        Kind::Published
    } else {
        Kind::Other
    }
}

/// The token of a `continuationItemRenderer` or a `continuationItemViewModel`.
pub fn continuation_token(item: &Value) -> Option<String> {
    str_at(item, "/continuationItemRenderer/continuationEndpoint/continuationCommand/token")
        .or_else(|| {
            str_at(item, "/continuationItemViewModel/continuationCommand/innertubeCommand/continuationCommand/token")
        })
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_durations() {
        assert_eq!(duration_secs("1:02:03"), Some(3723));
        assert_eq!(duration_secs("13:59:10"), Some(50350));
        assert_eq!(duration_secs("7:10"), Some(430));
        assert_eq!(duration_secs("0:37"), Some(37));
        assert_eq!(duration_secs("59"), Some(59));
        assert_eq!(duration_secs("SHORTS"), None);
        assert_eq!(duration_secs("LIVE"), None);
        assert_eq!(duration_secs(""), None);
        assert_eq!(duration_secs("1:2:3:4"), None);
        assert_eq!(duration_secs("1:60"), None);
        assert_eq!(duration_secs("1::03"), None);
        assert_eq!(duration_secs("-1:00"), None);
        assert_eq!(duration_secs("99999999999:00"), None);
        assert_eq!(duration_secs("20,772:51:34"), Some(74_782_294));
        assert_eq!(duration_secs("1,000:00:00"), Some(3_600_000));
        assert_eq!(duration_secs("20,77:51:34"), None);
        assert_eq!(duration_secs(",772:51:34"), None);
        assert_eq!(duration_secs("1:2,000:00"), None);
        // A comma is not a thousands separator in seconds alone.
        assert_eq!(duration_secs("1,000"), None);
    }

    #[test]
    fn cleans_direction_marks() {
        assert_eq!(clean("\u{200E}\u{2068}@GrinenkoPro\u{2069} • \u{2068}1.29K\u{2069}"), "@GrinenkoPro • 1.29K");
        assert_eq!(clean("a\u{202E}gpj.exe\u{202C}"), "agpj.exe");
        // Right-to-left text itself stays.
        assert_eq!(clean("שלום"), "שלום");
    }

    #[test]
    fn reads_text_objects() {
        assert_eq!(text(&json!({"simpleText": "a"})).as_deref(), Some("a"));
        assert_eq!(text(&json!({"runs": [{"text": "32"}, {"text": " watching"}]})).as_deref(), Some("32 watching"));
        assert_eq!(text(&json!({"content": "b"})).as_deref(), Some("b"));
        assert_eq!(text(&json!({"simpleText": " "})), None);
        assert_eq!(text(&json!({})), None);
    }

    #[test]
    fn reads_handles() {
        assert_eq!(handle("/@jonhoo").as_deref(), Some("@jonhoo"));
        assert_eq!(handle("@jonhoo").as_deref(), Some("@jonhoo"));
        assert_eq!(handle("/channel/UC123"), None);
    }

    #[test]
    fn makes_thumbnails_absolute() {
        let v =
            json!({"t": [{"url": "//yt3.ggpht.com/a", "width": 88, "height": 88}, {"url": "https://i.ytimg.com/b"}]});
        let t = thumbnails(&v, "/t");
        assert_eq!(t[0], Thumbnail { url: "https://yt3.ggpht.com/a".into(), width: 88, height: 88 });
        assert_eq!(t[1], Thumbnail { url: "https://i.ytimg.com/b".into(), width: 0, height: 0 });
    }

    #[test]
    fn classifies_text() {
        assert_eq!(classify("1.2M views"), Kind::Views);
        assert_eq!(classify("1 view"), Kind::Views);
        assert_eq!(classify("909 thousand views"), Kind::Views);
        assert_eq!(classify("994 watching"), Kind::Watching);
        assert_eq!(classify("3 years ago"), Kind::Published);
        assert_eq!(classify("Streamed 4 months ago"), Kind::Published);
        assert_eq!(classify("Linus Tech Tips"), Kind::Other);
        assert_eq!(classify("909K"), Kind::Other);
    }

    #[test]
    fn reads_both_continuation_types() {
        let renderer =
            json!({"continuationItemRenderer": {"continuationEndpoint": {"continuationCommand": {"token": "a"}}}});
        let view_model = json!({"continuationItemViewModel": {"continuationCommand": {"innertubeCommand": {"continuationCommand": {"token": "b"}}}}});
        assert_eq!(continuation_token(&renderer).as_deref(), Some("a"));
        assert_eq!(continuation_token(&view_model).as_deref(), Some("b"));
        assert_eq!(continuation_token(&json!({"videoRenderer": {}})), None);
    }
}
