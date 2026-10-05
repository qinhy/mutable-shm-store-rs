use serde_json::json;

use mstore::protocol::{decode_frame, encode_frame};

#[test]
fn frame_round_trip_is_big_endian_length_prefixed_json() {
    let message = json!({"op":"ping","args":{}});
    let frame = encode_frame(&message).unwrap();
    let payload_len = u32::from_be_bytes(frame[..4].try_into().unwrap()) as usize;
    assert_eq!(payload_len, frame.len() - 4);
    assert_eq!(decode_frame(&frame).unwrap(), message);
}
