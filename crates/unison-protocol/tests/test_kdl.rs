use club_unison::prelude::*;

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
