#![cfg(all(any(target_os = "linux", target_os = "macos"), feature = "ndarray"))]

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use mstore::{AccessMode, Client, MStoreError, MStoreServer};
use serde_json::Map;
use uuid::Uuid;

fn start_server() -> (Arc<MStoreServer>, thread::JoinHandle<()>, String) {
    let endpoint = format!(
        "unix:///tmp/mstore-rs-test-{}.sock",
        Uuid::new_v4().simple()
    );
    let server = Arc::new(MStoreServer::new(Some(endpoint.clone()), true).unwrap());
    let cloned = Arc::clone(&server);
    let handle = thread::spawn(move || cloned.serve_forever().unwrap());
    for _ in 0..200 {
        if std::path::Path::new(endpoint.trim_start_matches("unix://")).exists() {
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }
    (server, handle, endpoint)
}

#[test]
fn zero_copy_lifecycle_capabilities_and_cache() {
    let (server, handle, endpoint) = start_server();
    let client = Client::new(Some(endpoint), Duration::from_secs(2), 8);
    assert_eq!(client.ping().unwrap()["pong"].as_bool(), Some(true));

    let object = client
        .create_array::<u8>(&[8, 8, 3], "C", Map::new())
        .unwrap();
    unsafe { object.with_array_mut::<u8, _>(|mut image| image.fill(7)) }.unwrap();

    let read_token = object.issue("read", None).unwrap();
    let reader = client
        .open(object.object_id(), &read_token, AccessMode::Read, true)
        .unwrap();
    unsafe { reader.with_array::<u8, _>(|image| assert!(image.iter().all(|&x| x == 7))) }.unwrap();
    assert!(unsafe { reader.with_bytes_mut(|_| ()) }.is_err());

    let reader2 = client
        .open(object.object_id(), &read_token, AccessMode::Read, true)
        .unwrap();
    assert!(reader2.cache_hit());
    assert_eq!(client.cache_info().hits, 1);

    object.revoke(&read_token).unwrap();
    let cached_after_revoke = client
        .open(object.object_id(), &read_token, AccessMode::Read, true)
        .unwrap();
    assert!(cached_after_revoke.cache_hit());
    let fresh_after_revoke = client.open(object.object_id(), &read_token, AccessMode::Read, false);
    assert!(matches!(
        fresh_after_revoke,
        Err(MStoreError::TokenRevoked(_))
    ));

    object.delete().unwrap();
    unsafe { reader.with_bytes(|bytes| assert_eq!(bytes[0], 7)) };
    let err = match client.open(object.object_id(), &read_token, AccessMode::Read, false) {
        Ok(_) => panic!("deleted object unexpectedly reopened"),
        Err(err) => err,
    };
    assert!(matches!(err, MStoreError::ObjectNotFound(_)));

    let raw = client.create(16, None, None, "C", Map::new()).unwrap();
    unsafe { raw.with_bytes_mut(|bytes| bytes[..4].copy_from_slice(b"MSTR")) }.unwrap();
    unsafe { raw.with_bytes(|bytes| assert_eq!(&bytes[..4], b"MSTR")) };
    raw.delete().unwrap();

    client.close();
    server.shutdown();
    handle.join().unwrap();
}
