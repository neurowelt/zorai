use serde_json::json;
#[test]
fn mcp_results_preserve_text_structured_errors_and_unsupported_media() {
    let result=serde_json::from_value(json!({"content":[{"type":"text","text":"denied"},{"type":"image","mimeType":"image/png","data":"AA=="}],"structuredContent":{"code":"ROOT_DENIED","fix":"Grant workspace","job_id":"j"},"isError":true,"_meta":{"receipt":"r"}})).unwrap();
    let result = super::super::result::convert(result);
    assert!(result.is_error);
    assert!(result.content.contains("denied"));
    assert!(result.content.contains("ROOT_DENIED"));
    assert!(result.content.contains("Unsupported MCP media"));
    assert!(result.content.contains("AA=="));
    assert!(result.content.contains("receipt"));
    for status in [
        "pending",
        "running",
        "queued",
        "needs_reply",
        "requires_action",
        "completed",
    ] {
        let result =
            serde_json::from_value(json!({"content":[],"structuredContent":{"status":status}}))
                .unwrap();
        assert_eq!(
            super::super::result::convert(result).pending,
            matches!(status, "pending" | "running" | "queued")
        );
    }
}
