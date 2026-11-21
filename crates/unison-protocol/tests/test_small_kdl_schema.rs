use unison::parser::{ChannelBackend, ChannelFrom, ChannelLifetime, FieldType, SchemaParser};

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

/// request の safety hint (`readonly` / `destructive` / `idempotent`) がパースされる
#[test]
fn test_request_safety_hints_parsed() {
    let schema = r#"
        protocol "test" version="1.0.0" {
            channel "memory" from="client" lifetime="persistent" {
                request "Query" readonly=#true idempotent=#true {
                    field "key" type="string"
                    returns "Result" { field "value" type="json" }
                }
                request "Delete" destructive=#true {
                    field "key" type="string"
                    returns "Deleted" { field "ok" type="bool" }
                }
            }
        }
    "#;
    let parser = SchemaParser::new();
    let protocol = parser.parse(schema).unwrap().protocol.unwrap();
    let ch = &protocol.channels[0];

    let query = &ch.requests[0];
    assert_eq!(query.readonly, Some(true));
    assert_eq!(query.idempotent, Some(true));
    assert_eq!(query.destructive, None);

    let delete = &ch.requests[1];
    assert_eq!(delete.destructive, Some(true));
    assert_eq!(delete.readonly, None);
    assert_eq!(delete.idempotent, None);
}

/// safety hint 未宣言の request は全 hint が None (= 従来 schema 互換)
#[test]
fn test_request_without_safety_hints_defaults_to_none() {
    let schema = r#"
        protocol "test" version="1.0.0" {
            channel "chat" from="client" lifetime="persistent" {
                request "Send" {
                    field "text" type="string"
                    returns "Sent" { field "ok" type="bool" }
                }
            }
        }
    "#;
    let parser = SchemaParser::new();
    let protocol = parser.parse(schema).unwrap().protocol.unwrap();
    let req = &protocol.channels[0].requests[0];
    assert_eq!(req.readonly, None);
    assert_eq!(req.destructive, None);
    assert_eq!(req.idempotent, None);
}

/// `readonly=#true` と `destructive=#true` の同時宣言は矛盾 → validation error
#[test]
fn test_request_readonly_and_destructive_conflict_fails() {
    let schema = r#"
        protocol "test" version="1.0.0" {
            channel "memory" from="client" lifetime="persistent" {
                request "Broken" readonly=#true destructive=#true {
                    field "key" type="string"
                }
            }
        }
    "#;
    let parser = SchemaParser::new();
    let err = parser
        .parse(schema)
        .expect_err("readonly + destructive must fail validation");
    let msg = format!("{}", err);
    assert!(
        msg.contains("readonly") && msg.contains("destructive"),
        "error must mention the conflicting hints: {}",
        msg
    );
}

/// `type="array" items="Atlas"` は要素型を持つ Array として読む。
///
/// 要素型が読めないと「配った client の約束を壊していないか」の差分検査
/// (= `unison schema-diff`) で要素型の入れ替わりを見逃す。
#[test]
fn test_array_items_element_type_parsed() {
    let schema_str = r#"
protocol "t" version="1.0.0" {
    channel "c" from="client" lifetime="persistent" {
        request "R" {
            field "ids" type="array" items="string"
            returns "Res" {
                field "atlases" type="array" items="Atlas" required=#true
            }
        }
    }
}
"#;
    let schema = SchemaParser::new().parse(schema_str).expect("parse");
    let protocol = schema.protocol.unwrap();
    let req = &protocol.channels[0].requests[0];

    let FieldType::Array(elem) = req.fields[0].field_type() else {
        panic!("ids should be Array, got {:?}", req.fields[0].field_type());
    };
    assert!(matches!(*elem, FieldType::String), "got {elem:?}");

    let returns = req.returns.as_ref().unwrap();
    let FieldType::Array(elem) = returns.fields[0].field_type() else {
        panic!("atlases should be Array");
    };
    assert!(
        matches!(*elem, FieldType::Custom(ref n) if n == "Atlas"),
        "got {elem:?}"
    );
}

