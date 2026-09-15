use super::super::{catalog::public_name, normalize_config, McpServerConfig};
#[test]
fn mcp_catalog_routes_stable_distinct_and_provider_compatible() {
    let mut names = std::collections::HashSet::new();
    for server in ["a", "b", "a-b", "a_b"] {
        for tool in [
            "consult",
            "Consult",
            "get/answer",
            "get_answer",
            "工具",
            &"x".repeat(200),
        ] {
            let name = public_name(server, tool);
            assert!(name.len() <= 64);
            assert!(name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'));
            assert_eq!(name, public_name(server, tool));
            assert!(names.insert(name));
        }
    }
}
#[test]
fn mcp_config_endpoint_resets_consent_and_credentials() {
    let old = McpServerConfig {
        id: "server".into(),
        name: "Test".into(),
        url: "http://127.0.0.1:1/mcp".into(),
        share_workspace_context: true,
        auth: super::super::McpAuthConfig::Bearer {
            credential_ref: Some("00000000-0000-4000-8000-000000000001".into()),
        },
        ..Default::default()
    };
    let mut changed = old.clone();
    changed.url = "http://127.0.0.1:2/mcp".into();
    let changed = normalize_config(changed, Some(&old)).unwrap();
    assert!(!changed.share_workspace_context);
    assert!(super::super::config::credential_ref(&changed.auth).is_none());
    let mut renamed = old.clone();
    renamed.name = "Renamed".into();
    assert!(
        normalize_config(renamed, Some(&old))
            .unwrap()
            .share_workspace_context
    );
    for url in [
        "file:///tmp/x",
        "http://secret@localhost/mcp",
        "http://localhost/mcp?token=secret",
        "http://localhost/mcp#secret",
    ] {
        let mut config = old.clone();
        config.url = url.into();
        assert!(normalize_config(config, None).is_err());
    }
}

#[test]
fn mcp_discovery_metadata_defaults_and_validation() {
    let legacy = serde_json::json!({"id":"portal","name":"Local", "url":"http://localhost:1/mcp","adapter":"portal"});
    let config: McpServerConfig = serde_json::from_value(legacy).unwrap();
    assert_eq!(config.workflow_skill(), Some("companions"));
    assert_eq!(config.discovery_aliases(), ["portal", "companions"]);
    let mut edited = config.clone();
    edited.aliases = vec![" Advice ".into(), "advice".into()];
    edited.skill = Some(" custom-skill ".into());
    let normalized = normalize_config(edited, Some(&config)).unwrap();
    assert_eq!(normalized.aliases, ["Advice"]);
    assert_eq!(normalized.workflow_skill(), Some("custom-skill"));
    for skill in ["../escape", "two words", "line\nbreak", "UPPER"] {
        let mut invalid = config.clone();
        invalid.skill = Some(skill.into());
        assert!(normalize_config(invalid, None).is_err());
    }
    let mut invalid = config;
    invalid.aliases = vec!["bad\nname".into()];
    assert!(normalize_config(invalid, None).is_err());
}
