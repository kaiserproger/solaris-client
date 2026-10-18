//! Numeric defaults and visibility from pinned StructureBlockEditScreen, not network validation.
pub(super) fn coordinate(value: &str) -> i32 {
    value.parse().unwrap_or(0)
}
pub(super) fn seed(value: &str) -> i64 {
    value.parse().unwrap_or(0)
}
pub(super) fn integrity(value: &str) -> f32 {
    let value = value.trim_matches(|ch| ch <= '\u{20}');
    let (negative, token) = value
        .strip_prefix('-')
        .map_or((false, value.strip_prefix('+').unwrap_or(value)), |v| {
            (true, v)
        });
    if token == "NaN" {
        return f32::NAN;
    }
    if token == "Infinity" {
        return if negative {
            f32::NEG_INFINITY
        } else {
            f32::INFINITY
        };
    }
    let token = token.strip_suffix(['f', 'F', 'd', 'D']).unwrap_or(token);
    if let Some(hex) = token
        .strip_prefix("0x")
        .or_else(|| token.strip_prefix("0X"))
    {
        let Some((mantissa, exponent)) = hex.split_once(['p', 'P']) else {
            return 1.;
        };
        let Some(exponent) = decimal_exponent(exponent) else {
            return 1.;
        };
        let mut fraction = false;
        let mut digits = 0;
        let mut shift = 0;
        let mut number = 0f64;
        for byte in mantissa.bytes() {
            if byte == b'.' && !fraction {
                fraction = true;
                continue;
            }
            let Some(digit) = (byte as char).to_digit(16) else {
                return 1.;
            };
            number = number * 16. + digit as f64;
            digits += 1;
            if fraction {
                shift += 4;
            }
        }
        if digits == 0 {
            return 1.;
        }
        let number = (number * 2f64.powi(exponent.saturating_sub(shift).clamp(-4096, 4096))) as f32;
        return if negative { -number } else { number };
    }
    let mantissa = match token.split_once(['e', 'E']) {
        Some((mantissa, exponent)) if decimal_exponent(exponent).is_some() => mantissa,
        Some(_) => return 1.,
        None => token,
    };
    let mut dots = 0;
    let mut digits = 0;
    for byte in mantissa.bytes() {
        if byte == b'.' {
            dots += 1;
        } else if byte.is_ascii_digit() {
            digits += 1;
        } else {
            return 1.;
        }
    }
    if dots > 1 || digits == 0 {
        return 1.;
    }
    let number = token.parse::<f32>().unwrap_or(1.);
    if negative { -number } else { number }
}
fn decimal_exponent(value: &str) -> Option<i32> {
    let (negative, value) = value
        .strip_prefix('-')
        .map_or((false, value.strip_prefix('+').unwrap_or(value)), |v| {
            (true, v)
        });
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut result = 0i32;
    for byte in value.bytes() {
        result = result
            .saturating_mul(10)
            .saturating_add(i32::from(byte - b'0'));
    }
    Some(if negative {
        result.saturating_neg()
    } else {
        result
    })
}
pub(super) fn field_visible(mode: i32, field: &str) -> bool {
    match mode {
        0 => {
            field == "structure/name"
                || field.starts_with("structure/offset/")
                || field.starts_with("structure/size/")
        }
        1 => {
            field == "structure/name"
                || field.starts_with("structure/offset/")
                || matches!(field, "structure/integrity" | "structure/seed")
        }
        2 => field == "structure/name",
        3 => field == "structure/data",
        _ => false,
    }
}
pub(super) fn control_visible(mode: i32, control: &str) -> bool {
    match control {
        "mode" => true,
        "save" | "detect" | "show_air" => mode == 0,
        "load" | "strict" | "mirror" | "rotation" | "show_box" => mode == 1,
        "ignore_entities" => matches!(mode, 0 | 1),
        _ => false,
    }
}
