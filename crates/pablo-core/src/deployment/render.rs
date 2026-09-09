//! A bounded encoder for the closed TOML subset accepted by deployment v1.
//! Inline tables avoid implicit table/array context and preserve list order.
use super::{ConfigError, MAX_FILE_BYTES, error, limit};
use serde_json::Value;

pub(super) fn render(config: &Value) -> Result<String, ConfigError> {
    let mut writer = Writer(String::new());
    writer.put("schema_version = 1\n")?;
    for key in ["authority", "credentials", "deployment", "options"] {
        writer.string(key)?;
        writer.put(" = ")?;
        writer.value(&config[key])?;
        writer.put("\n")?;
    }
    Ok(writer.0)
}

struct Writer(String);
impl Writer {
    fn put(&mut self, text: &str) -> Result<(), ConfigError> {
        // A rendered entry must be reloadable under the single-file read bound.
        if text.len() > MAX_FILE_BYTES.saturating_sub(self.0.len()) {
            return Err(limit());
        }
        self.0.push_str(text);
        Ok(())
    }
    fn string(&mut self, text: &str) -> Result<(), ConfigError> {
        self.put("\"")?;
        for c in text.chars() {
            match c {
                '"' => self.put("\\\"")?,
                '\\' => self.put("\\\\")?,
                '\u{0}'..='\u{1f}' | '\u{7f}' => self.put(&format!("\\u{:04x}", c as u32))?,
                _ => self.put(c.encode_utf8(&mut [0; 4]))?,
            }
        }
        self.put("\"")
    }
    fn value(&mut self, value: &Value) -> Result<(), ConfigError> {
        match value {
            Value::String(s) => self.string(s),
            Value::Bool(b) => self.put(if *b { "true" } else { "false" }),
            Value::Number(n) if n.is_i64() => self.put(&n.to_string()),
            Value::Array(values) => {
                self.put("[")?;
                for (index, value) in values.iter().enumerate() {
                    if index != 0 {
                        self.put(", ")?;
                    }
                    self.value(value)?;
                }
                self.put("]")
            }
            Value::Object(values) => {
                self.put("{")?;
                let mut keys: Vec<_> = values.keys().collect();
                keys.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
                for (index, key) in keys.iter().enumerate() {
                    if index != 0 {
                        self.put(", ")?;
                    }
                    self.string(key)?;
                    self.put(" = ")?;
                    self.value(&values[*key])?;
                }
                self.put("}")
            }
            _ => Err(error("config_invalid_value", "/")),
        }
    }
}