/// `type="map" keys="string" values="Label"` は K/V 型を持つ Map として読む。
/// `keys` 省略時は `string` (= JSON object のキーは常に文字列)。
#[test]
fn test_map_key_value_types_parsed() {
    let schema_str = r#"
protocol "t" version="1.0.0" {
    channel "c" from="client" lifetime="persistent" {
        request "R" {
            field "labels" type="map" values="Label"
            returns "Res" {
                field "ok" type="bool"
            }
        }
    }
}
"#;
    let schema = SchemaParser::new().parse(schema_str).expect("parse");
    let protocol = schema.protocol.unwrap();
    let FieldType::Map(key, value) = protocol.channels[0].requests[0].fields[0].field_type() else {
        panic!("labels should be Map");
    };
    assert!(matches!(*key, FieldType::String), "got {key:?}");
    assert!(
        matches!(*value, FieldType::Custom(ref n) if n == "Label"),
        "got {value:?}"
    );
}

/// 要素型を書かない従来の `type="array"` / `type="map"` は従来どおり任意要素。
#[test]
fn test_array_and_map_without_element_types_stay_untyped() {
    let schema_str = r#"
protocol "t" version="1.0.0" {
    channel "c" from="client" lifetime="persistent" {
        request "R" {
            field "xs" type="array"
            field "m" type="map"
            returns "Res" {
                field "ok" type="bool"
            }
        }
    }
}
"#;
    let schema = SchemaParser::new().parse(schema_str).expect("parse");
    let protocol = schema.protocol.unwrap();
    let fields = &protocol.channels[0].requests[0].fields;
    assert!(matches!(fields[0].field_type(), FieldType::Array(e) if matches!(*e, FieldType::Json)),);
    assert!(matches!(fields[1].field_type(), FieldType::Map(k, v)
            if matches!(*k, FieldType::String) && matches!(*v, FieldType::Json)),);
}

// ---- 知らないノード ----
// club-kdl の derive は名前の一致する子ノードだけを拾い、 残りを黙って捨てる。
// 旧構文 (service / send / recv) やタイプミスが「空の channel」として読まれないよう、
// SchemaParser はその位置に書けない名前のノードを弾く。

fn parse_err(src: &str) -> String {
    SchemaParser::new()
        .parse(src)
        .expect_err("知らないノードは弾くべき")
        .to_string()
}

