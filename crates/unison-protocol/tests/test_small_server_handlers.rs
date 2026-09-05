mod common;

use unison::network::ProtocolServer;

#[tokio::test]
async fn test_integ_register_and_get_channel_handler() {
    let server = ProtocolServer::new();
    server
        .register_channel("ping", |_ctx, _stream| async { Ok(()) })
        .await;
    let handler = server.get_channel_handler("ping").await;
    assert!(handler.is_some());
}

#[tokio::test]
async fn test_integ_unregistered_channel_returns_none() {
    let server = ProtocolServer::new();
    let handler = server.get_channel_handler("nonexistent").await;
    assert!(handler.is_none());
}

#[tokio::test]
async fn test_integ_multiple_channels_build_identity() {
    let server = ProtocolServer::new();
    server
        .register_channel("ping", |_ctx, _stream| async { Ok(()) })
        .await;
    server
        .register_channel("events", |_ctx, _stream| async { Ok(()) })
        .await;
    server
        .register_channel("query", |_ctx, _stream| async { Ok(()) })
        .await;

    let identity = server.build_identity().await;
    let channel_names: Vec<&str> = identity.channels.iter().map(|c| c.name.as_str()).collect();
    assert!(channel_names.contains(&"ping"));
    assert!(channel_names.contains(&"events"));
    assert!(channel_names.contains(&"query"));
    assert_eq!(identity.channels.len(), 3);
}

#[tokio::test]
async fn test_integ_with_identity_metadata() {
    let server = ProtocolServer::with_identity("my-server", "2.0.0", "production");
    let identity = server.build_identity().await;
    assert_eq!(identity.name, "my-server");
    assert_eq!(identity.version, "2.0.0");
    assert_eq!(identity.namespace, "production");
}

// emit_connection_event は pub(crate) のため統合テストから直接呼べない。
// subscribe_connection_events が正常に Receiver を返すことを検証する。
#[tokio::test]
async fn test_integ_connection_event_subscribe() {
    let server = ProtocolServer::new();
    let _rx = server.subscribe_connection_events();
    // subscribe が正常に動作すること（型レベル確認）
}

// 複数サブスクライバが同時に subscribe できること。
// sender が drop された後、recv が Err を返すことを検証。
#[tokio::test]
async fn test_integ_multiple_subscribers() {
    let server = ProtocolServer::new();
    let mut rx1 = server.subscribe_connection_events();
    let mut rx2 = server.subscribe_connection_events();

    // server を drop して broadcast sender を閉じる
    drop(server);

    // sender が drop されたので recv は RecvError になる（lagged or closed）
    assert!(rx1.recv().await.is_err());
    assert!(rx2.recv().await.is_err());
}
