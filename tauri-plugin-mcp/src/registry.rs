use crate::{Error, Result, ToolDefinition};
use serde_json::Value;
use std::{collections::BTreeMap, sync::Arc};

pub(crate) const MAX_BYTES: usize = 1_048_576;

pub(crate) struct Tool {
    pub definition: ToolDefinition,
    pub input: jsonschema::Validator,
    pub output: jsonschema::Validator,
}

pub(crate) fn compile(definitions: Vec<ToolDefinition>) -> Result<BTreeMap<String, Arc<Tool>>> {
    if definitions.len() > 128 {
        return Err(Error::new("INVALID_DEFINITION", "Too many tools"));
    }
    bounded(&definitions)?;
    let mut tools = BTreeMap::new();
    for definition in definitions {
        let permissions = &definition.policy.permissions;
        if definition.name.is_empty()
            || definition.name.len() > 128
            || !definition
                .name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"_.-".contains(&c))
            || definition.description.trim().is_empty()
            || definition.description.len() > 4096
            || permissions.iter().any(|p| p.trim().is_empty())
            || permissions
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len()
                != permissions.len()
            || !(1..=300_000).contains(&definition.policy.timeout_ms)
            || !(1..=64).contains(&definition.policy.max_concurrency)
        {
            return Err(Error::new(
                "INVALID_DEFINITION",
                "Invalid name, description or policy",
            ));
        }
        let tool = Arc::new(Tool {
            input: schema(&definition.input_schema)?,
            output: schema(&definition.output_schema)?,
            definition,
        });
        if tools.insert(tool.definition.name.clone(), tool).is_some() {
            return Err(Error::new("DUPLICATE_TOOL", "Duplicate tool name"));
        }
    }
    Ok(tools)
}

pub(crate) fn bounded(value: &impl serde::Serialize) -> Result<()> {
    let bytes =
        serde_json::to_vec(value).map_err(|_| Error::new("INVALID_ARGUMENT", "Invalid JSON"))?;
    if bytes.len() > MAX_BYTES {
        return Err(Error::new("PAYLOAD_TOO_LARGE", "Payload exceeds 1 MiB"));
    }
    Ok(())
}

fn schema(value: &Value) -> Result<jsonschema::Validator> {
    fn scan(value: &Value) -> Result<()> {
        match value {
            Value::Object(object) => {
                for (key, value) in object {
                    if (key == "$ref" || key == "$dynamicRef")
                        && !value
                            .as_str()
                            .is_some_and(|reference| reference.starts_with('#'))
                    {
                        return Err(Error::new(
                            "INVALID_DEFINITION",
                            "Only local schema references are supported",
                        ));
                    }
                    if key == "$schema" && value != "https://json-schema.org/draft/2020-12/schema" {
                        return Err(Error::new(
                            "INVALID_DEFINITION",
                            "Schema dialect must be JSON Schema 2020-12",
                        ));
                    }
                    scan(value)?;
                }
            }
            Value::Array(items) => {
                for item in items {
                    scan(item)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    if value.get("type") != Some(&Value::String("object".into())) {
        return Err(Error::new(
            "INVALID_DEFINITION",
            "Root schema must have type object",
        ));
    }
    scan(value)?;
    jsonschema::options()
        .with_draft(jsonschema::Draft::Draft202012)
        .should_validate_formats(false)
        .build(value)
        .map_err(|_| Error::new("INVALID_DEFINITION", "Invalid JSON Schema"))
}
