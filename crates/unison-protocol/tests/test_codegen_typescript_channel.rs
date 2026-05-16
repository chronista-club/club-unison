//! v0.11.0 Phase 1: TS generator が channel block を扱えること verification
//!
//! v0.9.0 で導入された Unified Channel narrative を TS gen が catch up したことを
//! string-contains assertions で検証。 既存 test_codegen_channel.rs (= rust gen 用)
//! と並列の TS gen 版。

use unison::codegen::CodeGenerator;
use unison::parser::TypeRegistry;
use unison::prelude::*;

/// Stream channel (= default backend) の TS gen output が event / request / returns 型
/// + ChannelMeta const を含む
#[test]
fn test_typescript_codegen_stream_channel() {
    let schema = r#"
        protocol "test-stream" version="1.0.0" {
            namespace "test.stream"

            channel "query" from="client" lifetime="persistent" {
                request "Query" {
                    field "method" type="string" required=#true
                    field "params" type="json"

                    returns "Result" {
                        field "data" type="json"
                    }
                }

                event "QueryError" {
                    field "code" type="string"
                    field "message" type="string"
                }
            }
        }
    "#;

    let parser = SchemaParser::new();
    let parsed = parser.parse(schema).unwrap();
    let type_registry = TypeRegistry::new();
    let generator = TypeScriptGenerator::new();
    let code = generator.generate(&parsed, &type_registry).unwrap();

    // Section header
    assert!(
        code.contains("Channel: query"),
        "channel section header expected: \n{}",
        code
    );
    assert!(code.contains("backend=stream"), "backend=stream tag");

    // Request 型 interface
    assert!(
        code.contains("export interface Query {"),
        "Query request interface"
    );
    assert!(code.contains("method"), "Query field");

    // Returns 型 interface
    assert!(
        code.contains("export interface Result {"),
        "Result response interface"
    );
    assert!(code.contains("data"), "Result field");

    // Event 型 interface
    assert!(
        code.contains("export interface QueryError {"),
        "QueryError event interface"
    );

    // Channel metadata const
    assert!(
        code.contains("export const QueryChannelMeta"),
        "QueryChannelMeta const"
    );
    assert!(
        code.contains("backend: \"stream\""),
        "metadata backend=stream"
    );
    assert!(code.contains("from: \"client\""), "metadata from=client");
    assert!(
        code.contains("lifetime: \"persistent\""),
        "metadata lifetime=persistent"
    );
    assert!(
        code.contains("events:") && code.contains("QueryError"),
        "metadata events list"
    );
    assert!(
        code.contains("requests:") && code.contains("Query"),
        "metadata requests mapping"
    );
    assert!(
        code.contains("response: \"Result\""),
        "metadata response type name"
    );

    // v0.11.0 beta-freeze: type-map interfaces + phantom `__types` carrier
    // (= SDK の EventType<M> / RequestType<M,N> / ResponseType<M,N> 解決元)
    assert!(
        code.contains("export interface QueryChannelEventTypes {"),
        "event type-map interface"
    );
    assert!(
        code.contains("QueryError: QueryError;"),
        "event type-map entry maps name → interface"
    );
    assert!(
        code.contains("export interface QueryChannelRequestTypes {"),
        "request type-map interface"
    );
    assert!(
        code.contains("Query: { request: Query; response: Result };"),
        "request type-map entry maps name → request/response interfaces"
    );
    assert!(
        code.contains("__types: undefined as unknown as { events: QueryChannelEventTypes; requests: QueryChannelRequestTypes }"),
        "meta carries phantom __types carrier"
    );
}

