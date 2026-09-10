//! OpenRouter-specific accounting and terminal-frame compatibility.
use serde::Deserialize;
use serde_json::{Value, value::RawValue};

use super::{FinishReason, ProviderError, Usage, malformed};

/// Read the original decimal token, never a value rounded through f64.
pub(super) fn accounting(data: &[u8]) -> Result<(Usage, Option<u64>), ProviderError> {
    #[derive(Deserialize)]
    struct Frame<'a> {
        #[serde(borrow)]
        usage: Option<Accounting<'a>>,
    }
    #[derive(Deserialize)]
    struct Accounting<'a> {
        prompt_tokens: Option<u64>,
        completion_tokens: Option<u64>,
        prompt_tokens_details: Option<Details>,
        #[serde(borrow)]
        cost: Option<&'a RawValue>,
    }
    #[derive(Default, Deserialize)]
    struct Details {
        cached_tokens: Option<u64>,
        cache_write_tokens: Option<u64>,
    }
    let frame: Frame<'_> = serde_json::from_slice(data).map_err(|_| malformed())?;
    let usage = frame.usage.ok_or_else(malformed)?;
    let details = usage.prompt_tokens_details.unwrap_or_default();
    Ok((
        Usage {
            input_tokens: usage.prompt_tokens,
            output_tokens: usage.completion_tokens,
            cache_read_input_tokens: details.cached_tokens,
            cache_write_input_tokens: details.cache_write_tokens,
        },
        usage.cost.map(microusd).transpose()?,
    ))
}

fn microusd(raw: &RawValue) -> Result<u64, ProviderError> {
    let text = raw.get();
    if text.len() > 128 || !text.as_bytes().first().is_some_and(u8::is_ascii_digit) {
        return Err(malformed());
    }
    let (mantissa, exponent) = match text.split_once(['e', 'E']) {
        Some((m, e)) => (m, e.parse::<i32>().map_err(|_| malformed())?),
        None => (text, 0),
    };
    let fraction = mantissa.split_once('.').map_or(0, |(_, f)| f.len());
    let digits = mantissa.replace('.', "");
    let digits = digits.trim_start_matches('0');
    if digits.is_empty() {
        return Ok(0);
    }
    let scale = i64::from(exponent) - fraction as i64 + 6;
    let length = digits.len() as i64 + scale;
    if length > 20 {
        return Err(malformed());
    }
    if length <= 0 {
        return Ok(1); // Positive sub-micro charge, rounded up exactly once.
    }
    if scale >= 0 {
        let value = digits.parse::<u64>().map_err(|_| malformed())?;
        return value
            .checked_mul(10u64.pow(scale as u32))
            .ok_or_else(malformed);
    }
    let length = length as usize;
    let value = digits[..length].parse::<u64>().map_err(|_| malformed())?;
    value
        .checked_add(u64::from(digits[length..].bytes().any(|b| b != b'0')))
        .ok_or_else(malformed)
}

pub(super) fn accounting_choice(choice: &Value, finish: FinishReason) -> bool {
    let expected = match finish {
        FinishReason::Stop => "stop",
        FinishReason::Length => "length",
        FinishReason::ToolCalls => "tool_calls",
    };
    choice["index"] == 0
        && choice["finish_reason"] == expected
        && choice
            .get("delta")
            .and_then(Value::as_object)
            .is_some_and(|delta| {
                delta.iter().all(|(key, value)| match key.as_str() {
                    "role" => value == "assistant",
                    "content" => value.is_null() || value == "",
                    _ => false,
                })
            })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimal_charge_conversion_is_exact_bounded_and_rounds_only_sub_micro_units() {
        for (number, expected) in [
            ("0", 0),
            ("0.0", 0),
            ("1", 1_000_000),
            ("0.000001", 1),
            ("0.0000001", 1),
            ("1.234567000", 1_234_567),
            ("1.234567001", 1_234_568),
            ("2.5e-6", 3),
            ("1e-10000", 1),
            ("0e10000", 0),
            ("1e+6", 1_000_000_000_000),
            ("18446744073709.551615", u64::MAX),
        ] {
            let raw = RawValue::from_string(number.into()).unwrap();
            assert_eq!(microusd(&raw).unwrap(), expected, "{number}");
        }
        for number in [
            "-1",
            "-0.1",
            "true",
            "\"1\"",
            "[]",
            "{}",
            "1e10000",
            "1e2147483648",
            "18446744073709.5516151",
        ] {
            let raw = RawValue::from_string(number.into()).unwrap();
            assert!(microusd(&raw).is_err(), "{number}");
        }
        assert_eq!(accounting(br#"{"usage":{"cost":null}}"#).unwrap().1, None);
        assert_eq!(
            accounting(br#"{"usage":{"cost_details":{"upstream_inference_cost":12}}}"#)
                .unwrap()
                .1,
            None
        );
        assert!(accounting(br#"{"usage":{"cost":1,"cost":2}}"#).is_err());
        let long = format!("{{\"usage\":{{\"cost\":0.{}1}}}}", "0".repeat(128));
        assert!(accounting(long.as_bytes()).is_err());
    }
}
