use unison::prelude::*;

#[test]
fn test_basic_kdl_parsing() {
    let schema_str = r#"
protocol "TestProtocol" version="1.0.0" {
    channel "test-channel" from="client" lifetime="transient" {
        request "testMethod" {
            field "test" type="string" required=#true
            returns "testResult" {
                field "result" type="bool"
            }
        }
    }
}
"#;

    let parser = SchemaParser::new();
    let result = parser.parse(schema_str);

    assert!(result.is_ok(), "パース失敗: {:?}", result.err());

    let schema = result.unwrap();
    assert!(schema.protocol.is_some(), "プロトコルが見つかりません");

    let protocol = schema.protocol.unwrap();
    assert_eq!(protocol.name, "TestProtocol");
    assert_eq!(protocol.version, "1.0.0");
    assert_eq!(protocol.channels.len(), 1);

    let channel = &protocol.channels[0];
    assert_eq!(channel.name, "test-channel");
    assert_eq!(channel.requests.len(), 1);

    let request = &channel.requests[0];
    assert_eq!(request.name, "testMethod");
    assert!(request.returns.is_some());
}

#[test]
fn test_message_with_fields() {
    let schema_str = r#"
message "User" {
    field "id" type="int" required=#true
    field "name" type="string" required=#true
    field "email" type="string"
    field "age" type="int" min=0 max=150
}
"#;

    let parser = SchemaParser::new();
    let result = parser.parse(schema_str);

    assert!(result.is_ok());

    let schema = result.unwrap();
    assert_eq!(schema.messages.len(), 1);

    let message = &schema.messages[0];
    assert_eq!(message.name, "User");
    assert_eq!(message.fields.len(), 4);

    let id_field = &message.fields[0];
    assert_eq!(id_field.name, "id");
    assert!(id_field.required);
}

#[test]
fn test_enum_parsing() {
    let schema_str = r#"
enum "Status" {
    values "pending" "active" "completed" "cancelled"
}
"#;

    let parser = SchemaParser::new();
    let result = parser.parse(schema_str);

    assert!(result.is_ok());

    let schema = result.unwrap();
    assert_eq!(schema.enums.len(), 1);

    let enum_def = &schema.enums[0];
    assert_eq!(enum_def.name, "Status");
    assert_eq!(enum_def.values.len(), 4);
    assert_eq!(enum_def.values[0], "pending");
}

#[test]
fn test_channel_parsing() {
    let schema = r#"
        protocol "test-streaming" version="1.0.0" {
            namespace "test.streaming"

            channel "events" from="server" lifetime="persistent" {
                event "Event" {
                    field "event_type" type="string" required=#true
                    field "payload" type="json"
                }
            }

            channel "query" from="client" lifetime="transient" {
                request "Request" {
                    field "method" type="string" required=#true
                    field "params" type="json"
                    returns "Response" {
                        field "data" type="json"
                    }
                }
            }

            channel "chat" from="either" lifetime="persistent" {
                request "Message" {
                    field "text" type="string" required=#true
                    field "from" type="string"
                    returns "Ack" {
                        field "status" type="string"
                    }
                }
                event "Notice" {
                    field "text" type="string"
                }
            }
        }
    "#;

    let parser = SchemaParser::new();
    let result = parser.parse(schema).unwrap();
    let protocol = result.protocol.as_ref().unwrap();

    // channelが3つパースされること
    assert_eq!(protocol.channels.len(), 3);

    // events channel — event のみ
    let events = &protocol.channels[0];
    assert_eq!(events.name, "events");
    assert_eq!(events.from, ChannelFrom::Server);
    assert_eq!(events.lifetime, ChannelLifetime::Persistent);
    assert_eq!(events.events.len(), 1);
    assert!(events.requests.is_empty());

    let event = &events.events[0];
    assert_eq!(event.name, "Event");
    assert_eq!(event.fields.len(), 2);

    // query channel — request + returns
    let query = &protocol.channels[1];
    assert_eq!(query.from, ChannelFrom::Client);
    assert_eq!(query.lifetime, ChannelLifetime::Transient);
    assert_eq!(query.requests.len(), 1);

    let request = &query.requests[0];
    assert_eq!(request.name, "Request");
    let returns = request.returns.as_ref().unwrap();
    assert_eq!(returns.name, "Response");
    assert_eq!(returns.fields.len(), 1);

    // chat channel — request と event の混在
    let chat = &protocol.channels[2];
    assert_eq!(chat.from, ChannelFrom::Either);
    assert_eq!(chat.requests.len(), 1);
    assert_eq!(chat.events.len(), 1);
}

// === v0.10.0: datagram channel attributes (`backend` / `channel_id`) ===

/// `backend` 属性なしの channel は default `Stream` 解釈 (= v0.9.0 schema 互換)
#[test]
fn test_channel_backend_default_is_stream() {
    let schema = r#"
        protocol "test" version="1.0.0" {
            channel "events" from="server" lifetime="persistent" {
                event "Update" { field "value" type="string" }
            }
        }
    "#;
    let parser = SchemaParser::new();
    let protocol = parser.parse(schema).unwrap().protocol.unwrap();
    let ch = &protocol.channels[0];
    assert_eq!(ch.backend(), ChannelBackend::Stream);
    assert!(
        ch.backend.is_none(),
        "Option field is None when not specified"
    );
    assert!(ch.channel_id.is_none());
}

