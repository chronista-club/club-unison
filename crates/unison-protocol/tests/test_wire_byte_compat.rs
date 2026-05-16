//! Wire byte-compat fixture (= Phase 6b、 TS SDK との wire 一致検証)。
//!
//! 既知の `ProtocolMessage` を Rust の wire encoder で typed frame まで
//! serialize し、 そのバイト列を hex として `tests/fixtures/wire/` に書き出す。
//! TS 側 (`clients/typescript/tests/wire/byte_compat.test.ts`) が同じ論理 message
//! から bytes を生成し、 この fixture と byte 一致することを assert する。
//!
//! これにより live connection なしで「TS が Rust と同じ wire を喋る」ことを
//! 証明する (= real browser↔server round-trip は Phase 6d)。
//!
//! timestamp は `SystemTime::now()` 由来で非決定的なため、 fixture では header を
//! 手組みして `timestamp = 0` に固定する (= 決定的 bytes)。

use std::fs;
use std::path::PathBuf;

use unison::network::{MessageType, ProtocolMessage};
use unison::packet::{PacketType, UnisonPacket, UnisonPacketHeader};

/// FRAME_TYPE_PROTOCOL tag (= quic.rs と同値、 公開されていないのでローカル定義)
const FRAME_TYPE_PROTOCOL: u8 = 0x00;

/// 決定的な header を持つ `UnisonPacket` を組む (= timestamp 0 固定)。
fn deterministic_packet(payload: Vec<u8>) -> UnisonPacket {
    let mut header = UnisonPacketHeader::new(PacketType::Data);
    header.timestamp = 0; // 非決定性を除去
    UnisonPacket::with_header(header, payload).expect("packet build")
}

/// `ProtocolMessage` を 1 本の typed frame バイト列へ encode する。
///
/// layout: `[4B BE total_len][1B 0x00][UnisonPacket]`
/// (= quic.rs `write_typed_frame` と同一)。
fn encode_protocol_frame(msg: ProtocolMessage) -> Vec<u8> {
    // ProtocolMessage → buffa bytes → 決定的 UnisonPacket
    let proto_bytes = {
        // into_frame() は内部 header の timestamp が now() になるため、
        // payload bytes だけ取り出して deterministic_packet で包み直す。
        use ::buffa::Message;
        let proto_msg = protocol_message_to_proto(&msg);
        proto_msg.encode_to_vec()
    };
    let packet = deterministic_packet(proto_bytes);
    let packet_bytes = packet.to_bytes();

    let total_len = (1 + packet_bytes.len()) as u32;
    let mut frame = Vec::with_capacity(4 + total_len as usize);
    frame.extend_from_slice(&total_len.to_be_bytes());
    frame.push(FRAME_TYPE_PROTOCOL);
    frame.extend_from_slice(&packet_bytes);
    frame
}

/// `ProtocolMessage` → buffa `proto::ProtocolMessage`。
///
/// `ProtocolMessage::into_proto` は private なので、 同等の変換をここで再現する。
fn protocol_message_to_proto(msg: &ProtocolMessage) -> unison::proto::ProtocolMessage {
    let msg_type = match msg.msg_type {
        MessageType::Request => unison::proto::MessageType::REQUEST,
        MessageType::Response => unison::proto::MessageType::RESPONSE,
        MessageType::Event => unison::proto::MessageType::EVENT,
        MessageType::Error => unison::proto::MessageType::ERROR,
    };
    unison::proto::ProtocolMessage {
        id: msg.id,
        method: msg.method.clone(),
        msg_type: ::buffa::EnumValue::Known(msg_type),
        payload: msg.payload.clone(),
        __buffa_unknown_fields: Default::default(),
    }
}

/// fixture ディレクトリ (`crates/unison-protocol/tests/fixtures/wire/`)
fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("wire")
}

/// バイト列を lowercase hex 文字列へ
fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// fixture: channel `request` frame の reference bytes を書き出す。
#[test]
fn emit_request_frame_fixture() {
    let msg = ProtocolMessage::new_encoded(
        7,
        "SubscribeMetric".to_string(),
        MessageType::Request,
        br#"{"names":["cpu","memory"]}"#.to_vec(),
    );
    let frame = encode_protocol_frame(msg);

    let dir = fixture_dir();
    fs::create_dir_all(&dir).expect("create fixture dir");
    fs::write(dir.join("request_frame.hex"), to_hex(&frame)).expect("write fixture");

    // sanity: frame は最低限の長さを持つ
    assert!(frame.len() > 4 + 1 + 4);
    assert_eq!(frame[4], FRAME_TYPE_PROTOCOL);
}

/// fixture: channel `event` frame の reference bytes を書き出す。
#[test]
fn emit_event_frame_fixture() {
    let msg = ProtocolMessage::new_encoded(
        0,
        "MetricUpdate".to_string(),
        MessageType::Event,
        br#"{"name":"cpu","value":42}"#.to_vec(),
    );
    let frame = encode_protocol_frame(msg);

    let dir = fixture_dir();
    fs::create_dir_all(&dir).expect("create fixture dir");
    fs::write(dir.join("event_frame.hex"), to_hex(&frame)).expect("write fixture");

    assert!(frame.len() > 4 + 1 + 4);
}

/// fixture: `__identity` frame の reference bytes を書き出す。
#[test]
fn emit_identity_frame_fixture() {
    let identity_json = br#"{"name":"test-server","version":"1.0.0","namespace":"club.chronista.test","channels":[],"metadata":null}"#;
    let msg = ProtocolMessage::new_encoded(
        0,
        "__identity".to_string(),
        MessageType::Event,
        identity_json.to_vec(),
    );
    let frame = encode_protocol_frame(msg);

    let dir = fixture_dir();
    fs::create_dir_all(&dir).expect("create fixture dir");
    fs::write(dir.join("identity_frame.hex"), to_hex(&frame)).expect("write fixture");

    assert!(frame.len() > 4 + 1 + 4);
}

/// Rust 側の round-trip 健全性: encode した frame を Rust decoder で復元できる。
#[test]
fn rust_frame_round_trips() {
    let original = ProtocolMessage::new_encoded(
        7,
        "SubscribeMetric".to_string(),
        MessageType::Request,
        b"{}".to_vec(),
    );
    let frame = encode_protocol_frame(original.clone());

    // typed frame を手で剥がす
    let total_len = u32::from_be_bytes([frame[0], frame[1], frame[2], frame[3]]) as usize;
    assert_eq!(total_len, frame.len() - 4);
    assert_eq!(frame[4], FRAME_TYPE_PROTOCOL);
    let packet_bytes = bytes::Bytes::copy_from_slice(&frame[5..]);

    let packet = UnisonPacket::from_bytes(&packet_bytes).expect("packet decode");
    let restored = ProtocolMessage::from_frame(&packet).expect("message decode");

    assert_eq!(restored.id, original.id);
    assert_eq!(restored.method, original.method);
    assert_eq!(restored.msg_type, original.msg_type);
    assert_eq!(restored.payload, original.payload);
}
