use std::collections::BTreeSet;

use serde_json::Map;

use mstore::models::Permission;
use mstore::registry::{normalize_permission_alias, normalize_permissions, Registry};
use mstore::MStoreError;

#[test]
fn permission_aliases_match_python_semantics() {
    assert_eq!(
        normalize_permissions(["read"]).unwrap(),
        [Permission::Read, Permission::Info].into_iter().collect()
    );
    assert_eq!(
        normalize_permissions(["write"]).unwrap(),
        [Permission::Read, Permission::Write, Permission::Info]
            .into_iter()
            .collect()
    );
    assert_eq!(normalize_permission_alias("admin").unwrap(), Permission::all());
    assert!(normalize_permissions(["admin"]).is_err());
}

#[test]
fn delegated_token_cannot_escalate() {
    let registry = Registry::new();
    let (object, admin) = registry
        .create_object(4096, None, None, "C".into(), Map::new())
        .unwrap();
    let read = registry
        .issue_token(
            &object.info.object_id,
            &admin,
            normalize_permissions(["read"]).unwrap(),
            None,
        )
        .unwrap();
    let err = registry
        .issue_token(
            &object.info.object_id,
            &read,
            BTreeSet::from([Permission::Write]),
            None,
        )
        .unwrap_err();
    assert!(matches!(err, MStoreError::PermissionDenied(_)));
}


#[test]
fn revoke_and_expiry_block_future_validation() {
    let registry = Registry::new();
    let (object, admin) = registry
        .create_object(4096, None, None, "C".into(), Map::new())
        .unwrap();

    let read = registry
        .issue_token(
            &object.info.object_id,
            &admin,
            normalize_permissions(["read"]).unwrap(),
            None,
        )
        .unwrap();
    registry
        .validate(&object.info.object_id, &read, Permission::Read)
        .unwrap();
    registry
        .revoke_token(&object.info.object_id, &admin, &read)
        .unwrap();
    assert!(matches!(
        registry.validate(&object.info.object_id, &read, Permission::Read),
        Err(MStoreError::TokenRevoked(_))
    ));

    let short = registry
        .issue_token(
            &object.info.object_id,
            &admin,
            normalize_permissions(["read"]).unwrap(),
            Some(0.01),
        )
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    assert!(matches!(
        registry.validate(&object.info.object_id, &short, Permission::Read),
        Err(MStoreError::TokenExpired(_))
    ));
}
