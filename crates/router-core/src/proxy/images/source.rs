use std::fmt;

use axum::body::Bytes;
use serde::{
    Deserialize, Deserializer,
    de::{MapAccess, Visitor},
};
use serde_json::value::RawValue;

use super::{
    asset::{ImageAssetErrorKind, MAX_BASE64_BYTES, MCP_JSON_RESPONSE_LIMIT},
    download::MAX_URL_BYTES,
};

// Neither carrier is safe to format: it may contain image data or a signed URL.
pub(super) enum ImageResultSource {
    Base64(String),
    Url(String),
}

pub(super) fn take_image_source(body: Bytes) -> Result<ImageResultSource, ImageAssetErrorKind> {
    let source = select_image_source(&body);
    drop(body);
    source
}

fn select_image_source(body: &[u8]) -> Result<ImageResultSource, ImageAssetErrorKind> {
    if body.len() > MCP_JSON_RESPONSE_LIMIT {
        return Err(ImageAssetErrorKind::TooLarge);
    }
    // Borrow raw values instead of building a Value tree: a dense array in an
    // unselected field must not multiply the response's allocation budget.
    let response: &RawValue =
        serde_json::from_slice(body).map_err(|_| ImageAssetErrorKind::InvalidResponse)?;
    validate_unicode_escapes(response.get())?;
    let fields = read_fields(response)?;
    let data = fields
        .data
        .filter(|value| value.get().starts_with('['))
        .ok_or(ImageAssetErrorKind::MissingResult)?;
    let selected = select_data(data.get())?;
    if let Some(encoded) = selected.base64 {
        return expand_base64_string(encoded.get()).map(ImageResultSource::Base64);
    }
    let url = selected.url.ok_or(ImageAssetErrorKind::MissingResult)?;
    // A JSON escape uses at most six source bytes for each decoded byte.
    // Reject huge URL tokens before the string decoder allocates its scratch.
    if url.get().len() > MAX_URL_BYTES * 6 + 2 {
        return Err(ImageAssetErrorKind::InvalidUrl);
    }
    let url: String =
        serde_json::from_str(url.get()).map_err(|_| ImageAssetErrorKind::InvalidResponse)?;
    if url.len() > MAX_URL_BYTES {
        return Err(ImageAssetErrorKind::InvalidUrl);
    }
    Ok(ImageResultSource::Url(url))
}

#[derive(Default)]
struct RawFields<'a> {
    data: Option<&'a RawValue>,
    base64: Option<&'a RawValue>,
    url: Option<&'a RawValue>,
}

fn read_fields(value: &RawValue) -> Result<RawFields<'_>, ImageAssetErrorKind> {
    if !value.get().starts_with('{') {
        return Ok(RawFields::default());
    }
    serde_json::from_str(value.get()).map_err(|_| ImageAssetErrorKind::InvalidResponse)
}

impl<'de> Deserialize<'de> for RawFields<'de> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct FieldsVisitor;
        impl<'de> Visitor<'de> for FieldsVisitor {
            type Value = RawFields<'de>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an image response object")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut fields = RawFields::default();
                while let Some(key) = map.next_key::<&RawValue>()? {
                    let value = map.next_value::<&RawValue>()?;
                    // Borrow unknown keys too; even an escaped, oversized key
                    // must not allocate a second response-sized string.
                    if key.get().len() > "b64_json".len() * 6 + 2 {
                        continue;
                    }
                    let key: String =
                        serde_json::from_str(key.get()).map_err(serde::de::Error::custom)?;
                    match key.as_str() {
                        "data" => fields.data = Some(value),
                        "b64_json" => fields.base64 = Some(value),
                        "url" => fields.url = Some(value),
                        _ => {}
                    }
                }
                Ok(fields)
            }
        }
        deserializer.deserialize_map(FieldsVisitor)
    }
}

#[derive(Default)]
struct RawSelection<'a> {
    base64: Option<&'a RawValue>,
    url: Option<&'a RawValue>,
}