#[test]
fn rejects_unknown_node_in_channel() {
    let err = parse_err(
        r#"
protocol "x" version="1.0.0" {
    channel "users" from="client" lifetime="persistent" {
        send "CreateUser" {
            field "name" type="string"
        }
    }
}
"#,
    );
    assert!(err.contains(r#"unknown node "send""#), "{err}");
    assert!(err.contains(r#"protocol "x" > channel "users""#), "{err}");
    assert!(err.contains("allowed: request, event"), "{err}");
}

#[test]
fn rejects_unknown_top_level_node() {
    let err = parse_err(
        r#"
service "Old" {
    method "Ping" {}
}
"#,
    );
    assert!(err.contains(r#"unknown node "service""#), "{err}");
    assert!(err.contains("top level"), "{err}");
}

#[test]
fn rejects_misspelled_node() {
    let err = parse_err(
        r#"
protocol "x" version="1.0.0" {
    channel "users" from="client" lifetime="persistent" {
        requst "Get" {
            field "id" type="string"
        }
    }
}
"#,
    );
    assert!(err.contains(r#"unknown node "requst""#), "{err}");
}

#[test]
fn rejects_unknown_node_deep_in_returns() {
    let err = parse_err(
        r#"
protocol "x" version="1.0.0" {
    channel "users" from="client" lifetime="persistent" {
        request "Get" {
            field "id" type="string"
            returns "User" {
                field "name" type="string"
                extra "x"
            }
        }
    }
}
"#,
    );
    assert!(err.contains(r#"unknown node "extra""#), "{err}");
    assert!(
        err.contains(r#"channel "users" > request "Get" > returns "User""#),
        "{err}"
    );
    assert!(err.contains("allowed: field"), "{err}");
}

#[test]
fn rejects_child_nodes_under_field() {
    let err = parse_err(
        r#"
protocol "x" version="1.0.0" {
    channel "users" from="client" lifetime="persistent" {
        request "Get" {
            field "id" type="string" {
                note "x"
            }
        }
    }
}
"#,
    );
    assert!(err.contains(r#"unknown node "note""#), "{err}");
    assert!(err.contains(r#"> field "id""#), "{err}");
}

#[test]
fn slashdash_commented_nodes_are_not_unknown() {
    let schema = SchemaParser::new()
        .parse(
            r#"
protocol "x" version="1.0.0" {
    channel "users" from="client" lifetime="persistent" {
        /- send "Old" {
            field "name" type="string"
        }
        request "Get" {}
    }
}
"#,
        )
        .expect("/- でコメントアウトしたノードは KDL 上ノードではない");
    assert_eq!(schema.protocol.unwrap().channels[0].requests.len(), 1);
}

// ---- 他の方言の約束 (parse_contract) ----
// schema-check / schema-diff は、 Unison の語彙に無い node (例: Creo の REST の `route`) を
// top level に持つ約束も読む。 読み飛ばすのは top level の知らない node だけで、
// Unison の node の中は parse と同じく厳密に見る。

const FOREIGN_CONTRACT: &str = r#"
message "User" {
    field "id" type="string" required=#true
}

route "GET" "/api/users" response="User" {
    query "page" type="int"
}
"#;

#[test]
fn parse_rejects_foreign_top_level_node() {
    let err = parse_err(FOREIGN_CONTRACT);
    assert!(err.contains(r#"unknown node "route""#), "{err}");
}

#[test]
fn parse_contract_skips_foreign_top_level_nodes() {
    let schema = SchemaParser::new()
        .parse_contract(FOREIGN_CONTRACT)
        .expect("top level の知らない node は読み飛ばす");
    assert_eq!(schema.messages.len(), 1);
    assert_eq!(schema.messages[0].name, "User");
}

#[test]
fn parse_contract_is_still_strict_inside_unison_nodes() {
    let err = SchemaParser::new()
        .parse_contract(
            r#"
message "User" {
    field "id" type="string"
    feild "name" type="string"
}
"#,
        )
        .expect_err("Unison の node の中の知らない node は弾く")
        .to_string();
    assert!(err.contains(r#"unknown node "feild""#), "{err}");
    assert!(err.contains(r#"message "User""#), "{err}");
}

// ---- 受け取った schema (parse_lenient) ----
// discovery で相手の server から届いた schema は、 この版が知らない node をどの位置でも
// 読み飛ばす (= 新しい版の server に、 古い client が繋げなくならないように)。

#[test]
fn parse_lenient_skips_unknown_nodes_at_any_depth() {
    let schema = SchemaParser::new()
        .parse_lenient(
            r#"
protocol "x" version="1.0.0" {
    capability "streams"
    channel "users" from="client" lifetime="persistent" {
        stream "Upload" {}
        request "Get" {
            field "id" type="string" {
                note "x"
            }
        }
    }
}
extension "y" {}
"#,
        )
        .expect("知らない node は読み飛ばす");
    let protocol = schema.protocol.unwrap();
    assert_eq!(protocol.channels[0].requests.len(), 1);
    assert_eq!(protocol.channels[0].requests[0].fields.len(), 1);
}

#[test]
fn parse_lenient_still_rejects_broken_kdl_and_invalid_channels() {
    let parser = SchemaParser::new();
    assert!(parser.parse_lenient("not valid kdl }{").is_err());
    // semantic な検証 (datagram channel の channel_id 必須) は緩めない
    let err = parser
        .parse_lenient(
            r#"
protocol "x" version="1.0.0" {
    channel "pos" from="server" lifetime="persistent" backend="datagram" {
        event "Tick" {}
    }
}
"#,
        )
        .unwrap_err();
    assert!(
        matches!(err, unison::parser::ParseError::Validation(_)),
        "{err}"
    );
}
