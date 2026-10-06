//! UTF-16 editor positions and percent-encoded local file URIs.

use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use url::Url;

pub type Result<T> = std::result::Result<T, String>;

pub fn path(uri: &str) -> Result<PathBuf> {
    Url::parse(uri)
        .map_err(|e| e.to_string())?
        .to_file_path()
        .map_err(|_| "only local file:// documents are supported".into())
}

pub fn uri(path: &Path) -> Result<String> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(path)
    };
    Url::from_file_path(path)
        .map(|url| url.to_string())
        .map_err(|_| "invalid file path".into())
}

pub fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing string `{key}`"))
}

pub fn offset(text: &str, position: &Value) -> Result<usize> {
    let line = position
        .get("line")
        .and_then(Value::as_u64)
        .ok_or("invalid line")?;
    let character = position
        .get("character")
        .and_then(Value::as_u64)
        .ok_or("invalid character")?;
    let mut start = 0;
    for _ in 0..line {
        start += text[start..].find('\n').ok_or("line outside document")? + 1;
    }
    let rest = &text[start..];
    let end = rest.find('\n').unwrap_or(rest.len());
    let line = rest[..end].strip_suffix('\r').unwrap_or(&rest[..end]);
    let mut units = 0;
    for (byte, ch) in line.char_indices() {
        if units == character {
            return Ok(start + byte);
        }
        units += ch.len_utf16() as u64;
        if units > character {
            return Err("position splits a UTF-16 surrogate pair".into());
        }
    }
    // LSP positions beyond the last character clamp to the end of this line.
    Ok(start + line.len())
}

pub fn position(text: &str, byte: usize) -> Value {
    let mut byte = byte.min(text.len());
    while !text.is_char_boundary(byte) {
        byte -= 1;
    }
    let prefix = &text[..byte];
    let line = prefix.bytes().filter(|&b| b == b'\n').count();
    let tail = prefix
        .rsplit('\n')
        .next()
        .unwrap_or("")
        .trim_end_matches('\r');
    json!({"line": line, "character": tail.encode_utf16().count()})
}

pub fn range(text: &str, start: u32, end: u32) -> Value {
    json!({"start": position(text, start as usize), "end": position(text, end as usize)})
}

/// Apply a notification atomically, each range against the prior edit.
pub fn changed(text: &str, changes: &Value) -> Result<String> {
    let changes = changes
        .as_array()
        .ok_or("contentChanges must be an array")?;
    let mut result = text.to_string();
    for change in changes {
        let replacement = string(change, "text")?;
        if let Some(range) = change.get("range") {
            let start = offset(&result, &range["start"])?;
            let end = offset(&result, &range["end"])?;
            if start > end {
                return Err("reversed edit range".into());
            }
            result.replace_range(start..end, replacement);
        } else {
            result = replacement.to_string();
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_crlf_and_trailing_line() {
        let text = "a😀é\r\nx\n";
        assert_eq!(position(text, 7), json!({"line":0,"character":4}));
        assert_eq!(offset(text, &json!({"line":0,"character":3})).unwrap(), 5);
        assert!(offset(text, &json!({"line":0,"character":2})).is_err());
        assert_eq!(
            offset(text, &json!({"line":2,"character":0})).unwrap(),
            text.len()
        );
        assert!(offset(text, &json!({"line":3,"character":0})).is_err());
        assert_eq!(
            offset(text, &json!({"line":1,"character":999})).unwrap(),
            text.len() - 1
        );
    }

    #[test]
    fn sequential_edits_and_uri_roundtrip() {
        let edits = json!([
            {"range":{"start":{"line":0,"character":1},"end":{"line":0,"character":3}},"text":"b"},
            {"range":{"start":{"line":0,"character":2},"end":{"line":0,"character":3}},"text":"d"}
        ]);
        assert_eq!(changed("a😀c", &edits).unwrap(), "abd");
        let file = Path::new("/tmp/a b#😀.siox");
        assert_eq!(path(&uri(file).unwrap()).unwrap(), file);
        assert!(path("https://example.com/file.siox").is_err());
    }
}
