//! Imports subscriptions from Google Takeout.
//!
//! Takeout writes `subscriptions.csv` with three columns: the channel ID, the
//! channel URL, and the channel title. The header names change with the
//! language, so the columns are read by position, and the header is the row
//! whose first field is not a channel ID.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};

use crate::store::{Store, Subscription};

/// The file name in Takeout. The folders above it change with the language.
const FILE_NAME: &str = "subscriptions.csv";
/// How deep `find_csv` looks into a Takeout folder.
const MAX_DEPTH: usize = 5;

/// What the import found in the file.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Parsed {
    pub channels: Vec<Subscription>,
    /// Rows that are not a channel, other than the header.
    pub skipped: usize,
}

/// "1 channel" or "2 channels".
pub fn channels(n: usize) -> String {
    if n == 1 { "1 channel".into() } else { format!("{n} channels") }
}

/// What the import did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub added: usize,
    pub already: usize,
    pub skipped: usize,
}

/// Imports the subscriptions in `path`: a `subscriptions.csv` file, or a
/// folder that holds one, such as an unpacked Takeout archive. `now` is the
/// subscription time for new channels, in Unix seconds.
pub fn import(store: &Store, path: &Path, now: i64) -> Result<Summary> {
    if !path.exists() {
        bail!("{} was not found", path.display());
    }
    let file = if path.is_dir() { find_csv(path)? } else { path.to_path_buf() };
    if file.extension().is_some_and(|e| e.eq_ignore_ascii_case("zip") || e.eq_ignore_ascii_case("tgz")) {
        bail!("{} is an archive. Unpack it, then import the folder or {FILE_NAME}.", file.display());
    }
    let bytes = std::fs::read(&file).with_context(|| format!("read {}", file.display()))?;
    let text = String::from_utf8(bytes).with_context(|| format!("{} is not UTF-8 text", file.display()))?;
    let parsed = parse(&text);
    if parsed.channels.is_empty() {
        bail!("{} has no channels. Is it the subscriptions file from Takeout?", file.display());
    }
    let known: std::collections::HashSet<String> = store.subscriptions()?.into_iter().map(|s| s.channel_id).collect();
    let mut summary = Summary { skipped: parsed.skipped, ..Summary::default() };
    for mut s in parsed.channels {
        if known.contains(&s.channel_id) {
            summary.already += 1;
            continue;
        }
        s.subscribed_at = now;
        store.subscribe(&s)?;
        summary.added += 1;
    }
    Ok(summary)
}

/// Finds `subscriptions.csv` in a folder and the folders below it.
pub fn find_csv(dir: &Path) -> Result<PathBuf> {
    let mut found = Vec::new();
    let mut folders = vec![(dir.to_path_buf(), 0)];
    while let Some((folder, depth)) = folders.pop() {
        let entries = std::fs::read_dir(&folder).with_context(|| format!("read {}", folder.display()))?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() && depth < MAX_DEPTH {
                folders.push((path, depth + 1));
            } else if path.file_name().is_some_and(|n| n.eq_ignore_ascii_case(FILE_NAME)) {
                found.push(path);
            }
        }
    }
    found.sort();
    match found.as_slice() {
        [] => bail!("found no {FILE_NAME} in {}. Give the path of the file.", dir.display()),
        [one] => Ok(one.clone()),
        many => bail!(
            "found {} files named {FILE_NAME} in {}. Give the path of one:\n{}",
            many.len(),
            dir.display(),
            many.iter().map(|p| format!("  {}", p.display())).collect::<Vec<_>>().join("\n")
        ),
    }
}

/// Reads the rows of the file. A row is a channel when its first field
/// looks like a channel ID (`UC` and 22 more characters).
pub fn parse(text: &str) -> Parsed {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut parsed = Parsed::default();
    for (i, row) in rows(text).into_iter().enumerate() {
        if row.iter().all(|f| f.trim().is_empty()) {
            continue;
        }
        let id = row.first().map(|f| f.trim()).unwrap_or_default();
        if !is_channel_id(id) {
            // The first row is the header.
            if i > 0 {
                parsed.skipped += 1;
            }
            continue;
        }
        let name = row.get(2).map(|f| f.trim()).filter(|n| !n.is_empty()).unwrap_or(id);
        if parsed.channels.iter().any(|c| c.channel_id == id) {
            continue;
        }
        parsed.channels.push(Subscription {
            channel_id: id.to_string(),
            name: name.to_string(),
            handle: row.get(1).and_then(|url| handle(url)),
            subscribed_at: 0,
        });
    }
    parsed
}

