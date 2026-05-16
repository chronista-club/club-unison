mod common;

use serde_json::Value;
use unison::context::handlers::{CompositeHandler, EchoHandler, PingHandler};
use unison::context::{HandlerRegistry, MessageDispatcher};
use unison::network::{MessageType, NetworkError, ProtocolMessage};

/// PingHandler を登録 → ProtocolMessage で dispatch → "Pong: ..." レスポンス検証
#[tokio::test]
async fn test_integ_ping_handler_dispatch() {
    let registry = HandlerRegistry::new();
    registry.register("ping", PingHandler).await;

    let msg = ProtocolMessage::new_with_json(
        1,
        "ping".to_string(),
        MessageType::Request,
        serde_json::json!({"message": "Hello"}),
    )
    .unwrap();

    let result = registry.dispatch(msg).await.unwrap();
    let response_msg = result["message"].as_str().unwrap();
    assert!(response_msg.starts_with("Pong: "));
    assert!(response_msg.contains("Hello"));
}

/// EchoHandler → payload がそのまま返ること
#[tokio::test]
async fn test_integ_echo_handler_dispatch() {
    let registry = HandlerRegistry::new();
    registry.register("echo", EchoHandler).await;

    let payload = serde_json::json!({"data": [1, 2, 3], "nested": {"key": "value"}});
    let msg = ProtocolMessage::new_with_json(
        1,
        "echo".to_string(),
        MessageType::Request,
        payload.clone(),
    )
    .unwrap();

    let result = registry.dispatch(msg).await.unwrap();
    assert_eq!(result, payload);
}

/// 未登録メソッドへの dispatch → HandlerNotFound
#[tokio::test]
async fn test_integ_unregistered_method_dispatch() {
    let registry = HandlerRegistry::new();

    let msg = ProtocolMessage::new_with_json(
        1,
        "nonexistent".to_string(),
        MessageType::Request,
        serde_json::json!({}),
    )
    .unwrap();

    let result = registry.dispatch(msg).await;
    assert!(result.is_err());
    match result.unwrap_err() {
        NetworkError::HandlerNotFound { method } => assert_eq!(method, "nonexistent"),
        e => panic!("Expected HandlerNotFound, got: {:?}", e),
    }
}

/// MessageHandler トレイト経由での dispatch
#[tokio::test]
async fn test_integ_message_handler_trait_dispatch() {
    use unison::context::MessageHandler as MH;

    let registry = HandlerRegistry::new();
    registry.register("echo", EchoHandler).await;

    let msg = ProtocolMessage::new_with_json(
        1,
        "echo".to_string(),
        MessageType::Request,
        serde_json::json!({"test": true}),
    )
    .unwrap();

    let result: Value = MH::handle(&registry, msg).await.unwrap();
    assert_eq!(result["test"], true);
}

/// CompositeHandler のフォールスルー動作
#[tokio::test]
async fn test_integ_composite_handler_fallthrough() {
    let composite = CompositeHandler::new()
        .add_handler(Box::new(EchoHandler))
        .add_handler(Box::new(PingHandler));

    let registry = HandlerRegistry::new();
    registry.register("composite", composite).await;

    let payload = serde_json::json!({"message": "test"});
    let msg = ProtocolMessage::new_with_json(
        1,
        "composite".to_string(),
        MessageType::Request,
        payload.clone(),
    )
    .unwrap();

    let result = registry.dispatch(msg).await.unwrap();
    // EchoHandler が最初にマッチするので payload がそのまま返る
    assert_eq!(result, payload);
}
