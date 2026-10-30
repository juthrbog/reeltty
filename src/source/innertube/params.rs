//! The search `params` value: base64 of a small protobuf message.
//!
//! ```text
//! 1: sort       (varint) 3 = view count. Relevance (0) is left out.
//! 2: filters    (message)
//!    1: upload date  1 = last hour .. 5 = this year
//!    2: type         1 = video, 2 = channel, 3 = playlist
//!    3: duration     4 = under 3 min, 5 = 3-20 min, 2 = over 20 min
//! ```

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;

use crate::source::filters::{Duration, ResultType, SearchFilters, Sort, UploadDate};

/// The `params` value for these filters. `None` when all filters are the default.
pub fn encode(filters: &SearchFilters) -> Option<String> {
    if filters.is_default() {
        return None;
    }
    let mut inner = Writer::default();
    if let Some(d) = filters.upload_date {
        inner.varint(1, upload_date(d));
    }
    if let Some(t) = filters.result_type {
        inner.varint(2, result_type(t));
    }
    if let Some(d) = filters.duration {
        inner.varint(3, duration(d));
    }
    let mut msg = Writer::default();
    // The server also writes relevance as `1: 0`. Leaving it out gives the same results.
    if filters.sort == Sort::ViewCount {
        msg.varint(1, 3);
    }
    if !inner.buf.is_empty() {
        msg.bytes(2, &inner.buf);
    }
    Some(STANDARD.encode(msg.buf))
}

fn upload_date(d: UploadDate) -> u64 {
    match d {
        UploadDate::LastHour => 1,
        UploadDate::Today => 2,
        UploadDate::ThisWeek => 3,
        UploadDate::ThisMonth => 4,
        UploadDate::ThisYear => 5,
    }
}

fn result_type(t: ResultType) -> u64 {
    match t {
        ResultType::Video => 1,
        ResultType::Channel => 2,
        ResultType::Playlist => 3,
    }
}

fn duration(d: Duration) -> u64 {
    match d {
        Duration::Under3Min => 4,
        Duration::From3To20Min => 5,
        Duration::Over20Min => 2,
    }
}

