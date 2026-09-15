use super::{McpAuthConfig, McpServerConfig};

pub fn normalize_config(
    mut config: McpServerConfig,
    previous: Option<&McpServerConfig>,
) -> Result<McpServerConfig, String> {
    if config.id.is_empty()
        || config.id.len() > 128
        || !config
            .id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(
            "MCP server ID must contain 1–128 letters, digits, underscores or hyphens".into(),
        );
    }
    if config.name.trim().is_empty() || config.name.len() > 128 {
        return Err("MCP server name is required (maximum 128 bytes)".into());
    }
    config.name = config.name.trim().to_string();
    if config.name.chars().any(char::is_control) {
        return Err("MCP server name must be a single line".into());
    }
    if config.aliases.len() > 16 {
        return Err("MCP server supports at most 16 aliases".into());
    }
    let mut aliases = Vec::<String>::new();
    for alias in &config.aliases {
        let alias = alias.trim();
        if alias.is_empty() || alias.len() > 128 || alias.chars().any(char::is_control) {
            return Err("MCP aliases must contain 1–128 bytes on a single line".into());
        }
        if !aliases
            .iter()
            .any(|value| value.eq_ignore_ascii_case(alias))
        {
            aliases.push(alias.to_string());
        }
    }
    config.aliases = aliases;
    config.skill = config
        .skill
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    if config.skill.as_ref().is_some_and(|skill| {
        skill.len() > 64
            || !skill
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
    }) {
        return Err(
            "MCP skill must be a lowercase skill name (letters, digits, hyphens; maximum 64 bytes)"
                .into(),
        );
    }
    let endpoint = url::Url::parse(&config.url).map_err(|_| "Invalid MCP endpoint URL")?;
    if !matches!(endpoint.scheme(), "http" | "https")
        || endpoint.host_str().is_none()
        || !endpoint.username().is_empty()
        || endpoint.password().is_some()
        || endpoint.fragment().is_some()
        || endpoint.query().is_some()
    {
        return Err(
            "MCP endpoint must be HTTP(S), with a host and without credentials, query, or fragment"
                .into(),
        );
    }
    config.url = endpoint.to_string();
    if previous.is_some_and(|old| url::Url::parse(&old.url).ok().as_ref() != Some(&endpoint)) {
        config.share_workspace_context = false;
        // An endpoint change must never forward a previous endpoint's secret.
        if previous.is_some_and(|old| credential_ref(&old.auth) == credential_ref(&config.auth)) {
            if let Some(reference) = credential_ref_mut(&mut config.auth) {
                *reference = None;
            }
        }
    }
    if let McpAuthConfig::ApiKey { header, .. } = &config.auth {
        let parsed = reqwest::header::HeaderName::from_bytes(header.as_bytes())
            .map_err(|_| "Invalid MCP API-key header name")?;
        if matches!(
            parsed.as_str(),
            "host"
                | "content-length"
                | "content-type"
                | "accept"
                | "cookie"
                | "mcp-session-id"
                | "mcp-protocol-version"
        ) {
            return Err("Reserved HTTP header cannot be used as an MCP API-key header".into());
        }
    }
    if let Some(reference) = credential_ref(&config.auth) {
        uuid::Uuid::parse_str(reference).map_err(|_|"MCP credential reference must be a vault-generated UUID; enter secrets only in the credential editor")?;
    }
    Ok(config)
}

pub(super) fn credential_ref(auth: &McpAuthConfig) -> Option<&str> {
    match auth {
        McpAuthConfig::None => None,
        McpAuthConfig::Bearer { credential_ref } | McpAuthConfig::ApiKey { credential_ref, .. } => {
            credential_ref.as_deref()
        }
    }
}
pub(super) fn credential_ref_mut(auth: &mut McpAuthConfig) -> Option<&mut Option<String>> {
    match auth {
        McpAuthConfig::None => None,
        McpAuthConfig::Bearer { credential_ref } | McpAuthConfig::ApiKey { credential_ref, .. } => {
            Some(credential_ref)
        }
    }
}
