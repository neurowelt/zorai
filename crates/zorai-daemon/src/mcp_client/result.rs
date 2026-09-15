#[derive(Debug, Clone)]
pub struct McpCallOutcome {
    pub content: String,
    pub is_error: bool,
    pub pending: bool,
}

pub(super) fn convert(result: rmcp::model::CallToolResult) -> McpCallOutcome {
    let value = serde_json::to_value(&result).expect("MCP result serialization");
    let mut texts = Vec::new();
    for content in value["content"].as_array().into_iter().flatten() {
        if content["type"] == "text" {
            if let Some(text) = content["text"].as_str() {
                texts.push(text.to_owned());
            }
            if let Some(mut details) = content.as_object().cloned() {
                details.remove("type");
                details.remove("text");
                if !details.is_empty() {
                    texts.push(format!(
                        "Text content metadata: {}",
                        serde_json::Value::Object(details)
                    ));
                }
            }
        } else {
            texts.push(format!(
                "Unsupported MCP media content (preserved as JSON): {content}"
            ));
        }
    }
    if let Some(structured) = value.get("structuredContent") {
        texts.push(format!("structuredContent: {structured}"));
    }
    if let Some(meta) = value.get("_meta") {
        texts.push(format!("result metadata: {meta}"));
    }
    let pending = is_pending(&value["structuredContent"])
        || value["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v["text"].as_str())
            .filter_map(|text| serde_json::from_str::<serde_json::Value>(text).ok())
            .any(|v| is_pending(&v));
    McpCallOutcome {
        content: texts.join("\n"),
        is_error: result.is_error.unwrap_or(false),
        pending,
    }
}
fn is_pending(value: &serde_json::Value) -> bool {
    matches!(
        value["status"].as_str(),
        Some("pending" | "running" | "queued" | "in_progress")
    )
}
