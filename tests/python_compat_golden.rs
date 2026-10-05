use mstore::protocol::decode_frame;
use serde_json::json;

fn decode_hex(s: &str) -> Vec<u8> {
    assert_eq!(s.len() % 2, 0);
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

#[test]
fn decodes_frames_generated_by_python_v03() {
    let frames = [
        (
            include_str!("python_v03_golden.txt").lines().nth(0).unwrap(),
            json!({"op":"ping","args":{}}),
        ),
        (
            include_str!("python_v03_golden.txt").lines().nth(1).unwrap(),
            json!({"op":"open","args":{"object_id":"0123456789abcdef0123456789abcdef","token":"abc_DEF-123","mode":"read"}}),
        ),
        (
            include_str!("python_v03_golden.txt").lines().nth(2).unwrap(),
            json!({"ok":false,"error":{"type":"permission_denied","message":"token lacks permission(s): ['write']"}}),
        ),
    ];

    for (hex, expected) in frames {
        assert_eq!(decode_frame(&decode_hex(hex)).unwrap(), expected);
    }
}