/// Writes protobuf fields. The caller writes them in ascending field order.
#[derive(Default)]
struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    const VARINT: u64 = 0;
    const LEN: u64 = 2;

    fn varint(&mut self, field: u64, value: u64) {
        self.raw_varint((field << 3) | Self::VARINT);
        self.raw_varint(value);
    }

    fn bytes(&mut self, field: u64, value: &[u8]) {
        self.raw_varint((field << 3) | Self::LEN);
        self.raw_varint(value.len() as u64);
        self.buf.extend_from_slice(value);
    }

    fn raw_varint(&mut self, mut v: u64) {
        while v >= 0x80 {
            // Truncation is intended: the low 7 bits plus the "more" bit.
            #[allow(clippy::cast_possible_truncation)]
            self.buf.push((v as u8 & 0x7f) | 0x80);
            v >>= 7;
        }
        #[allow(clippy::cast_possible_truncation)]
        self.buf.push(v as u8);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(
        sort: Sort,
        upload_date: Option<UploadDate>,
        result_type: Option<ResultType>,
        duration: Option<Duration>,
    ) -> SearchFilters {
        SearchFilters { sort, upload_date, result_type, duration }
    }

    fn enc(filters: SearchFilters) -> String {
        encode(&filters).unwrap()
    }

    /// Values copied from responses are URL-encoded.
    fn url_decode(s: &str) -> String {
        s.replace("%3D", "=")
    }

    #[test]
    fn default_filters_give_none() {
        assert_eq!(encode(&SearchFilters::default()), None);
    }

    #[test]
    fn single_values() {
        use Sort::Relevance as R;
        let cases = [
            (f(R, None, Some(ResultType::Video), None), "EgIQAQ=="),
            (f(R, None, Some(ResultType::Channel), None), "EgIQAg=="),
            (f(R, None, Some(ResultType::Playlist), None), "EgIQAw=="),
            (f(R, Some(UploadDate::LastHour), None, None), "EgIIAQ=="),
            (f(R, Some(UploadDate::Today), None, None), "EgIIAg=="),
            (f(R, Some(UploadDate::ThisWeek), None, None), "EgIIAw=="),
            (f(R, Some(UploadDate::ThisMonth), None, None), "EgIIBA=="),
            (f(R, Some(UploadDate::ThisYear), None, None), "EgIIBQ=="),
            (f(R, None, None, Some(Duration::Under3Min)), "EgIYBA=="),
            (f(R, None, None, Some(Duration::From3To20Min)), "EgIYBQ=="),
            (f(R, None, None, Some(Duration::Over20Min)), "EgIYAg=="),
            (f(Sort::ViewCount, None, None, None), "CAM="),
        ];
        for (filters, want) in cases {
            assert_eq!(enc(filters), want, "{filters:?}");
        }
    }

    #[test]
    fn combinations() {
        let all = f(Sort::ViewCount, Some(UploadDate::ThisWeek), Some(ResultType::Video), Some(Duration::Over20Min));
        assert_eq!(enc(all), "CAMSBggDEAEYAg==");
        let year_playlist = f(Sort::Relevance, Some(UploadDate::ThisYear), Some(ResultType::Playlist), None);
        assert_eq!(enc(year_playlist), "EgQIBRAD");
    }

    #[test]
    fn matches_server_built_params() {
        use Sort::ViewCount as V;
        let week = Some(UploadDate::ThisWeek);
        let video = Some(ResultType::Video);
        let over20 = Some(Duration::Over20Min);
        let cases = [
            (f(V, week, None, over20), "CAMSBAgDGAI%3D"),
            (f(V, None, video, over20), "CAMSBBABGAI%3D"),
            (f(V, week, video, None), "CAMSBAgDEAE%3D"),
            (f(V, Some(UploadDate::Today), video, over20), "CAMSBggCEAEYAg%3D%3D"),
            (f(V, week, video, Some(Duration::Under3Min)), "CAMSBggDEAEYBA%3D%3D"),
            (f(V, week, video, Some(Duration::From3To20Min)), "CAMSBggDEAEYBQ%3D%3D"),
            (f(V, Some(UploadDate::ThisMonth), video, over20), "CAMSBggEEAEYAg%3D%3D"),
            (f(V, Some(UploadDate::ThisYear), video, over20), "CAMSBggFEAEYAg%3D%3D"),
        ];
        for (filters, want) in cases {
            assert_eq!(enc(filters), url_decode(want), "{filters:?}");
        }
    }

    #[test]
    fn relevance_is_left_out() {
        // The server writes relevance as `1: 0` ("CAASBggDEAEYAg=="). encode leaves it out,
        // so the result is the same message without the first two bytes.
        let filters =
            f(Sort::Relevance, Some(UploadDate::ThisWeek), Some(ResultType::Video), Some(Duration::Over20Min));
        let server = STANDARD.decode(url_decode("CAASBggDEAEYAg%3D%3D")).unwrap();
        assert_eq!(server[..2], [0x08, 0x00]);
        assert_eq!(STANDARD.decode(enc(filters)).unwrap(), server[2..]);
    }

    #[test]
    fn writer_handles_two_byte_keys() {
        // Location (field 23) from the server-built table.
        let mut inner = Writer::default();
        inner.varint(1, 3);
        inner.varint(2, 1);
        inner.varint(3, 2);
        inner.varint(23, 1);
        let mut msg = Writer::default();
        msg.varint(1, 3);
        msg.bytes(2, &inner.buf);
        assert_eq!(STANDARD.encode(&msg.buf), url_decode("CAMSCQgDEAEYArgBAQ%3D%3D"));
    }

    #[test]
    fn varint_bytes() {
        let mut w = Writer::default();
        w.raw_varint(0);
        w.raw_varint(127);
        w.raw_varint(128);
        w.raw_varint(300);
        assert_eq!(w.buf, [0x00, 0x7f, 0x80, 0x01, 0xac, 0x02]);
    }
}
