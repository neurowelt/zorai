use super::*;
pub(crate) fn normalize_tool_dispatch(
    tool_name: &str,
    args: &serde_json::Value,
) -> (String, serde_json::Value) {
    match tool_name {
        tool_names::SUMMARY => {
            let mut normalized = args.clone();
            if let serde_json::Value::Object(ref mut map) = normalized {
                map.insert(
                    "kind".to_string(),
                    serde_json::Value::String(tool_names::SUMMARY.to_string()),
                );
            }
            (tool_names::SEMANTIC_QUERY.to_string(), normalized)
        }
        _ => (
            tool_names::canonical_tool_name(tool_name).to_string(),
            args.clone(),
        ),
    }
}

pub(crate) fn parse_tool_args(
    tool_name: &str,
    raw_arguments: &str,
) -> std::result::Result<serde_json::Value, String> {
    if matches!(tool_name, tool_names::CREATE_FILE | tool_names::WRITE_FILE) {
        if let Ok(args) = parse_file_multipart_args(raw_arguments) {
            return Ok(args);
        }
    }
    serde_json::from_str(raw_arguments).map_err(|error| {
        let preview: String = raw_arguments.chars().take(240).collect();
        format!(
            "Invalid JSON arguments for tool `{tool_name}`: {error}. Argument length: {}. Preview: {}{}",
            raw_arguments.len(),
            preview,
            if raw_arguments.chars().count() > 240 { "..." } else { "" }
        )
    })
}

fn parse_file_multipart_args(raw_arguments: &str) -> Result<serde_json::Value> {
    let trimmed = raw_arguments.trim();
    if trimmed.is_empty() || trimmed.starts_with('{') {
        anyhow::bail!("not a multipart payload");
    }

    let boundary = if let Some(header) = trimmed.lines().next() {
        if header
            .to_ascii_lowercase()
            .starts_with("content-type: multipart/form-data;")
        {
            header
                .split("boundary=")
                .nth(1)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| value.trim_matches('"').to_string())
                .ok_or_else(|| {
                    anyhow::anyhow!("multipart boundary missing from Content-Type header")
                })?
        } else if let Some(rest) = trimmed.strip_prefix("--") {
            rest.lines()
                .next()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| anyhow::anyhow!("multipart boundary missing from body"))?
                .to_string()
        } else {
            anyhow::bail!("not a multipart payload");
        }
    } else {
        anyhow::bail!("not a multipart payload");
    };

    let body = if trimmed
        .to_ascii_lowercase()
        .starts_with("content-type: multipart/form-data;")
    {
        trimmed
            .split_once("\n\n")
            .map(|(_, value)| value)
            .ok_or_else(|| anyhow::anyhow!("multipart payload missing body"))?
    } else {
        trimmed
    };

    let delimiter = format!("--{boundary}");
    let mut fields = serde_json::Map::new();

    for chunk in body.split(&delimiter).skip(1) {
        let part = chunk.trim_start_matches('\r').trim_start_matches('\n');
        if part.is_empty() || part == "--" {
            continue;
        }
        let part = part.strip_suffix("--").unwrap_or(part).trim();
        if part.is_empty() {
            continue;
        }

        let (headers, value) = part
            .split_once("\n\n")
            .or_else(|| part.split_once("\r\n\r\n"))
            .ok_or_else(|| anyhow::anyhow!("multipart part missing header/body separator"))?;
        let mut name = None;
        let mut filename = None;

        for header in headers.lines() {
            let lower = header.to_ascii_lowercase();
            if !lower.starts_with("content-disposition:") {
                continue;
            }
            for segment in header.split(';').skip(1) {
                let segment = segment.trim();
                if let Some(value) = segment.strip_prefix("name=") {
                    name = Some(value.trim_matches('"').to_string());
                } else if let Some(value) = segment.strip_prefix("filename=") {
                    filename = Some(value.trim_matches('"').to_string());
                }
            }
        }

        let name = name.ok_or_else(|| anyhow::anyhow!("multipart part missing name"))?;
        let value = value
            .trim_end_matches('\r')
            .trim_end_matches('\n')
            .to_string();
        if name == "file" || name == "content" {
            fields.insert("content".to_string(), serde_json::Value::String(value));
            if let Some(filename) = filename {
                fields
                    .entry("filename".to_string())
                    .or_insert_with(|| serde_json::Value::String(filename));
            }
        } else {
            fields.insert(name, serde_json::Value::String(value));
        }
    }

    if !fields.contains_key("content") {
        anyhow::bail!("multipart payload missing file/content part");
    }

    Ok(serde_json::Value::Object(fields))
}

pub(crate) fn get_string_arg<'a>(args: &'a serde_json::Value, names: &[&str]) -> Option<&'a str> {
    names
        .iter()
        .find_map(|name| args.get(*name).and_then(|value| value.as_str()))
}

pub(crate) fn get_apply_patch_text_arg<'a>(args: &'a serde_json::Value) -> Option<&'a str> {
    get_string_arg(args, &["input", "patch"])
}

pub(crate) fn get_file_path_arg<'a>(args: &'a serde_json::Value) -> Option<&'a str> {
    ["path", "file_path", "filepath", "filename", "file"]
        .into_iter()
        .find_map(|name| {
            args.get(name)
                .and_then(|value| value.as_str())
                .filter(|value| !value.trim().is_empty())
        })
}

pub(crate) fn get_explicit_cwd_arg<'a>(args: &'a serde_json::Value) -> Option<&'a str> {
    args.get("cwd")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

pub(crate) fn get_file_content_arg(args: &serde_json::Value) -> Result<String> {
    if let Some(value) = get_string_arg(args, &["content", "contents", "text", "data", "body"]) {
        return Ok(value.to_string());
    }
    if let Some(encoded) =
        get_string_arg(args, &["content_base64", "contents_base64", "data_base64"])
    {
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|error| anyhow::anyhow!("invalid base64 file content: {error}"))?;
        return String::from_utf8(decoded)
            .map_err(|error| anyhow::anyhow!("decoded file content is not utf-8: {error}"));
    }
    anyhow::bail!("missing file content argument (expected one of: content, contents, text, data, body, content_base64)")
}