fn select_data(data: &str) -> Result<RawSelection<'_>, ImageAssetErrorKind> {
    let mut selected = RawSelection::default();
    let mut remaining = data[1..data.len() - 1].trim_start();
    while !remaining.is_empty() {
        let (item, consumed) = {
            let mut stream = serde_json::Deserializer::from_str(remaining).into_iter::<&RawValue>();
            let item = stream
                .next()
                .ok_or(ImageAssetErrorKind::InvalidResponse)?
                .map_err(|_| ImageAssetErrorKind::InvalidResponse)?;
            (item, stream.byte_offset())
        };
        // Drop each parser's nesting scratch before inspecting the borrowed
        // object, so two large parser stacks never coexist with the JSON body.
        let fields = read_fields(item)?;
        selected.base64 = fields.base64.filter(|value| value.get().starts_with('"'));
        if selected.base64.is_some() {
            selected.url = None;
            return Ok(selected);
        }
        if selected.url.is_none() {
            selected.url = fields.url.filter(|value| value.get().starts_with('"'));
        }
        remaining = remaining[consumed..].trim_start();
        if !remaining.is_empty() {
            remaining = remaining
                .strip_prefix(',')
                .ok_or(ImageAssetErrorKind::InvalidResponse)?
                .trim_start();
        }
    }
    Ok(selected)
}

fn expand_base64_string(raw: &str) -> Result<String, ImageAssetErrorKind> {
    let content = &raw.as_bytes()[1..raw.len() - 1];
    let mut encoded = String::new();
    encoded
        .try_reserve_exact(content.len().min(MAX_BASE64_BYTES))
        .map_err(|_| ImageAssetErrorKind::TooLarge)?;
    let mut index = 0;
    while index < content.len() {
        let byte = if content[index] == b'\\' {
            let (byte, consumed) = base64_escape(&content[index..])?;
            index += consumed;
            byte
        } else {
            let byte = content[index];
            index += 1;
            byte
        };
        // JSON syntax was already validated. Only the ASCII Base64 alphabet
        // can become a canonical value, so expanding its two permitted JSON
        // escape forms needs no response-sized Serde scratch/copy pair.
        if !byte.is_ascii_alphanumeric() && !matches!(byte, b'+' | b'/' | b'=') {
            return Err(ImageAssetErrorKind::InvalidBase64);
        }
        if encoded.len() == MAX_BASE64_BYTES {
            return Err(ImageAssetErrorKind::TooLarge);
        }
        encoded.push(char::from(byte));
    }
    Ok(encoded)
}

fn base64_escape(raw: &[u8]) -> Result<(u8, usize), ImageAssetErrorKind> {
    match raw.get(1) {
        Some(b'/') => Ok((b'/', 2)),
        Some(b'u') => {
            let scalar = escaped_code_unit(raw)?;
            let byte = u8::try_from(scalar).map_err(|_| ImageAssetErrorKind::InvalidBase64)?;
            Ok((byte, 6))
        }
        _ => Err(ImageAssetErrorKind::InvalidBase64),
    }
}

fn escaped_code_unit(raw: &[u8]) -> Result<u16, ImageAssetErrorKind> {
    let digits = raw
        .get(2..6)
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
        .ok_or(ImageAssetErrorKind::InvalidResponse)?;
    u16::from_str_radix(digits, 16).map_err(|_| ImageAssetErrorKind::InvalidResponse)
}

