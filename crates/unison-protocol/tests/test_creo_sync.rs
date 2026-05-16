use unison::codegen::CodeGenerator;
use unison::parser::TypeRegistry;
use unison::prelude::*;

#[test]
fn test_creo_sync_parse_and_generate() {
    let schema = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/creo_sync.kdl"
    ))
    .unwrap();
    let parser = SchemaParser::new();
    let parsed = parser.parse(&schema).unwrap();

    let protocol = parsed.protocol.as_ref().expect("protocol should exist");
    assert_eq!(protocol.name, "creo-sync");
    assert_eq!(protocol.channels.len(), 5); // control, events, query, messaging, urgent

    // チャネル名を確認
    let channel_names: Vec<&str> = protocol.channels.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(
        channel_names,
        vec!["control", "events", "query", "messaging", "urgent"]
    );

    // コード生成
    let type_registry = TypeRegistry::new();
    let generator = RustGenerator::new();
    let code = generator.generate(&parsed, &type_registry).unwrap();

    // 全メッセージ型が生成されること
    assert!(code.contains("Subscribe"), "Expected Subscribe struct");
    assert!(code.contains("MemoryEvent"), "Expected MemoryEvent struct");
    assert!(code.contains("Query"), "Expected Query struct");
    assert!(code.contains("CCMessage"), "Expected CCMessage struct");
    assert!(code.contains("Alert"), "Expected Alert struct");
    assert!(code.contains("Ack"), "Expected Ack struct");

    // Connection型が生成されること
    assert!(code.contains("Connection"), "Expected Connection struct");

    // チャネルフィールドが生成されること
    assert!(code.contains("control"), "Expected 'control' field");
    assert!(code.contains("events"), "Expected 'events' field");
    assert!(code.contains("query"), "Expected 'query' field");
    assert!(code.contains("messaging"), "Expected 'messaging' field");
    assert!(code.contains("urgent"), "Expected 'urgent' field");
}

#[test]
fn test_creo_sync_channel_types() {
    let schema = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/creo_sync.kdl"
    ))
    .unwrap();
    let parser = SchemaParser::new();
    let parsed = parser.parse(&schema).unwrap();
    let protocol = parsed.protocol.as_ref().unwrap();

    // control: client→server, persistent, request "Subscribe" → returns "Ack"
    let control = &protocol.channels[0];
    assert_eq!(control.from, ChannelFrom::Client);
    assert_eq!(control.lifetime, ChannelLifetime::Persistent);
    assert_eq!(control.requests.len(), 1);
    assert_eq!(control.requests[0].name, "Subscribe");
    assert!(control.requests[0].returns.is_some());
    assert_eq!(control.requests[0].returns.as_ref().unwrap().name, "Ack");

    // events: server→client, persistent, event "MemoryEvent"
    let events = &protocol.channels[1];
    assert_eq!(events.from, ChannelFrom::Server);
    assert_eq!(events.lifetime, ChannelLifetime::Persistent);
    assert_eq!(events.events.len(), 1);
    assert_eq!(events.events[0].name, "MemoryEvent");
    assert!(events.requests.is_empty());

    // query: client→server, persistent, request "Query" + event "QueryError"
    let query = &protocol.channels[2];
    assert_eq!(query.from, ChannelFrom::Client);
    assert_eq!(query.lifetime, ChannelLifetime::Persistent);
    assert_eq!(query.requests.len(), 1);
    assert_eq!(query.requests[0].name, "Query");
    assert!(query.requests[0].returns.is_some());
    assert_eq!(query.events.len(), 1);
    assert_eq!(query.events[0].name, "QueryError");

    // messaging: either, persistent, request + event
    let messaging = &protocol.channels[3];
    assert_eq!(messaging.from, ChannelFrom::Either);
    assert_eq!(messaging.lifetime, ChannelLifetime::Persistent);
    assert_eq!(messaging.requests.len(), 1);
    assert_eq!(messaging.events.len(), 1);
    assert_eq!(messaging.events[0].name, "CCMessage");

    // urgent: server→client, transient, event "Alert"
    let urgent = &protocol.channels[4];
    assert_eq!(urgent.from, ChannelFrom::Server);
    assert_eq!(urgent.lifetime, ChannelLifetime::Transient);
    assert_eq!(urgent.events.len(), 1);
    assert_eq!(urgent.events[0].name, "Alert");
    assert!(urgent.requests.is_empty());
}
