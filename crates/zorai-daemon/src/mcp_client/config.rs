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