fn validate_unicode_escapes(raw: &str) -> Result<(), ImageAssetErrorKind> {
    // RawValue checks the complete JSON grammar without materializing strings,
    // but permits lone surrogate escapes. Preserve the former string parser's
    // Unicode rejection with a constant-space pass, including ignored fields.
    let mut remaining = raw.as_bytes();
    while let Some(index) = remaining.iter().position(|&byte| byte == b'\\') {
        remaining = &remaining[index..];
        if remaining.get(1) != Some(&b'u') {
            remaining = &remaining[2..];
            continue;
        }
        let scalar = escaped_code_unit(remaining)?;
        remaining = &remaining[6..];
        if (0xd800..=0xdbff).contains(&scalar) {
            if !remaining.starts_with(b"\\u")
                || !(0xdc00..=0xdfff).contains(&escaped_code_unit(remaining)?)
            {
                return Err(ImageAssetErrorKind::InvalidResponse);
            }
            remaining = &remaining[6..];
        } else if (0xdc00..=0xdfff).contains(&scalar) {
            return Err(ImageAssetErrorKind::InvalidResponse);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_base64(response: &str, expected: &str) {
        let result = take_image_source(Bytes::copy_from_slice(response.as_bytes()));
        let Ok(ImageResultSource::Base64(encoded)) = result else {
            panic!("expected a Base64 carrier");
        };
        assert_eq!(encoded, expected);
    }

    #[test]
    fn first_string_base64_precedes_every_url_including_empty_base64() {
        assert_base64(
            r#"{"data":[{"url":"https://assets.example/first.png"},{"b64_json":null},{"b64_json":42},{"b64_json":"AQ=="},{"b64_json":"Ag=="}]}"#,
            "AQ==",
        );
        assert_base64(
            r#"{"data":[{"b64_json":"","url":"https://assets.example/image.png"}]}"#,
            "",
        );
        assert_eq!(take_image_source(Bytes::from_static(br#"{"data":[{"b64_json":"invalid value","url":"https://assets.example/image.png"}]}"#)).err(), Some(ImageAssetErrorKind::InvalidBase64));
    }

    #[test]
    fn first_url_string_wins_only_without_any_base64_string() {
        for first in ["", "https://assets.example/image.png?signature=synthetic"] {
            let response = serde_json::json!({"data":[
                {"b64_json":null,"url":false}, {"b64_json":42,"url":first},
                {"url":"https://assets.example/ignored.png"}
            ]});
            let Ok(ImageResultSource::Url(url)) =
                take_image_source(Bytes::from(response.to_string()))
            else {
                panic!("expected a URL carrier");
            };
            assert_eq!(url, first);
        }
    }

    #[test]
    fn escaped_keys_values_and_duplicate_fields_preserve_selection() {
        assert_base64(
            r#"{"\u0064ata":[{"b64_json":"Ag==","b64_json":"\u0041Q\u003d\u003d"}]}"#,
            "AQ==",
        );
        assert_base64(r#"{"data":[],"data":[{"b64_json":"\/w=="}]}"#, "/w==");
        assert_eq!(
            take_image_source(Bytes::from_static(br#"{"data":[{"b64_json":"AQ==\n"}]}"#)).err(),
            Some(ImageAssetErrorKind::InvalidBase64)
        );
        assert_eq!(
            take_image_source(Bytes::from_static(br#"{"data":[{"b64_json":"\u754c"}]}"#)).err(),
            Some(ImageAssetErrorKind::InvalidBase64)
        );
    }

    #[test]
    fn missing_carriers_and_malformed_json_stay_distinct() {
        for response in [
            "null",
            "[]",
            "{}",
            r#"{"data":null}"#,
            r#"{"data":[null,{},3]}"#,
        ] {
            assert_eq!(
                take_image_source(Bytes::copy_from_slice(response.as_bytes())).err(),
                Some(ImageAssetErrorKind::MissingResult)
            );
        }
        for response in [
            r#"{"data":["#,
            r#"{"data":[{"b64_json":"AQ=="}]} trailing"#,
            r#"{"data":[{"url":"\q"}]}"#,
            r#"{"data":[{"b64_json":"\ud800"}]}"#,
            r#"{"extra":"\udc00","data":[{"b64_json":"AQ=="}]}"#,
            r#"{"extra":"\ud800\u0041","data":[{"b64_json":"AQ=="}]}"#,
        ] {
            assert_eq!(
                take_image_source(Bytes::copy_from_slice(response.as_bytes())).err(),
                Some(ImageAssetErrorKind::InvalidResponse)
            );
        }
        assert_base64(
            r#"{"extra":"\ud83d\ude00\\ud800","data":[{"b64_json":"AQ=="}]}"#,
            "AQ==",
        );
    }

    #[test]
    fn dense_unselected_values_and_oversized_escaped_urls_are_bounded() {
        let padding = "[null,true,0,{}],".repeat(16_384);
        assert_base64(
            &format!("{{\"extra\":[{padding}null],\"data\":[{{\"b64_json\":\"AQ==\"}}]}}"),
            "AQ==",
        );
        let response = format!(
            "{{\"data\":[{{\"url\":\"{}\"}}]}}",
            "\\u0041".repeat(MAX_URL_BYTES + 1)
        );
        assert_eq!(
            take_image_source(Bytes::from(response)).err(),
            Some(ImageAssetErrorKind::InvalidUrl)
        );
    }
}
