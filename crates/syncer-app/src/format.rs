//! Small, pure formatting/parsing helpers extracted out of the UI layer so
//! they can be unit-tested without a display (per AGENTS.md Testing Policy).

/// Formats a byte count as a human-readable string using binary (1024-based)
/// units, e.g. `1536` -> `"1.5 KiB"`. Values below 1024 are shown as a plain
/// `"N B"`.
pub fn format_bytes(bytes: i64) -> String {
    const UNITS: [&str; 6] = ["KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];

    if bytes < 0 {
        return format!("-{}", format_bytes(-bytes));
    }
    if bytes < 1024 {
        return format!("{bytes} B");
    }

    let mut value = bytes as f64 / 1024.0;
    let mut unit = UNITS[0];
    for candidate in &UNITS[1..] {
        if value < 1024.0 {
            break;
        }
        value /= 1024.0;
        unit = candidate;
    }

    if value < 10.0 {
        format!("{value:.2} {unit}")
    } else if value < 100.0 {
        format!("{value:.1} {unit}")
    } else {
        format!("{value:.0} {unit}")
    }
}

/// Formats a quota usage/total pair as `"used / total (pct%)"`, or a
/// best-effort partial string when one side is unknown.
pub fn format_quota(used: Option<i64>, total: Option<i64>) -> String {
    match (used, total) {
        (Some(used), Some(total)) if total > 0 => {
            let pct = (used as f64 / total as f64) * 100.0;
            format!(
                "{} / {} ({pct:.1}%)",
                format_bytes(used),
                format_bytes(total)
            )
        }
        (Some(used), _) => format!("{} used", format_bytes(used)),
        (None, Some(total)) => format!("unknown used / {} total", format_bytes(total)),
        (None, None) => "unknown".to_string(),
    }
}

/// Parses a comma-separated extension list into a normalized `Vec<String>`:
/// trims whitespace, strips any leading dot, lowercases, and drops empty
/// entries. Used by the Settings tab's allowed-extensions text field.
pub fn parse_extensions(input: &str) -> Vec<String> {
    input
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.trim_start_matches('.').to_ascii_lowercase())
        .collect()
}

/// Serializes an extension list back into the comma-separated form the
/// Settings tab text field displays.
pub fn format_extensions(extensions: &[String]) -> String {
    extensions.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_bytes_small_values() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1023), "1023 B");
    }

    #[test]
    fn format_bytes_kib_mib_gib() {
        assert_eq!(format_bytes(1024), "1.00 KiB");
        assert_eq!(format_bytes(1536), "1.50 KiB");
        assert_eq!(format_bytes(1024 * 1024), "1.00 MiB");
        assert_eq!(format_bytes(5 * 1024 * 1024 * 1024), "5.00 GiB");
    }

    #[test]
    fn format_bytes_negative() {
        assert_eq!(format_bytes(-1024), "-1.00 KiB");
    }

    #[test]
    fn format_bytes_scales_precision_by_magnitude() {
        // < 10 in unit: 2 decimals; < 100: 1 decimal; >= 100: 0 decimals.
        assert_eq!(format_bytes(9 * 1024), "9.00 KiB");
        assert_eq!(format_bytes(50 * 1024), "50.0 KiB");
        assert_eq!(format_bytes(500 * 1024), "500 KiB");
    }

    #[test]
    fn format_quota_used_and_total() {
        let s = format_quota(Some(1024 * 1024 * 1024), Some(10 * 1024 * 1024 * 1024));
        assert_eq!(s, "1.00 GiB / 10.0 GiB (10.0%)");
    }

    #[test]
    fn format_quota_missing_total() {
        assert_eq!(format_quota(Some(1024), None), "1.00 KiB used");
    }

    #[test]
    fn format_quota_missing_both() {
        assert_eq!(format_quota(None, None), "unknown");
    }

    #[test]
    fn parse_extensions_basic() {
        assert_eq!(
            parse_extensions("png, jpg,  .jpeg ,WEBP"),
            vec!["png", "jpg", "jpeg", "webp"]
        );
    }

    #[test]
    fn parse_extensions_drops_empty_entries() {
        assert_eq!(parse_extensions("png,,jpg,"), vec!["png", "jpg"]);
        assert_eq!(parse_extensions(""), Vec::<String>::new());
    }

    #[test]
    fn extensions_round_trip() {
        let original = vec!["png".to_string(), "jpg".to_string(), "webp".to_string()];
        let text = format_extensions(&original);
        assert_eq!(text, "png, jpg, webp");
        assert_eq!(parse_extensions(&text), original);
    }
}
