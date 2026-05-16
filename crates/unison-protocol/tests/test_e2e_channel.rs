//! E2Eテスト: creo_syncスキーマから生成されるConnectionBuilder の検証
//!
//! チャネル型の生成（UnisonChannel, ConnectionBuilder）を検証。

use unison::codegen::CodeGenerator;
use unison::parser::TypeRegistry;
use unison::prelude::*;

#[test]
fn test_e2e_connection_builder_generation() {
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

    // QuicConnection構造体が生成されること
    assert!(
        code.contains("QuicConnection"),
        "Expected QuicConnection struct in generated code"
    );

    // ConnectionBuilderトレイトが生成されること
    assert!(
        code.contains("ConnectionBuilder"),
        "Expected ConnectionBuilder trait in generated code"
    );

    // build()メソッドが生成されること
    assert!(
        code.contains("build"),
        "Expected build() method in generated code"
    );

    // open_channelが使われること
    assert!(
        code.contains("open_channel"),
        "Expected open_channel calls in generated code"
    );

    // 5つのチャネルすべてがQuicConnection内にあること
    for channel in &["control", "events", "query", "messaging", "urgent"] {
        assert!(
            code.contains(channel),
            "Expected '{}' channel in QuicConnection",
            channel
        );
    }

    // UnisonChannel型が使われること（Unified Channel統合後）
    assert!(
        code.contains("UnisonChannel"),
        "Expected UnisonChannel type in generated code"
    );
}

#[test]
fn test_e2e_channel_type_mapping() {
    let schema = r#"
        protocol "test-sync" version="1.0.0" {
            namespace "test.sync"

            channel "control" from="client" lifetime="persistent" {
                send "ControlCmd" {
                    field "action" type="string" required=#true
                }
                recv "ControlAck" {
                    field "status" type="string" required=#true
                }
            }

            channel "events" from="server" lifetime="persistent" {
                send "Event" {
                    field "event_type" type="string" required=#true
                    field "data" type="json"
                }
            }

            channel "query" from="client" lifetime="transient" {
                send "QueryRequest" {
                    field "method" type="string" required=#true
                }
                recv "QueryResponse" {
                    field "result" type="json"
                }
            }
        }
    "#;

    let parser = SchemaParser::new();
    let parsed = parser.parse(schema).unwrap();
    let type_registry = TypeRegistry::new();
    let generator = RustGenerator::new();
    let code = generator.generate(&parsed, &type_registry).unwrap();

    // Unified Channel: 全チャネルが UnisonChannel 型に統一
    assert!(
        code.contains("UnisonChannel"),
        "Expected UnisonChannel for all channels"
    );

    // ConnectionBuilderトレイトとimpl
    assert!(
        code.contains("ConnectionBuilder"),
        "Expected ConnectionBuilder trait"
    );
}

#[test]
fn test_unison_channel_imports() {
    let schema = r#"
        protocol "minimal" version="1.0.0" {
            namespace "test"

            channel "data" from="client" lifetime="persistent" {
                send "DataMsg" {
                    field "value" type="string" required=#true
                }
                recv "DataAck" {
                    field "ok" type="bool" required=#true
                }
            }
        }
    "#;

    let parser = SchemaParser::new();
    let parsed = parser.parse(schema).unwrap();
    let type_registry = TypeRegistry::new();
    let generator = RustGenerator::new();
    let code = generator.generate(&parsed, &type_registry).unwrap();

    // UnisonChannel importが含まれること
    assert!(
        code.contains("UnisonChannel"),
        "Expected UnisonChannel in imports"
    );
}
