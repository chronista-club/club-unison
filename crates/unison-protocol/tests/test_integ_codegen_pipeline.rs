mod common;

use unison::codegen::{CodeGenerator, RustGenerator};
use unison::parser::{SchemaParser, TypeRegistry};

/// schemas/ping_pong.kdl を読み込み → parse → generate Rust → 構造検証
#[test]
fn test_integ_ping_pong_full_pipeline() {
    let schema = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../schemas/ping_pong.kdl"
    ))
    .unwrap();

    let parser = SchemaParser::new();
    let parsed = parser.parse(&schema).unwrap();
    let type_registry = TypeRegistry::new();
    let generator = RustGenerator::new();
    let code = generator.generate(&parsed, &type_registry).unwrap();

    assert!(
        code.contains("Ping"),
        "Expected Ping struct in generated code"
    );
    assert!(
        code.contains("Pong"),
        "Expected Pong struct in generated code"
    );
    assert!(
        code.contains("UnisonChannel"),
        "Expected UnisonChannel reference in generated code"
    );
}

/// schemas/hierophant.kdl を読み込み → parse → generate → pub/sub + P2P 検証
/// Refs: USN-3 (Hierophant Green 💚)
#[test]
fn test_integ_hierophant_full_pipeline() {
    let schema = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../schemas/hierophant.kdl"
    ))
    .unwrap();

    let parser = SchemaParser::new();
    let parsed = parser.parse(&schema).unwrap();

    // プロトコル基本情報
    let protocol = parsed.protocol.as_ref().expect("protocol should exist");
    assert_eq!(protocol.name, "hierophant");
    assert_eq!(protocol.channels.len(), 2); // identity, pubsub

    let channel_names: Vec<&str> = protocol.channels.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(channel_names, vec!["identity", "pubsub"]);

    // codegen が通ること
    let type_registry = TypeRegistry::new();
    let generator = RustGenerator::new();
    let code = generator.generate(&parsed, &type_registry).unwrap();

    // Identity plane (point-to-point)
    assert!(code.contains("Register"), "Expected Register request");
    assert!(code.contains("Send"), "Expected Send request");
    assert!(code.contains("Received"), "Expected Received event");

    // Pub/Sub plane
    assert!(code.contains("Publish"), "Expected Publish request");
    assert!(
        code.contains("Subscribe"),
        "Expected Subscribe request (pubsub plane)"
    );
    assert!(code.contains("Unsubscribe"), "Expected Unsubscribe request");
    assert!(code.contains("TopicEvent"), "Expected TopicEvent event");
}

/// schemas/creo_sync.kdl → parse → generate → 5チャネル検証
#[test]
fn test_integ_creo_sync_full_pipeline() {
    let schema = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/creo_sync.kdl"
    ))
    .unwrap();

    let parser = SchemaParser::new();
    let parsed = parser.parse(&schema).unwrap();
    let type_registry = TypeRegistry::new();
    let generator = RustGenerator::new();
    let code = generator.generate(&parsed, &type_registry).unwrap();

    // 5チャネルのメッセージ型が生成されていること
    assert!(
        code.contains("Subscribe"),
        "Expected Subscribe from control channel"
    );
    assert!(
        code.contains("MemoryEvent"),
        "Expected MemoryEvent from events channel"
    );
    assert!(code.contains("Query"), "Expected Query from query channel");
    assert!(
        code.contains("SendMessage"),
        "Expected SendMessage from messaging channel"
    );
    assert!(code.contains("Alert"), "Expected Alert from urgent channel");
}

/// request/returns 構文の codegen 検証
#[test]
fn test_integ_request_returns_codegen() {
    let schema = r#"
        protocol "test" version="1.0.0" {
            namespace "test"
            channel "api" from="client" lifetime="persistent" {
                request "GetUser" {
                    field "user_id" type="string" required=#true
                    returns "UserResponse" {
                        field "name" type="string"
                        field "email" type="string"
                    }
                }
            }
        }
    "#;

    let parser = SchemaParser::new();
    let parsed = parser.parse(schema).unwrap();
    let type_registry = TypeRegistry::new();
    let generator = RustGenerator::new();
    let code = generator.generate(&parsed, &type_registry).unwrap();

    assert!(code.contains("GetUser"), "Expected GetUser struct");
    assert!(
        code.contains("UserResponse"),
        "Expected UserResponse struct"
    );
    assert!(code.contains("user_id"), "Expected user_id field");
    assert!(code.contains("name"), "Expected name field");
}

/// event 構文の codegen 検証
#[test]
fn test_integ_event_codegen() {
    let schema = r#"
        protocol "test" version="1.0.0" {
            namespace "test"
            channel "notifications" from="server" lifetime="persistent" {
                event "UserJoined" {
                    field "user_name" type="string" required=#true
                    field "timestamp" type="string"
                }
                event "UserLeft" {
                    field "user_name" type="string" required=#true
                }
            }
        }
    "#;

    let parser = SchemaParser::new();
    let parsed = parser.parse(schema).unwrap();
    let type_registry = TypeRegistry::new();
    let generator = RustGenerator::new();
    let code = generator.generate(&parsed, &type_registry).unwrap();

    assert!(code.contains("UserJoined"), "Expected UserJoined struct");
    assert!(code.contains("UserLeft"), "Expected UserLeft struct");
    assert!(code.contains("user_name"), "Expected user_name field");
}

/// 不正KDLでのパースエラー伝播
#[test]
fn test_integ_invalid_kdl_parse_error() {
    let invalid_schema = r#"
        protocol "test" version="1.0.0" {
            this is not valid kdl {{{}}}
        }
    "#;

    let parser = SchemaParser::new();
    let result = parser.parse(invalid_schema);
    assert!(result.is_err());
}