/// `backend="stream"` 明示 → `ChannelBackend::Stream`
#[test]
fn test_channel_backend_explicit_stream() {
    let schema = r#"
        protocol "test" version="1.0.0" {
            channel "events" from="server" lifetime="persistent" backend="stream" {
                event "Update" { field "value" type="string" }
            }
        }
    "#;
    let parser = SchemaParser::new();
    let protocol = parser.parse(schema).unwrap().protocol.unwrap();
    let ch = &protocol.channels[0];
    assert_eq!(ch.backend(), ChannelBackend::Stream);
    assert_eq!(ch.backend, Some(ChannelBackend::Stream));
}

/// `backend="datagram"` channel_id=1 → `ChannelBackend::Datagram`, channel_id=1
#[test]
fn test_channel_backend_datagram_with_id() {
    let schema = r#"
        protocol "test" version="1.0.0" {
            channel "position" from="server" lifetime="persistent" backend="datagram" channel_id=1 {
                event "Transform" {
                    field "id" type="string"
                    field "pos" type="json"
                }
            }
        }
    "#;
    let parser = SchemaParser::new();
    let protocol = parser.parse(schema).unwrap().protocol.unwrap();
    let ch = &protocol.channels[0];
    assert_eq!(ch.backend(), ChannelBackend::Datagram);
    assert_eq!(ch.channel_id, Some(1));
}

/// `backend="datagram"` で `channel_id` 未指定 → validation error
#[test]
fn test_channel_datagram_without_id_fails() {
    let schema = r#"
        protocol "test" version="1.0.0" {
            channel "position" from="server" lifetime="persistent" backend="datagram" {
                event "Transform" { field "id" type="string" }
            }
        }
    "#;
    let parser = SchemaParser::new();
    let err = parser
        .parse(schema)
        .expect_err("datagram without channel_id must fail");
    let msg = format!("{}", err);
    assert!(
        msg.contains("channel_id"),
        "error must mention channel_id: {}",
        msg
    );
}

/// `channel_id=0` は予約 (= sentinel) → validation error
#[test]
fn test_channel_datagram_id_zero_fails() {
    let schema = r#"
        protocol "test" version="1.0.0" {
            channel "position" from="server" lifetime="persistent" backend="datagram" channel_id=0 {
                event "Transform" { field "id" type="string" }
            }
        }
    "#;
    let parser = SchemaParser::new();
    let err = parser.parse(schema).expect_err("channel_id=0 must fail");
    let msg = format!("{}", err);
    assert!(
        msg.contains("reserved") || msg.contains("0"),
        "error must mention reserved/0: {}",
        msg
    );
}

/// `backend="datagram"` channel に `request` ブロックがあると validation error
#[test]
fn test_channel_datagram_with_request_fails() {
    let schema = r#"
        protocol "test" version="1.0.0" {
            channel "position" from="server" lifetime="persistent" backend="datagram" channel_id=1 {
                request "Query" {
                    field "key" type="string"
                    returns "Result" { field "value" type="string" }
                }
            }
        }
    "#;
    let parser = SchemaParser::new();
    let err = parser
        .parse(schema)
        .expect_err("datagram + request must fail");
    let msg = format!("{}", err);
    assert!(
        msg.contains("datagram") || msg.contains("request"),
        "error must mention datagram/request constraint: {}",
        msg
    );
}

/// Stream channel に channel_id を指定しても害なく動作 (= datagram でなければ無視)
#[test]
fn test_channel_stream_ignores_channel_id() {
    let schema = r#"
        protocol "test" version="1.0.0" {
            channel "events" from="server" lifetime="persistent" channel_id=42 {
                event "Update" { field "value" type="string" }
            }
        }
    "#;
    let parser = SchemaParser::new();
    let protocol = parser.parse(schema).unwrap().protocol.unwrap();
    let ch = &protocol.channels[0];
    assert_eq!(ch.backend(), ChannelBackend::Stream);
    assert_eq!(ch.channel_id, Some(42));
    // stream channel では channel_id が parse はされるが、 意味的には未使用
}

/// Datagram channel + stream channel 共存
#[test]
fn test_channel_mixed_stream_and_datagram_channels() {
    let schema = r#"
        protocol "test" version="1.0.0" {
            channel "control" from="either" lifetime="persistent" {
                request "Subscribe" {
                    field "topic" type="string"
                    returns "Subscribed" { field "ok" type="bool" }
                }
            }
            channel "position" from="server" lifetime="persistent" backend="datagram" channel_id=1 {
                event "Transform" { field "id" type="string" }
            }
            channel "presence" from="either" lifetime="persistent" backend="datagram" channel_id=2 {
                event "Heartbeat" { field "user_id" type="string" }
            }
        }
    "#;
    let parser = SchemaParser::new();
    let protocol = parser.parse(schema).unwrap().protocol.unwrap();
    assert_eq!(protocol.channels.len(), 3);
    assert_eq!(protocol.channels[0].backend(), ChannelBackend::Stream);
    assert_eq!(protocol.channels[1].backend(), ChannelBackend::Datagram);
    assert_eq!(protocol.channels[1].channel_id, Some(1));
    assert_eq!(protocol.channels[2].backend(), ChannelBackend::Datagram);
    assert_eq!(protocol.channels[2].channel_id, Some(2));
}