/// Datagram channel の TS gen output が channel_id を metadata に含む、 event only
#[test]
fn test_typescript_codegen_datagram_channel() {
    let schema = r#"
        protocol "test-datagram" version="1.0.0" {
            namespace "test.datagram"

            channel "position" from="server" lifetime="persistent" backend="datagram" channel_id=1 {
                event "Transform" {
                    field "id" type="string"
                    field "pos" type="json"
                    field "rot" type="json"
                }
            }
        }
    "#;

    let parser = SchemaParser::new();
    let parsed = parser.parse(schema).unwrap();
    let type_registry = TypeRegistry::new();
    let generator = TypeScriptGenerator::new();
    let code = generator.generate(&parsed, &type_registry).unwrap();

    // Section header に backend=datagram + channel_id
    assert!(code.contains("Channel: position"));
    assert!(code.contains("backend=datagram"));
    assert!(code.contains("channel_id=1"));

    // Event interface
    assert!(
        code.contains("export interface Transform {"),
        "Transform event"
    );
    assert!(code.contains("id"), "Transform.id");
    assert!(code.contains("pos"), "Transform.pos");
    assert!(code.contains("rot"), "Transform.rot");

    // ChannelMeta const に channelId
    assert!(
        code.contains("export const PositionChannelMeta"),
        "PositionChannelMeta const"
    );
    assert!(
        code.contains("backend: \"datagram\""),
        "metadata backend=datagram"
    );
    assert!(code.contains("channelId: 1"), "metadata channelId=1");
    assert!(code.contains("from: \"server\""), "metadata from=server");

    // Datagram channel は requests 空
    assert!(
        code.contains("requests: {} as const"),
        "datagram channel has no requests"
    );

    // Events list に Transform
    assert!(code.contains("events: [\"Transform\"]"), "events list");

    // Type-map: event interface + 空 request map
    assert!(
        code.contains("export interface PositionChannelEventTypes {"),
        "datagram event type-map interface"
    );
    assert!(
        code.contains("Transform: Transform;"),
        "datagram event type-map entry"
    );
    assert!(
        code.contains("export type PositionChannelRequestTypes = Record<string, never>;"),
        "datagram channel has empty request type-map"
    );
    assert!(
        code.contains("__types: undefined as unknown as { events: PositionChannelEventTypes; requests: PositionChannelRequestTypes }"),
        "datagram meta carries phantom __types carrier"
    );
}

/// 混在 schema: stream + datagram channel が並列で生成される
#[test]
fn test_typescript_codegen_mixed_channels() {
    let schema = r#"
        protocol "test-mixed" version="1.0.0" {
            namespace "test.mixed"

            channel "control" from="either" lifetime="persistent" {
                request "Subscribe" {
                    field "topic" type="string"
                    returns "Subscribed" { field "ok" type="bool" }
                }
            }

            channel "position" from="server" lifetime="persistent" backend="datagram" channel_id=1 {
                event "Transform" { field "id" type="string" }
            }
        }
    "#;

    let parser = SchemaParser::new();
    let parsed = parser.parse(schema).unwrap();
    let type_registry = TypeRegistry::new();
    let generator = TypeScriptGenerator::new();
    let code = generator.generate(&parsed, &type_registry).unwrap();

    // Both channels present
    assert!(code.contains("Channel: control"));
    assert!(code.contains("Channel: position"));

    // Stream channel metadata
    assert!(code.contains("export const ControlChannelMeta"));
    assert!(code.contains("backend: \"stream\""));

    // Datagram channel metadata
    assert!(code.contains("export const PositionChannelMeta"));
    assert!(code.contains("backend: \"datagram\""));
    assert!(code.contains("channelId: 1"));

    // Types
    assert!(code.contains("export interface Subscribe {"));
    assert!(code.contains("export interface Subscribed {"));
    assert!(code.contains("export interface Transform {"));
}

/// v0.9.0 互換: backend 属性なしの schema は default stream として扱う
#[test]
fn test_typescript_codegen_backend_default_stream() {
    let schema = r#"
        protocol "test-default" version="1.0.0" {
            channel "events" from="server" lifetime="persistent" {
                event "Update" { field "value" type="string" }
            }
        }
    "#;

    let parser = SchemaParser::new();
    let parsed = parser.parse(schema).unwrap();
    let type_registry = TypeRegistry::new();
    let generator = TypeScriptGenerator::new();
    let code = generator.generate(&parsed, &type_registry).unwrap();

    assert!(code.contains("backend=stream"), "default backend=stream");
    assert!(
        code.contains("backend: \"stream\""),
        "metadata backend stream"
    );
    assert!(
        !code.contains("channelId"),
        "stream channel has no channelId"
    );
}

/// Empty channel: events / requests どちらもない channel
#[test]
fn test_typescript_codegen_empty_channel() {
    let schema = r#"
        protocol "test-empty" version="1.0.0" {
            channel "ping" from="client" lifetime="transient" {
            }
        }
    "#;

    let parser = SchemaParser::new();
    let parsed = parser.parse(schema).unwrap();
    let type_registry = TypeRegistry::new();
    let generator = TypeScriptGenerator::new();
    let code = generator.generate(&parsed, &type_registry).unwrap();

    assert!(code.contains("export const PingChannelMeta"));
    assert!(code.contains("events: [] as const"));
    assert!(code.contains("requests: {} as const"));

    // empty channel は両 type-map が Record<string, never>
    assert!(
        code.contains("export type PingChannelEventTypes = Record<string, never>;"),
        "empty channel event type-map"
    );
    assert!(
        code.contains("export type PingChannelRequestTypes = Record<string, never>;"),
        "empty channel request type-map"
    );
}
