//! Best-effort parser for VRChat / VRCX screenshot metadata embedded in PNG
//! text chunks (`tEXt` / `iTXt`).
//!
//! Two known producers embed JSON payloads in screenshot PNGs:
//!
//! - **VRChat native** (`--enable-screenshot-metadata` launch option): embeds
//!   a JSON object, commonly under a text-chunk keyword like `"Description"`,
//!   with top-level fields such as `application`, `version`, `world`
//!   (`{ name, id, ... }`), `players` (`[{ displayName, id }, ...]`),
//!   `author`, `pos`.
//! - **VRCX (Screenshot Helper)**: a similarly-shaped JSON blob, historically
//!   embedded under a different text-chunk keyword (VRCX has used
//!   `"Description"` as well as tool-specific keywords across versions) and
//!   sometimes a slightly different JSON shape (e.g. `world` as a plain name
//!   string rather than an object, or a `players` array of plain name
//!   strings rather than objects).
//!
//! Exact field names are not publicly documented in a single stable spec, so
//! this parser is intentionally lenient: it tries several known shapes in
//! turn and tolerates missing fields (`#[serde(default)]` throughout) so
//! partial data still produces a usable description. Any parse failure
//! returns `None` rather than propagating an error — per the plan, upload
//! must proceed without a description rather than fail outright.

use std::path::Path;

use serde::Deserialize;

/// Parsed VRChat/VRCX screenshot metadata.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScreenshotMetadata {
    pub world_name: Option<String>,
    pub player_names: Vec<String>,
}

impl ScreenshotMetadata {
    fn is_empty(&self) -> bool {
        self.world_name.is_none() && self.player_names.is_empty()
    }
}

/// VRChat-native shape: `world` is an object, `players` is a list of objects
/// with a `displayName` field (VRCX-exported native metadata uses the same
/// shape for `displayName`; some tools use `name` instead, so both are
/// tried).
#[derive(Debug, Default, Deserialize)]
struct NativeShape {
    #[serde(default)]
    world: Option<NativeWorld>,
    #[serde(default)]
    players: Vec<NativePlayer>,
}

#[derive(Debug, Default, Deserialize)]
struct NativeWorld {
    #[serde(default)]
    name: Option<String>,
    /// Present in real VRChat metadata but not currently surfaced in
    /// [`ScreenshotMetadata`]; kept for documentation / forward-compat.
    #[serde(default)]
    #[allow(dead_code)]
    id: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct NativePlayer {
    #[serde(default)]
    #[serde(alias = "displayName")]
    #[serde(alias = "name")]
    display_name: Option<String>,
    /// See [`NativeWorld::id`].
    #[serde(default)]
    #[allow(dead_code)]
    id: Option<String>,
}

/// VRCX Screenshot Helper shape: flatter, `world` may be a plain string
/// (world name) and `players` a plain list of name strings.
#[derive(Debug, Default, Deserialize)]
struct VrcxShape {
    #[serde(default)]
    world: Option<String>,
    #[serde(default)]
    #[serde(alias = "world_name")]
    world_name_alt: Option<String>,
    #[serde(default)]
    players: Vec<String>,
}

fn parse_native(json: &str) -> Option<ScreenshotMetadata> {
    let shape: NativeShape = serde_json::from_str(json).ok()?;
    let world_name = shape.world.and_then(|w| w.name);
    let player_names: Vec<String> = shape
        .players
        .into_iter()
        .filter_map(|p| p.display_name)
        .filter(|n| !n.is_empty())
        .collect();

    let meta = ScreenshotMetadata {
        world_name,
        player_names,
    };
    if meta.is_empty() {
        None
    } else {
        Some(meta)
    }
}

fn parse_vrcx(json: &str) -> Option<ScreenshotMetadata> {
    let shape: VrcxShape = serde_json::from_str(json).ok()?;
    let world_name = shape.world.or(shape.world_name_alt);
    let player_names: Vec<String> = shape
        .players
        .into_iter()
        .filter(|n| !n.is_empty())
        .collect();

    let meta = ScreenshotMetadata {
        world_name,
        player_names,
    };
    if meta.is_empty() {
        None
    } else {
        Some(meta)
    }
}

/// Tries each known JSON shape in turn against a single text-chunk payload.
fn parse_json_payload(text: &str) -> Option<ScreenshotMetadata> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    parse_native(text).or_else(|| parse_vrcx(text))
}

/// Text-chunk keywords known to carry VRChat/VRCX screenshot metadata.
/// Checked case-insensitively; the PNG spec keyword is typically
/// `"Description"`, but some VRCX versions have used other keywords.
const CANDIDATE_KEYWORDS: &[&str] = &["description", "vrchat", "vrcx", "comment"];

fn keyword_is_candidate(keyword: &str) -> bool {
    let lower = keyword.to_ascii_lowercase();
    CANDIDATE_KEYWORDS.iter().any(|k| lower.contains(k))
}

