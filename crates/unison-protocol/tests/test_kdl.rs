use unison::prelude::*;

#[test]
fn test_basic_kdl_parsing() {
    let schema_str = r#"
protocol "TestProtocol" version="1.0.0" {
    service "TestService" {
        method "testMethod" {
            request {
                field "test" type="string" required=#true
            }
            response {
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
    assert_eq!(protocol.services.len(), 1);

    let service = &protocol.services[0];
    assert_eq!(service.name, "TestService");
    assert_eq!(service.methods.len(), 1);

    let method = &service.methods[0];
    assert_eq!(method.name, "testMethod");
    assert!(method.request.is_some());
    assert!(method.response.is_some());
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
                send "Event" {
                    field "event_type" type="string" required=#true
                    field "payload" type="json"
                }
            }

            channel "control" from="client" lifetime="persistent" {
                send "Subscribe" {
                    field "category" type="string"
                }
                recv "Ack" {
                    field "status" type="string"
                }
            }

            channel "query" from="client" lifetime="transient" {
                send "Request" {
                    field "method" type="string" required=#true
                    field "params" type="json"
                }
                recv "Response" {
                    field "data" type="json"
                }
                error "QueryError" {
                    field "code" type="string"
                    field "message" type="string"
                }
            }

            channel "chat" from="either" lifetime="persistent" {
                send "Message" {
                    field "text" type="string" required=#true
                    field "from" type="string"
                }
                recv "Message"
            }
        }
    "#;

    let parser = SchemaParser::new();
    let result = parser.parse(schema).unwrap();
    let protocol = result.protocol.as_ref().unwrap();

    // channelが4つパースされること
    assert_eq!(protocol.channels.len(), 4);

    // events channel
    let events = &protocol.channels[0];
    assert_eq!(events.name, "events");
    assert_eq!(events.from, ChannelFrom::Server);
    assert_eq!(events.lifetime, ChannelLifetime::Persistent);
    assert!(events.send.is_some());
    assert!(events.recv.is_none());

    // events.send のメッセージ名とフィールドを確認
    let send_msg = events.send.as_ref().unwrap();
    assert_eq!(send_msg.name, "Event");
    assert_eq!(send_msg.fields.len(), 2);

    // control channel
    let control = &protocol.channels[1];
    assert_eq!(control.from, ChannelFrom::Client);
    assert!(control.send.is_some());
    assert!(control.recv.is_some());

    // query channel - with error
    let query = &protocol.channels[2];
    assert_eq!(query.lifetime, ChannelLifetime::Transient);
    assert!(query.error.is_some());

    // chat channel
    let chat = &protocol.channels[3];
    assert_eq!(chat.from, ChannelFrom::Either);
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
