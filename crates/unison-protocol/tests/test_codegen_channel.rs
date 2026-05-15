use club_unison::codegen::CodeGenerator;
use club_unison::parser::TypeRegistry;
use club_unison::prelude::*;

#[test]
fn test_channel_codegen() {
    let schema = r#"
        protocol "test-sync" version="1.0.0" {
            namespace "test.sync"

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

    // Channel message structs are generated
    assert!(
        code.contains("Event"),
        "Expected 'Event' struct in generated code"
    );
    assert!(code.contains("event_type"), "Expected 'event_type' field");
    assert!(
        code.contains("QueryRequest"),
        "Expected 'QueryRequest' struct"
    );
    assert!(
        code.contains("QueryResponse"),
        "Expected 'QueryResponse' struct"
    );

    // Connection struct is generated with channel fields
    assert!(code.contains("Connection"), "Expected Connection struct");
    assert!(
        code.contains("events"),
        "Expected 'events' field in Connection"
    );
    assert!(
        code.contains("query"),
        "Expected 'query' field in Connection"
    );
    // Unified Channel: 全チャネルが UnisonChannel 型に統一
    assert!(
        code.contains("UnisonChannel"),
        "Expected UnisonChannel type for all channels"
    );
}

/// v0.10.0: datagram channel の codegen 出力 (= `DatagramChannel` field +
/// `open_datagram_channel` build call)
#[test]
fn test_channel_codegen_datagram_backend() {
    let schema = r#"
        protocol "test-vp" version="1.0.0" {
            namespace "test.vp"

            channel "control" from="either" lifetime="persistent" {
                request "Subscribe" {
                    field "topic" type="string"
                    returns "Subscribed" { field "ok" type="bool" }
                }
            }

            channel "position" from="server" lifetime="persistent" backend="datagram" channel_id=1 {
                event "Transform" {
                    field "id" type="string"
                    field "pos" type="json"
                }
            }
        }
    "#;

    let parser = SchemaParser::new();
    let parsed = parser.parse(schema).unwrap();
    let type_registry = TypeRegistry::new();
    let generator = RustGenerator::new();
    let code = generator.generate(&parsed, &type_registry).unwrap();

    // Datagram channel event struct
    assert!(
        code.contains("Transform"),
        "Expected 'Transform' event struct"
    );

    // Stream channel field は引き続き UnisonChannel
    assert!(
        code.contains("UnisonChannel"),
        "stream channel still uses UnisonChannel"
    );

    // Datagram channel field は DatagramChannel
    assert!(
        code.contains("DatagramChannel"),
        "datagram channel uses DatagramChannel: \n{}",
        code
    );

    // build() call for datagram channel uses open_datagram_channel
    assert!(
        code.contains("open_datagram_channel"),
        "datagram channel build uses open_datagram_channel: \n{}",
        code
    );

    // Stream channel は open_channel を引き続き使う
    assert!(
        code.contains("open_channel"),
        "stream channel build still uses open_channel"
    );

    // Imports に DatagramChannel が含まれる
    assert!(
        code.contains("datagram_channel :: DatagramChannel")
            || code.contains("datagram_channel::DatagramChannel"),
        "imports must include DatagramChannel: \n{}",
        code
    );
}

/// v0.10.0: stream-only schema (= v0.9.0 互換) で `DatagramChannel` import が含まれても
/// `open_datagram_channel` は使われないこと (= backward compat)
#[test]
fn test_channel_codegen_stream_only_backward_compat() {
    let schema = r#"
        protocol "test-legacy" version="1.0.0" {
            namespace "test.legacy"

            channel "events" from="server" lifetime="persistent" {
                event "Update" { field "value" type="string" }
            }
        }
    "#;

    let parser = SchemaParser::new();
    let parsed = parser.parse(schema).unwrap();
    let type_registry = TypeRegistry::new();
    let generator = RustGenerator::new();
    let code = generator.generate(&parsed, &type_registry).unwrap();

    // Stream channel は UnisonChannel + open_channel を使う (= 既存挙動)
    assert!(code.contains("UnisonChannel"));
    assert!(code.contains("open_channel"));
    // datagram path は使われない (= open_datagram_channel call が無い)
    assert!(
        !code.contains("open_datagram_channel"),
        "stream-only schema must not call open_datagram_channel"
    );
}