/// Reads a PNG's `tEXt`/`iTXt` chunks and attempts to parse VRChat/VRCX
/// screenshot metadata from them. Returns `None` on any failure (missing
/// file, invalid PNG, no matching/parseable text chunk) — this is
/// non-fatal by design so upload can proceed without a description.
pub fn parse_png_metadata(path: &Path) -> Option<ScreenshotMetadata> {
    let file = std::fs::File::open(path).ok()?;
    let decoder = png::Decoder::new(file);
    let reader = decoder.read_info().ok()?;
    let info = reader.info();

    // iTXt chunks (UTF-8) — try candidate keywords first, then all others.
    let mut itxt_candidates = Vec::new();
    let mut itxt_others = Vec::new();
    for chunk in &info.utf8_text {
        if keyword_is_candidate(&chunk.keyword) {
            itxt_candidates.push(chunk);
        } else {
            itxt_others.push(chunk);
        }
    }
    for chunk in itxt_candidates.into_iter().chain(itxt_others) {
        if let Ok(text) = chunk.get_text() {
            if let Some(meta) = parse_json_payload(&text) {
                return Some(meta);
            }
        }
    }

    // tEXt chunks (Latin-1) — same strategy.
    let mut text_candidates = Vec::new();
    let mut text_others = Vec::new();
    for chunk in &info.uncompressed_latin1_text {
        if keyword_is_candidate(&chunk.keyword) {
            text_candidates.push(chunk);
        } else {
            text_others.push(chunk);
        }
    }
    for chunk in text_candidates.into_iter().chain(text_others) {
        if let Some(meta) = parse_json_payload(&chunk.text) {
            return Some(meta);
        }
    }

    None
}

/// Formats parsed metadata into a human-readable description string, e.g.
/// `"World: My World | Players: Alice, Bob"`. Omits parts that are missing.
pub fn format_description(meta: &ScreenshotMetadata) -> String {
    let mut parts = Vec::new();
    if let Some(world) = &meta.world_name {
        if !world.is_empty() {
            parts.push(format!("World: {world}"));
        }
    }
    if !meta.player_names.is_empty() {
        parts.push(format!("Players: {}", meta.player_names.join(", ")));
    }
    parts.join(" | ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;

    fn write_png_with_itxt(path: &std::path::Path, keyword: &str, text: &str) {
        let file = File::create(path).unwrap();
        let mut encoder = png::Encoder::new(file, 2, 2);
        encoder.set_color(png::ColorType::Grayscale);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .add_itxt_chunk(keyword.to_string(), text.to_string())
            .unwrap();
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&[0, 0, 0, 0]).unwrap();
    }

    fn write_png_with_text(path: &std::path::Path, keyword: &str, text: &str) {
        let file = File::create(path).unwrap();
        let mut encoder = png::Encoder::new(file, 2, 2);
        encoder.set_color(png::ColorType::Grayscale);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .add_text_chunk(keyword.to_string(), text.to_string())
            .unwrap();
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&[0, 0, 0, 0]).unwrap();
    }

    fn write_png_plain(path: &std::path::Path) {
        let file = File::create(path).unwrap();
        let mut encoder = png::Encoder::new(file, 2, 2);
        encoder.set_color(png::ColorType::Grayscale);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&[0, 0, 0, 0]).unwrap();
    }

    #[test]
    fn parses_vrchat_native_shaped_itxt_payload() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("native.png");
        let json = r#"{
            "application": "VRChat",
            "version": 1,
            "author": { "id": "usr_123", "displayName": "Author" },
            "world": { "name": "My Cool World", "id": "wrld_abc" },
            "players": [
                { "displayName": "Alice", "id": "usr_1" },
                { "displayName": "Bob", "id": "usr_2" }
            ]
        }"#;
        write_png_with_itxt(&path, "Description", json);

        let meta = parse_png_metadata(&path).expect("should parse metadata");
        assert_eq!(meta.world_name.as_deref(), Some("My Cool World"));
        assert_eq!(meta.player_names, vec!["Alice", "Bob"]);

        let desc = format_description(&meta);
        assert_eq!(desc, "World: My Cool World | Players: Alice, Bob");
    }

    #[test]
    fn parses_vrcx_shaped_payload() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vrcx.png");
        let json = r#"{
            "world": "VRCX World Name",
            "players": ["Charlie", "Dana"]
        }"#;
        write_png_with_itxt(&path, "Description", json);

        let meta = parse_png_metadata(&path).expect("should parse metadata");
        assert_eq!(meta.world_name.as_deref(), Some("VRCX World Name"));
        assert_eq!(meta.player_names, vec!["Charlie", "Dana"]);
    }

    #[test]
    fn parses_text_chunk_latin1_payload() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("text_chunk.png");
        let json = r#"{"world":{"name":"Latin World"},"players":[{"displayName":"Eve"}]}"#;
        write_png_with_text(&path, "Description", json);

        let meta = parse_png_metadata(&path).expect("should parse metadata");
        assert_eq!(meta.world_name.as_deref(), Some("Latin World"));
        assert_eq!(meta.player_names, vec!["Eve"]);
    }

    #[test]
    fn png_with_no_metadata_chunk_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plain.png");
        write_png_plain(&path);

        assert!(parse_png_metadata(&path).is_none());
    }

    #[test]
    fn malformed_json_returns_none_without_panicking() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("malformed.png");
        write_png_with_itxt(&path, "Description", "{not valid json at all");

        assert!(parse_png_metadata(&path).is_none());
    }

    #[test]
    fn nonexistent_file_returns_none() {
        let path = Path::new("/does/not/exist/vrc-immich-syncer-test.png");
        assert!(parse_png_metadata(path).is_none());
    }

    #[test]
    fn format_description_handles_partial_data() {
        let meta = ScreenshotMetadata {
            world_name: Some("Only World".to_string()),
            player_names: vec![],
        };
        assert_eq!(format_description(&meta), "World: Only World");

        let meta = ScreenshotMetadata {
            world_name: None,
            player_names: vec!["Solo".to_string()],
        };
        assert_eq!(format_description(&meta), "Players: Solo");
    }
}
