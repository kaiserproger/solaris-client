use serde_json::Value;
use std::sync::LazyLock;

pub(super) fn tools() -> &'static Vec<Value> {
    static TOOLS: LazyLock<Vec<Value>> = LazyLock::new(|| {
        let mut tools: Vec<Value> = serde_json::from_str(include_str!("minecraft_catalog.json"))
            .expect("checked canonical Java Minecraft catalog");
        tools.retain(|tool| !matches!(tool["name"].as_str(), Some("minecraft_run_scenario")));
        if let Some(scan) = tools
            .iter_mut()
            .find(|tool| tool["name"] == "minecraft_scan_blocks")
        {
            scan["inputSchema"]["properties"]["offset"] = serde_json::json!({
                "type":"integer","minimum":0,"maximum":4096,
                "description":"Continue a bounded scan using its next_offset, with unchanged bounds."
            });
            scan["description"] = serde_json::json!(
                "Read received block states in y/z/x order. Responses are byte-bounded; when truncated, repeat unchanged bounds with offset=next_offset. count is returned rows; total_count is requested volume."
            );
        }
        tools.push(serde_json::json!({
            "name":"client_capture",
            "description":"Capture the real GPU scene target to a PNG artifact.",
            "inputSchema":{"type":"object","properties":{"name":{"type":"string","minLength":1,"maxLength":100}},"required":["name"],"additionalProperties":false},
            "annotations":{"readOnlyHint":false,"destructiveHint":false,"idempotentHint":false,"openWorldHint":false}
        }));
        tools
    });
    &TOOLS
}

pub(super) fn state_supported(name: &str) -> bool {
    !matches!(
        name,
        "client_capture"
            | "minecraft_screenshot"
            | "minecraft_click_confirmation_button"
            | "minecraft_click_screen_button"
            | "minecraft_set_screen_input"
    )
}

pub(super) fn resources() -> Vec<Value> {
    [
        "guide",
        "capabilities",
        "instances",
        "scenarios",
        "limits",
        "compatibility",
    ]
    .into_iter()
    .map(|name| {
        serde_json::json!({
            "uri":format!("solaris-client://{name}"),"name":name,"mimeType":"application/json"
        })
    })
    .collect()
}

pub(super) fn definition(name: &str) -> Result<&'static Value, String> {
    tools()
        .iter()
        .find(|tool| tool["name"].as_str() == Some(name))
        .ok_or_else(|| format!("unknown_tool: {name}"))
}

pub(super) fn validate(name: &str, arguments: &Value) -> Result<&'static Value, String> {
    let definition = definition(name)?;
    validate_value(&definition["inputSchema"], arguments, "arguments")?;
    Ok(definition)
}

fn validate_value(schema: &Value, value: &Value, path: &str) -> Result<(), String> {
    let invalid = |reason: &str| format!("invalid_arguments: {path} {reason}");
    if let Some(variants) = schema["anyOf"].as_array() {
        if !variants
            .iter()
            .any(|variant| validate_value(variant, value, path).is_ok())
        {
            return Err(invalid("does not match a declared alternative"));
        }
    }
    if let Some(variants) = schema["enum"].as_array() {
        if !variants.contains(value) {
            return Err(invalid("is outside the declared enum"));
        }
    }
    match schema["type"].as_str() {
        Some("object") => {
            let object = value
                .as_object()
                .ok_or_else(|| invalid("must be an object"))?;
            if let Some(required) = schema["required"].as_array() {
                for key in required {
                    let key = key.as_str().expect("catalog required property string");
                    if !object.contains_key(key) {
                        return Err(invalid(&format!("requires {key}")));
                    }
                }
            }
            if let Some(limit) = schema["maxProperties"].as_u64() {
                if object.len() as u64 > limit {
                    return Err(invalid("has too many properties"));
                }
            }
            if let Some(dependencies) = schema["dependentRequired"].as_object() {
                for (key, required) in dependencies {
                    if object.contains_key(key) {
                        for dependency in required.as_array().expect("catalog dependency array") {
                            let dependency =
                                dependency.as_str().expect("catalog dependency string");
                            if !object.contains_key(dependency) {
                                return Err(invalid(&format!("{key} requires {dependency}")));
                            }
                        }
                    }
                }
            }
            for (key, value) in object {
                if let Some(property) = schema["properties"].get(key) {
                    validate_value(property, value, &format!("{path}.{key}"))?;
                } else {
                    match &schema["additionalProperties"] {
                        Value::Bool(false) => {
                            return Err(invalid(&format!("has undeclared {key}")));
                        }
                        Value::Object(_) => validate_value(
                            &schema["additionalProperties"],
                            value,
                            &format!("{path}.{key}"),
                        )?,
                        _ => {}
                    }
                }
            }
        }
        Some("array") => {
            let array = value
                .as_array()
                .ok_or_else(|| invalid("must be an array"))?;
            for (bound, minimum) in [("minItems", true), ("maxItems", false)] {
                if let Some(limit) = schema[bound].as_u64() {
                    if (minimum && (array.len() as u64) < limit)
                        || (!minimum && (array.len() as u64) > limit)
                    {
                        return Err(invalid("violates the declared item count"));
                    }
                }
            }
            if schema.get("items").is_some() {
                for (index, item) in array.iter().enumerate() {
                    validate_value(&schema["items"], item, &format!("{path}[{index}]"))?;
                }
            }
        }
        Some("string") => {
            let text = value.as_str().ok_or_else(|| invalid("must be a string"))?;
            let length = text.encode_utf16().count() as u64;
            if schema["minLength"]
                .as_u64()
                .is_some_and(|minimum| length < minimum)
                || schema["maxLength"]
                    .as_u64()
                    .is_some_and(|maximum| length > maximum)
            {
                return Err(invalid("violates the declared string length"));
            }
            if let Some(pattern) = schema["pattern"].as_str() {
                // The canonical catalog has one closed physical-key grammar. Avoid a
                // second parser or regex engine for a fixed ASCII protocol token.
                if pattern != "^key\\.keyboard\\.[a-z0-9.]{1,32}$" {
                    return Err("unsupported canonical catalog pattern".into());
                }
                let key = text
                    .strip_prefix("key.keyboard.")
                    .filter(|key| (1..=32).contains(&key.len()))
                    .filter(|key| {
                        key.bytes().all(|byte| {
                            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'.'
                        })
                    });
                if key.is_none() {
                    return Err(invalid("violates the physical-key grammar"));
                }
            }
        }
        Some("integer" | "number") => {
            let number = value
                .as_f64()
                .filter(|number| number.is_finite())
                .ok_or_else(|| invalid("must be a finite number"))?;
            if schema["type"] == "integer" && number.fract() != 0. {
                return Err(invalid("must be an integer"));
            }
            if schema["minimum"]
                .as_f64()
                .is_some_and(|minimum| number < minimum)
                || schema["maximum"]
                    .as_f64()
                    .is_some_and(|maximum| number > maximum)
            {
                return Err(invalid("is outside the declared numeric range"));
            }
        }
        Some("boolean") if !value.is_boolean() => return Err(invalid("must be a boolean")),
        Some("boolean") | None => {}
        Some(_) => return Err("unsupported canonical catalog schema type".into()),
    }
    Ok(())
}
