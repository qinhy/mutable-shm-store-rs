use serde_json::Map;

use mstore::{connect, AccessMode, Result};

fn main() -> Result<()> {
    let store = connect(None);
    println!("ping = {}", store.ping()?);

    let image = store.create_array::<u8>(&[4, 4, 3], "C", Map::new())?;
    unsafe { image.with_array_mut::<u8, _>(|mut arr| arr.fill(10)) }?;

    let read_token = image.issue("read", None)?;
    let reader = store.open(image.object_id(), &read_token, AccessMode::Read, true)?;
    unsafe {
        reader.with_array::<u8, _>(|arr| {
            println!("shape={:?}, first={}", arr.shape(), arr[[0, 0, 0]]);
        })
    }?;

    Ok(())
}
