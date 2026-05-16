//! CGP統合テスト

use serde_json::Value;
use std::sync::Arc;
use tokio::sync::RwLock;
use unison::context::{
    CgpProtocolContext, Handler, HandlerRegistry, MessageHandler, ServiceRegistry, TransportLayer,
    UnisonContextBuilder,
};
use unison::network::{MessageType, NetworkError, ProtocolMessage};

// ========================================
// モックTransportLayer実装
// ========================================

struct MockTransport {
    connected: std::sync::atomic::AtomicBool,
    messages: Arc<RwLock<Vec<ProtocolMessage>>>,
}

impl MockTransport {
    fn new() -> Self {
        Self {
            connected: std::sync::atomic::AtomicBool::new(false),
            messages: Arc::new(RwLock::new(Vec::new())),
        }
    }
}

impl TransportLayer for MockTransport {
    type Message = ProtocolMessage;
    type Error = NetworkError;

    async fn send(&self, message: Self::Message) -> Result<(), Self::Error> {
        let mut messages = self.messages.write().await;
        messages.push(message);
        Ok(())
    }

    async fn receive(&self) -> Result<Self::Message, Self::Error> {
        let mut messages = self.messages.write().await;
        messages.pop().ok_or(NetworkError::Timeout)
    }

    async fn connect(&self, _url: &str) -> Result<(), Self::Error> {
        self.connected
            .store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }

    async fn disconnect(&self) -> Result<(), Self::Error> {
        self.connected
            .store(false, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected.load(std::sync::atomic::Ordering::SeqCst)
    }
}

// ========================================
// モックServiceRegistry実装
// ========================================

#[derive(Clone)]
struct MockService {
    name: String,
}

struct MockRegistry {
    services: Arc<RwLock<std::collections::HashMap<String, MockService>>>,
}

impl MockRegistry {
    fn new() -> Self {
        Self {
            services: Arc::new(RwLock::new(std::collections::HashMap::new())),
        }
    }
}

impl ServiceRegistry for MockRegistry {
    type Service = MockService;
    type Error = NetworkError;

    async fn register(&self, name: String, service: Self::Service) -> Result<(), Self::Error> {
        let mut services = self.services.write().await;
        services.insert(name, service);
        Ok(())
    }

    async fn get(&self, name: &str) -> Option<Self::Service> {
        let services = self.services.read().await;
        services.get(name).cloned()
    }

    async fn list(&self) -> Vec<String> {
        let services = self.services.read().await;
        services.keys().cloned().collect()
    }

    async fn remove(&self, name: &str) -> Result<(), Self::Error> {
        let mut services = self.services.write().await;
        services.remove(name);
        Ok(())
    }
}

// ========================================
// モックMessageHandler実装
// ========================================

struct MockHandler;

impl MessageHandler for MockHandler {
    type Input = Value;
    type Output = Value;
    type Error = NetworkError;

    async fn handle(&self, input: Self::Input) -> Result<Self::Output, Self::Error> {
        Ok(serde_json::json!({
            "echo": input,
            "processed": true
        }))
    }
}

// ========================================
// テストケース
// ========================================

#[tokio::test]
async fn test_cgp_context_creation() {
    let transport = MockTransport::new();
    let registry = MockRegistry::new();
    let handler = MockHandler;

    let context = CgpProtocolContext::new(transport, registry, handler);

    // コンテキストが正しく作成されることを確認
    assert!(!context.transport().is_connected());
}

#[tokio::test]
async fn test_cgp_context_builder() {
    let transport = MockTransport::new();
    let registry = MockRegistry::new();
    let handler = MockHandler;

    let context = UnisonContextBuilder::new()
        .with_transport(transport)
        .with_registry(registry)
        .with_handler(handler)
        .build();

    assert!(context.is_ok());
}

#[tokio::test]
async fn test_transport_connection() {
    let transport = MockTransport::new();
    let registry = MockRegistry::new();
    let handler = MockHandler;

    let context = CgpProtocolContext::new(transport, registry, handler);

    // 接続前の状態確認
    assert!(!context.transport().is_connected());

    // 接続
    let result = context.transport().connect("test://localhost").await;
    assert!(result.is_ok());
    assert!(context.transport().is_connected());

    // 切断
    let result = context.transport().disconnect().await;
    assert!(result.is_ok());
    assert!(!context.transport().is_connected());
}

#[tokio::test]
async fn test_service_registry() {
    let transport = MockTransport::new();
    let registry = MockRegistry::new();
    let handler = MockHandler;

    let context = CgpProtocolContext::new(transport, registry, handler);

    // サービス登録
    let service = MockService {
        name: "test_service".to_string(),
    };

    let registry = context.registry();
    let result = registry
        .write()
        .await
        .register("test".to_string(), service.clone())
        .await;
    assert!(result.is_ok());

    // サービス取得
    let retrieved = registry.read().await.get("test").await;
    assert!(retrieved.is_some());
    assert_eq!(retrieved.unwrap().name, "test_service");

    // サービス一覧
    let list = registry.read().await.list().await;
    assert_eq!(list.len(), 1);
    assert!(list.contains(&"test".to_string()));

    // サービス削除
    let result = registry.write().await.remove("test").await;
    assert!(result.is_ok());

    let list = registry.read().await.list().await;
    assert_eq!(list.len(), 0);
}

#[tokio::test]
async fn test_message_handler() {
    let transport = MockTransport::new();
    let registry = MockRegistry::new();
    let handler = MockHandler;

    let context = CgpProtocolContext::new(transport, registry, handler);

    let input = serde_json::json!({
        "test": "data",
        "value": 42
    });

    let result = context.handler().handle(input.clone()).await;
    assert!(result.is_ok());

    let output = result.unwrap();
    assert_eq!(output["echo"], input);
    assert_eq!(output["processed"], true);
}

#[tokio::test]
async fn test_handler_registry() {
    use unison::context::MessageDispatcher;

    let registry = HandlerRegistry::new();

    // カスタムハンドラーの定義
    struct TestHandler;

    impl Handler for TestHandler {
        fn handle(
            &self,
            _payload: Value,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<Value, NetworkError>> + Send + '_>,
        > {
            Box::pin(async move {
                Ok(serde_json::json!({
                    "result": "test_success"
                }))
            })
        }
    }

    // ハンドラー登録
    registry.register("test_method", TestHandler).await;

    // メソッド一覧の確認
    let methods = registry.list_methods().await;
    assert!(methods.contains(&"test_method".to_string()));

    // メッセージディスパッチのテスト
    let message = ProtocolMessage::new_with_json(
        1,
        "test_method".to_string(),
        MessageType::Request,
        serde_json::json!({}),
    )
    .unwrap();

    let result = registry.dispatch(message).await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap()["result"], "test_success");
}

#[tokio::test]
async fn test_handler_not_found() {
    use unison::context::MessageDispatcher;

    let registry = HandlerRegistry::new();

    let message = ProtocolMessage::new_with_json(
        1,
        "unknown_method".to_string(),
        MessageType::Request,
        serde_json::json!({}),
    )
    .unwrap();

    let result = registry.dispatch(message).await;
    assert!(result.is_err());

    match result.unwrap_err() {
        NetworkError::HandlerNotFound { method } => {
            assert_eq!(method, "unknown_method");
        }
        _ => panic!("Expected HandlerNotFound error"),
    }
}
