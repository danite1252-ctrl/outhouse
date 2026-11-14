//! Time tags: `<t:UNIX>` and `<t:UNIX:STYLE>` (Discord's syntax). The web
//! client shows them in each reader's own time zone (`web/timetags.js`);
//! the server writes them into bot messages instead of fixed-zone times,
//! and spells them out in UTC where it builds the text itself (push
//! notifications).

use chrono::{DateTime, TimeZone, Utc};

/// A tag for `at`: `style` is one of t T d D f F R.
pub fn tag(at: DateTime<Utc>, style: char) -> String {
    format!("<t:{}:{style}>", at.timestamp())
}

/// One tag spelled out in UTC (relative ones get the date and time, since
/// a notification would go stale).
fn utc_text(unix: i64, style: char) -> Option<String> {
    let at = Utc.timestamp_opt(unix, 0).single()?;
    Some(match style {
        't' => at.format("%H:%M UTC").to_string(),
        'T' => at.format("%H:%M:%S UTC").to_string(),
        'd' => at.format("%d/%m/%Y").to_string(),
        'D' => at.format("%-d %B %Y").to_string(),
        'F' => at.format("%A, %-d %B %Y %H:%M UTC").to_string(),
        _ => at.format("%-d %B %Y %H:%M UTC").to_string(),
    })
}

/// Cuts off a time tag left unfinished at the end of shortened text (e.g.
/// "raid at <t:17672" after trimming a notification), so readers never see
/// half a tag.
pub fn drop_partial_tag(text: &str) -> &str {
    match text.rfind("<t:") {
        Some(i) if !text[i..].contains('>') => text[..i].trim_end(),
        _ => text,
    }
}

/// Text with every time tag spelled out in UTC; anything that isn't a
/// valid tag stays as it was.
#[allow(dead_code)] // for plain-text places that can't run code (e.g. emails)
pub fn plain(text: &str) -> String {
    if !text.contains("<t:") {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<t:") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 3..];
        if let Some((replaced, used)) = parse(after) {
            out.push_str(&replaced);
            rest = &after[used..];
        } else {
            out.push_str("<t:");
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

/// `after` follows "<t:". Returns the spelled-out time and how many bytes
/// of `after` the tag used (up to and including '>').
fn parse(after: &str) -> Option<(String, usize)> {
    let end = after.find('>')?;
    if end > 16 {
        return None;
    }
    let inner = &after[..end];
    let (num, style) = match inner.split_once(':') {
        Some((n, s)) if s.len() == 1 && "tTdDfFR".contains(s) => (n, s.chars().next()?),
        Some(_) => return None,
        None => (inner, 'f'),
    };
    let digits = num.strip_prefix('-').unwrap_or(num);
    if digits.is_empty() || digits.len() > 13 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let unix: i64 = num.parse().ok()?;
    Some((utc_text(unix, style)?, end + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_half_tags() {
        assert_eq!(drop_partial_tag("raid at <t:17672"), "raid at");
        assert_eq!(
            drop_partial_tag("raid at <t:1767259800:F> ok"),
            "raid at <t:1767259800:F> ok"
        );
        assert_eq!(drop_partial_tag("no tags"), "no tags");
    }

    #[test]
    fn makes_tags() {
        let at = Utc.with_ymd_and_hms(2026, 1, 1, 9, 30, 0).unwrap();
        assert_eq!(tag(at, 'F'), "<t:1767259800:F>");
    }

    #[test]
    fn spells_tags_out_in_utc() {
        assert_eq!(plain("raid <t:1767259800:t> ok"), "raid 09:30 UTC ok");
        assert_eq!(plain("<t:1767259800>"), "1 January 2026 09:30 UTC");
        assert_eq!(
            plain("<t:1767259800:F>"),
            "Thursday, 1 January 2026 09:30 UTC"
        );
        assert_eq!(plain("<t:1767259800:R>"), "1 January 2026 09:30 UTC");
        assert_eq!(
            plain("<t:1767259800:d> <t:1767259800:D>"),
            "01/01/2026 1 January 2026"
        );
        assert_eq!(plain("<t:1767259800:T>"), "09:30:00 UTC");
    }

    #[test]
    fn leaves_everything_else_alone() {
        assert_eq!(plain("no tags"), "no tags");
        assert_eq!(
            plain("<t:abc> <t:12:x> <t:> <t:1"),
            "<t:abc> <t:12:x> <t:> <t:1"
        );
        assert_eq!(plain("<t:99999999999999:f>"), "<t:99999999999999:f>");
        assert_eq!(plain("a <t:<t:1767259800:t>"), "a <t:09:30 UTC");
    }
}