fn is_channel_id(s: &str) -> bool {
    s.len() == 24 && s.starts_with("UC") && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// The `@handle` in a channel URL, if the URL has one. Takeout usually gives
/// `/channel/UC...` URLs, which have none.
fn handle(url: &str) -> Option<String> {
    let path = url.trim().split_once("youtube.com/")?.1;
    let handle = path.split(['/', '?']).next()?;
    (handle.starts_with('@') && handle.len() > 1).then(|| handle.to_string())
}

/// Splits CSV text into rows of fields (RFC 4180): fields in double quotes
/// can hold commas, line breaks, and doubled quotes.
fn rows(text: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match (quoted, c) {
            (true, '"') if chars.peek() == Some(&'"') => {
                chars.next();
                field.push('"');
            }
            (true, '"') => quoted = false,
            (false, '"') if field.is_empty() => quoted = true,
            (false, ',') => row.push(std::mem::take(&mut field)),
            (false, '\r') if chars.peek() == Some(&'\n') => {}
            (false, '\n' | '\r') => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
            }
            // Inside quotes, everything is text.
            (_, c) => field.push(c),
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALPHA: &str = "UCaaaaaaaaaaaaaaaaaaaaaa";
    const BETA: &str = "UCbbbbbbbbbbbbbbbbbbbbbb";

    #[test]
    fn splits_csv() {
        let rows = rows("a,\"b, c\",\"say \"\"hi\"\"\"\r\n\"two\nlines\",x\n\nlast");
        assert_eq!(rows, [vec!["a", "b, c", "say \"hi\""], vec!["two\nlines", "x"], vec![""], vec!["last"]]);
    }

    #[test]
    fn reads_takeout_in_any_language() {
        let text = format!(
            "\u{feff}Kanal-ID,Kanal-URL,Kanaltitel\n\
             {ALPHA},http://www.youtube.com/channel/{ALPHA},\"Alpha, the channel\"\n\
             {BETA},https://www.youtube.com/@beta,Beta\n\
             {ALPHA},http://www.youtube.com/channel/{ALPHA},Alpha again\n\
             not a channel,x,y\n\
             \n"
        );
        let parsed = parse(&text);
        let names: Vec<&str> = parsed.channels.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["Alpha, the channel", "Beta"]);
        assert_eq!(parsed.channels[0].handle, None);
        assert_eq!(parsed.channels[1].handle.as_deref(), Some("@beta"));
        assert_eq!(parsed.skipped, 1);
    }

    #[test]
    fn a_file_without_a_header_or_titles_works() {
        let parsed = parse(&format!("{ALPHA},,\r\n{BETA}"));
        assert_eq!(parsed.channels.len(), 2);
        assert_eq!(parsed.channels[0].name, ALPHA, "the ID when there is no title");
        assert_eq!(parsed.skipped, 0);
    }

    #[test]
    fn channel_ids() {
        assert!(is_channel_id(ALPHA));
        assert!(is_channel_id("UC-_0123456789abcdefghij"));
        assert!(!is_channel_id("UCshort"));
        assert!(!is_channel_id("PLaaaaaaaaaaaaaaaaaaaaaa"));
        assert!(!is_channel_id("UCaaaaaaaaaaaaaaaaaaaa a"));
    }

    #[test]
    fn imports_into_the_store_and_finds_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("Takeout/YouTube und YouTube Music/Abos");
        std::fs::create_dir_all(&folder).unwrap();
        let file = folder.join("subscriptions.csv");
        std::fs::write(&file, format!("Channel Id,Channel Url,Channel Title\n{ALPHA},,Alpha\n{BETA},,Beta\n")).unwrap();
        assert_eq!(find_csv(dir.path()).unwrap(), file);

        let store = Store::open_in_memory().unwrap();
        store
            .subscribe(&Subscription { channel_id: BETA.into(), name: "B".into(), handle: None, subscribed_at: 1 })
            .unwrap();
        let summary = import(&store, dir.path(), 100).unwrap();
        assert_eq!(summary, Summary { added: 1, already: 1, skipped: 0 });
        let subs = store.subscriptions().unwrap();
        assert_eq!(subs.len(), 2);
        let beta = subs.iter().find(|s| s.channel_id == BETA).unwrap();
        assert_eq!((beta.name.as_str(), beta.subscribed_at), ("B", 1), "a channel that is there stays as it is");
        assert_eq!(subs.iter().find(|s| s.channel_id == ALPHA).unwrap().subscribed_at, 100);
    }

    #[test]
    fn clear_errors() {
        let dir = tempfile::tempdir().unwrap();
        let err = import(&Store::open_in_memory().unwrap(), dir.path(), 0).unwrap_err();
        assert!(err.to_string().contains("found no subscriptions.csv"), "{err}");
        let err = import(&Store::open_in_memory().unwrap(), &dir.path().join("none.zip"), 0).unwrap_err();
        assert!(err.to_string().contains("was not found"), "{err}");
        let zip = dir.path().join("takeout-1.zip");
        std::fs::write(&zip, b"PK").unwrap();
        let err = import(&Store::open_in_memory().unwrap(), &zip, 0).unwrap_err();
        assert!(err.to_string().contains("Unpack it"), "{err}");
        let empty = dir.path().join("x.csv");
        std::fs::write(&empty, "a,b,c\n").unwrap();
        let err = import(&Store::open_in_memory().unwrap(), &empty, 0).unwrap_err();
        assert!(err.to_string().contains("has no channels"), "{err}");
        for name in ["a", "b"] {
            let d = dir.path().join(name);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("subscriptions.csv"), "").unwrap();
        }
        let err = find_csv(dir.path()).unwrap_err();
        assert!(err.to_string().contains("found 2 files"), "{err}");
    }
}
