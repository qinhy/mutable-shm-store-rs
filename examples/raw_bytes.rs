use serde_json::Map;

use mstore::{connect, AccessMode, Result};

fn main() -> Result<()> {
    let store = connect(None);
    let object = store.create(1024 * 1024, None, None, "C", Map::new())?;
    unsafe { object.with_bytes_mut(|bytes| bytes[..4].copy_from_slice(b"MSTR")) }?;

    let read_token = object.issue("read", None)?;
    let reader = store.open(object.object_id(), &read_token, AccessMode::Read, true)?;
    unsafe { reader.with_bytes(|bytes| assert_eq!(&bytes[..4], b"MSTR")) };
    println!("{} is shared without payload IPC copies", reader.object_id());
    Ok(())
}
