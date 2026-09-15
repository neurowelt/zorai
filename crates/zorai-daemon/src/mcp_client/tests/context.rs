use super::super::context::*;
use serde_json::json;

#[test]
fn mcp_context_capability_consent_truth_table_and_no_cwd() {
    let directory = tempfile::tempdir().unwrap();
    let context = McpRequestContext::from_workspace("thread-a", Some(directory.path()));
    for supported in [false, true] {
        for consent in [false, true] {
            let support = ContextSupport {
                supported,
                ..Default::default()
            };
            let meta = support.metadata(consent, false, &context).unwrap();
            assert_eq!(meta.contains_key(WORKSPACE), supported && consent);
            assert_eq!(meta.contains_key(CONVERSATION), supported && consent);
            assert_eq!(
                support.metadata(consent, true, &context).is_ok(),
                supported && consent
            );
        }
    }
    let absent = McpRequestContext::from_workspace("thread-a", None);
    assert!(absent.workspace_uri.is_none());
    assert_eq!(
        ContextSupport {
            supported: true,
            ..Default::default()
        }
        .metadata(true, true, &absent)
        .unwrap_err()
        .code,
        "MCP_WORKSPACE_MISSING"
    );
}

#[test]
fn mcp_context_uri_escaping_invalid_and_capability_versions() {
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().join("a space # ü");
    std::fs::create_dir(&directory).unwrap();
    let context = McpRequestContext::from_workspace("t", Some(&directory));
    let uri = context.workspace_uri.unwrap();
    assert!(uri.as_str().contains("%20"));
    assert!(uri.as_str().contains("%23"));
    assert_eq!(
        uri.to_file_path().unwrap(),
        directory.canonicalize().unwrap()
    );
    assert!(
        McpRequestContext::from_workspace("t", Some(std::path::Path::new("relative")))
            .error
            .is_some()
    );
    for capability in [
        json!({}),
        json!({"experimental":{CAPABILITY:{"version":2}}}),
        json!({"experimental":{CAPABILITY:{"version":"1"}}}),
        json!({"experimental":{CAPABILITY:{"version":1,"requiredForTools":"consult"}}}),
        json!({"experimental":{CAPABILITY:{"version":1,"requiredForTools":["consult",42]}}}),
    ] {
        assert!(!ContextSupport::parse(&capability).supported);
    }
    assert!(ContextSupport::parse(
        &json!({"experimental":{CAPABILITY:{"version":1,"requiredForTools":["consult"]}}})
    )
    .required
    .contains("consult"));
}

#[test]
fn mcp_context_stale_uri_and_conflict_are_errors() {
    let temp = tempfile::tempdir().unwrap();
    let context = McpRequestContext::from_workspace("t", Some(temp.path()));
    drop(temp);
    assert_eq!(
        ContextSupport {
            supported: true,
            ..Default::default()
        }
        .metadata(true, true, &context)
        .unwrap_err()
        .code,
        "MCP_WORKSPACE_INVALID"
    );
    let context = McpRequestContext {
        thread_id: "t".into(),
        workspace_uri: None,
        error: Some(McpCallError::new(
            "MCP_WORKSPACE_CONFLICT",
            "conflicting binding",
        )),
    };
    assert_eq!(
        ContextSupport {
            supported: true,
            ..Default::default()
        }
        .metadata(true, true, &context)
        .unwrap_err()
        .code,
        "MCP_WORKSPACE_CONFLICT"
    );
}
