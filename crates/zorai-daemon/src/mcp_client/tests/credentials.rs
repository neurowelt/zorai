use super::super::{credentials::CredentialStore, *};
#[test]
fn mcp_credentials_encrypted_scoped_reload_clear_delete_and_masking() {
    let temp = tempfile::tempdir().unwrap();
    let store = CredentialStore::new(temp.path().to_owned());
    let mut config = McpServerConfig {
        id: "one".into(),
        name: "one".into(),
        url: "http://127.0.0.1:1/mcp".into(),
        auth: McpAuthConfig::Bearer {
            credential_ref: None,
        },
        ..Default::default()
    };
    store
        .edit(
            &mut config,
            McpCredentialUpdate::Replace("very-private-token".into()),
        )
        .unwrap();
    let reference = super::super::config::credential_ref(&config.auth)
        .unwrap()
        .to_owned();
    let path = temp
        .path()
        .join("mcp-credentials")
        .join(format!("{reference}.enc"));
    assert!(!String::from_utf8_lossy(&std::fs::read(&path).unwrap()).contains("very-private-token"));
    let reload = CredentialStore::new(temp.path().to_owned());
    assert_eq!(
        reload.load(&config).unwrap().as_deref(),
        Some("very-private-token")
    );
    let mut other = config.clone();
    other.id = "two".into();
    assert!(reload.load(&other).is_err());
    other = config.clone();
    other.url = "http://127.0.0.1:2/mcp".into();
    assert!(reload.load(&other).is_err());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(temp.path().join("mcp-credentials/key"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    assert!(store
        .edit(&mut config, McpCredentialUpdate::Replace("********".into()))
        .is_err());
    assert!(!format!(
        "{:?}",
        McpCredentialUpdate::Replace("very-private-token".into())
    )
    .contains("very-private-token"));
    store.edit(&mut config, McpCredentialUpdate::Clear).unwrap();
    assert!(store.load(&config).unwrap().is_none());
    store.delete(&reference).unwrap();
    assert!(!path.exists());
}

#[test]
fn mcp_keep_broken_credentials_allows_disable_and_sharing_revocation() {
    let temp = tempfile::tempdir().unwrap();
    let store = CredentialStore::new(temp.path().to_owned());
    let mut config = McpServerConfig {
        id: "one".into(),
        name: "One".into(),
        url: "http://127.0.0.1:1/mcp".into(),
        enabled: true,
        share_workspace_context: true,
        auth: McpAuthConfig::Bearer {
            credential_ref: None,
        },
        ..Default::default()
    };
    store
        .edit(
            &mut config,
            McpCredentialUpdate::Replace("private-secret".into()),
        )
        .unwrap();
    let reference = super::super::config::credential_ref(&config.auth)
        .unwrap()
        .to_owned();
    let path = temp
        .path()
        .join("mcp-credentials")
        .join(format!("{reference}.enc"));
    for missing in [false, true] {
        if missing {
            std::fs::remove_file(&path).unwrap();
        } else {
            std::fs::write(&path, b"broken encrypted credential").unwrap();
        }
        assert!(store.load(&config).is_err());
        config.enabled = false;
        config.share_workspace_context = false;
        store.edit(&mut config, McpCredentialUpdate::Keep).unwrap();
        assert_eq!(
            super::super::config::credential_ref(&config.auth),
            Some(reference.as_str())
        );
        assert!(!config.enabled && !config.share_workspace_context);
    }
}

#[test]
fn mcp_incomplete_key_is_repaired_and_valid_key_is_preserved() {
    for length in [0, 7, 31] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("mcp-credentials");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("key"), vec![0; length]).unwrap();
        let store = CredentialStore::new(temp.path().to_owned());
        let mut config = McpServerConfig {
            id: "one".into(),
            name: "One".into(),
            url: "http://127.0.0.1:1/mcp".into(),
            auth: McpAuthConfig::Bearer {
                credential_ref: None,
            },
            ..Default::default()
        };
        store
            .edit(
                &mut config,
                McpCredentialUpdate::Replace("private-secret".into()),
            )
            .unwrap();
        let key = std::fs::read(root.join("key")).unwrap();
        assert_eq!(key.len(), 32);
        assert_eq!(
            store.load(&config).unwrap().as_deref(),
            Some("private-secret")
        );
        store
            .edit(
                &mut config,
                McpCredentialUpdate::Replace("replacement-secret".into()),
            )
            .unwrap();
        assert_eq!(std::fs::read(root.join("key")).unwrap(), key);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(root.join("key"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn mcp_key_repair_rejects_symlink_without_touching_target() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("mcp-credentials");
    std::fs::create_dir(&root).unwrap();
    let target = temp.path().join("target");
    std::fs::write(&target, b"keep").unwrap();
    std::os::unix::fs::symlink(&target, root.join("key")).unwrap();
    let store = CredentialStore::new(temp.path().to_owned());
    let mut config = McpServerConfig {
        auth: McpAuthConfig::Bearer {
            credential_ref: None,
        },
        ..Default::default()
    };
    assert!(store
        .edit(
            &mut config,
            McpCredentialUpdate::Replace("secret-value".into())
        )
        .unwrap_err()
        .contains("symlink"));
    assert_eq!(std::fs::read(target).unwrap(), b"keep");
}
