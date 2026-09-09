use super::{ConfigError, MAX_OUTPUT_BYTES, error, limit};
use serde_json::Value;

pub(crate) fn encode(value: &Value) -> Result<Vec<u8>, ConfigError> {
    let mut output = Vec::new();
    write(value, &mut output)?;
    Ok(output)
}
fn put(out: &mut Vec<u8>, bytes: &[u8]) -> Result<(), ConfigError> {
    if bytes.len() > MAX_OUTPUT_BYTES.saturating_sub(out.len()) {
        return Err(limit());
    }
    out.extend_from_slice(bytes);
    Ok(())
}
fn string(value: &str, out: &mut Vec<u8>) -> Result<(), ConfigError> {
    put(out, b"\"")?;
    for byte in value.bytes() {
        match byte {
            b'"' => put(out, b"\\\"")?,
            b'\\' => put(out, b"\\\\")?,
            0..=31 => {
                const HEX: &[u8] = b"0123456789abcdef";
                put(
                    out,
                    &[
                        b'\\',
                        b'u',
                        b'0',
                        b'0',
                        HEX[(byte / 16) as usize],
                        HEX[(byte % 16) as usize],
                    ],
                )?;
            }
            _ => put(out, &[byte])?,
        }
    }
    put(out, b"\"")
}
fn write(value: &Value, out: &mut Vec<u8>) -> Result<(), ConfigError> {
    match value {
        Value::Null => put(out, b"null"),
        Value::Bool(v) => put(out, if *v { b"true" } else { b"false" }),
        Value::String(v) => string(v, out),
        Value::Number(v) if v.is_i64() || v.is_u64() => put(out, v.to_string().as_bytes()),
        Value::Number(_) => Err(error("config_invalid_value", "/")),
        Value::Array(values) => {
            put(out, b"[")?;
            for (index, value) in values.iter().enumerate() {
                if index != 0 {
                    put(out, b",")?;
                }
                write(value, out)?;
            }
            put(out, b"]")
        }
        Value::Object(values) => {
            let mut keys: Vec<_> = values.keys().collect();
            keys.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
            put(out, b"{")?;
            for (index, key) in keys.iter().enumerate() {
                if index != 0 {
                    put(out, b",")?;
                }
                string(key, out)?;
                put(out, b":")?;
                write(&values[*key], out)?;
            }
            put(out, b"}")
        }
    }
}

pub(crate) fn bytes_digest(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let digest = aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, bytes);
    let mut out = String::from("sha256:");
    for byte in digest.as_ref() {
        write!(out, "{byte:02x}").expect("writing to String");
    }
    out
}

pub(crate) fn check_inspection_size(value: &impl serde::Serialize) -> Result<(), ConfigError> {
    // Count the actual inspection serialization without allocating a second
    // Value tree or an unbounded output buffer containing operator content.
    struct Counter(usize);
    impl std::io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > MAX_OUTPUT_BYTES.saturating_sub(self.0) {
                return Err(std::io::Error::other("config_limit"));
            }
            self.0 += bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    serde_json::to_writer(Counter(0), value).map_err(|_| limit())
}

/// Fingerprint a bounded JSON value using the frozen config encoding. This is
/// not a credential hashing API; pass only the documented public payloads.
pub fn fingerprint(value: &Value) -> Result<String, ConfigError> {
    // Validate depth/work before the recursive encoder, including host values.
    super::input::check_value(value, &mut 0, 1)?;
    Ok(bytes_digest(&encode(value)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn config_and_inspection_encoding_stop_at_the_output_bound() {
        let mut value = Value::String("x".repeat(MAX_OUTPUT_BYTES - 2));
        assert_eq!(encode(&value).unwrap().len(), MAX_OUTPUT_BYTES);
        check_inspection_size(&value).unwrap();
        if let Value::String(text) = &mut value {
            text.push('x');
        }
        assert_eq!(encode(&value).unwrap_err().code, "config_limit");
        assert_eq!(
            check_inspection_size(&value).unwrap_err().code,
            "config_limit"
        );
    }
}
