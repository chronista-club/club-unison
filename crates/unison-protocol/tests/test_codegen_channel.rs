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
