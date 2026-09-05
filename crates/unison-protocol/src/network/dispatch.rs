//! transport 非依存の接続ディスパッチ。
//!
//! raw QUIC と WebTransport の両 ingress が [`handle_connection`] へ収束する。
//! クライアント側でサーバー発信ストリームを捌くループ ([`client_accept_bi_loop`])
//! もここに置く。

use anyhow::Result;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock, oneshot};
use tracing::{debug, error, info, warn};

use super::conn::UnisonConn;
use super::frame::{FRAME_TYPE_PROTOCOL, read_typed_frame, write_channel_ack, write_typed_frame};
use super::stream::UnisonStream;
use super::{NetworkError, ProtocolMessage, context::ConnectionContext, server::ProtocolServer};
use crate::packet::UnisonPacket;

/// client 側 server-initiated channel handler。
///
/// server 側の [`ChannelHandler`](super::server::ChannelHandler) と対称だが、client 側は
/// ctx を持たず raw [`UnisonStream`] のみを受け取る。handler はこの stream を**直読**して
/// reliable に payload を受ける（recv ループ／中継 mpsc を挟まない = 取りこぼし無し）。
pub type ClientServerChannelHandler = Arc<
    dyn Fn(UnisonStream) -> Pin<Box<dyn Future<Output = Result<(), NetworkError>> + Send>>
        + Send
        + Sync,
>;

/// channel 名 → handler の registry（client 側、[`register_server_channel`] で登録）。
///
/// [`register_server_channel`]: super::client::ProtocolClient::register_server_channel
pub type ClientServerChannelRegistry = Arc<RwLock<HashMap<String, ClientServerChannelHandler>>>;

/// クライアント側: サーバー発信の双方向ストリームを受け付けるループ
///
/// サーバーが `connection.open_bi()` で開いたストリームを `accept_bi()` で受信し、
/// **先頭 frame の method** で振り分ける:
/// - `__identity` → Identity 専用の oneshot チャネルへ（従来どおり）。
/// - それ以外（= server-initiated channel）→ `server_channels` registry を引き、登録 handler へ
///   raw [`UnisonStream`] を渡す。宣言 frame は routing で消費済みなので、handler は後続
///   payload を **直読** して reliable に受ける（recv ループ／中継 mpsc を挟まない =
///   遅い consumer には QUIC backpressure が掛かり、取りこぼしも OOM も起きない）。
/// - 未登録 channel → 従来どおり drop + warn（無回帰）。
pub(crate) async fn client_accept_bi_loop(
    connection: quinn::Connection,
    identity_tx: Arc<Mutex<Option<oneshot::Sender<ProtocolMessage>>>>,
    server_channels: ClientServerChannelRegistry,
) {
    loop {
        match connection.accept_bi().await {
            Ok((send_stream, mut recv_stream)) => {
                let identity_tx = identity_tx.clone();
                let server_channels = Arc::clone(&server_channels);
                tokio::spawn(async move {
                    match read_typed_frame(&mut recv_stream).await {
                        Ok((FRAME_TYPE_PROTOCOL, frame_bytes)) => {
                            if let Ok(frame) = UnisonPacket::from_bytes(&frame_bytes)
                                && let Ok(message) = ProtocolMessage::from_frame(&frame)
                            {
                                if message.method == "__identity" {
                                    // Identity メッセージは専用 oneshot チャネルに送信
                                    if let Some(id_tx) = identity_tx.lock().await.take() {
                                        let _ = id_tx.send(message);
                                    } else {
                                        warn!(
                                            "Identity oneshot already consumed, dropping identity message"
                                        );
                                    }
                                } else {
                                    // server-initiated channel: registry を引いて handler へ
                                    // raw UnisonStream を渡す（= server handler と対称、直読 reliable）。
                                    let handler = {
                                        let reg = server_channels.read().await;
                                        reg.get(&message.method).cloned()
                                    };
                                    match handler {
                                        Some(handler) => {
                                            let stream = UnisonStream::from_streams(
                                                0,
                                                message.method.clone(),
                                                Box::new(send_stream),
                                                Box::new(recv_stream),
                                            );
                                            if let Err(e) = handler(stream).await {
                                                if e.is_normal_close() {
                                                    debug!(
                                                        channel = %message.method,
                                                        "server channel closed normally (end of stream)"
                                                    );
                                                } else {
                                                    error!(
                                                        channel = %message.method,
                                                        "server channel handler error: {}",
                                                        e
                                                    );
                                                }
                                            }
                                        }
                                        None => {
                                            // 未登録: 従来どおり drop + warn（無回帰）。
                                            warn!(
                                                method = %message.method,
                                                "no server-channel handler registered; dropping server-initiated stream"
                                            );
                                        }
                                    }
                                }
                            }
                        }
                        Ok((frame_type, _)) => {
                            warn!(
                                "Unexpected frame type in server-initiated stream: 0x{:02x}",
                                frame_type
                            );
                        }
                        Err(e) => {
                            warn!("Failed to read server-initiated stream: {}", e);
                        }
                    }
                });
            }
            Err(quinn::ConnectionError::ApplicationClosed(_)) => {
                info!("Connection closed by server");
                break;
            }
            Err(e) => {
                warn!("Failed to accept server-initiated stream: {}", e);
                break;
            }
        }
    }
}

/// transport 非依存の接続ハンドラー。
///
/// raw QUIC と WebTransport の両 ingress がこの関数へ収束する。 `connection` は
/// [`UnisonConn`] の trait object であり、 この関数は transport の種類を知らない。
pub(crate) async fn handle_connection(
    connection: Arc<dyn UnisonConn>,
    server: Arc<ProtocolServer>,
    ctx: Arc<ConnectionContext>,
) -> Result<()> {
    let remote_addr = connection.remote_address();
    let connection_id = ctx.connection_id;

    // server-initiated stream (= ServerToClient) を handler が開けるよう、ctx に conn を渡す。
    // server 側のみ・1 行（ctx/conn はここで同居しているので call-site ripple ゼロ）。
    ctx.set_conn(Arc::clone(&connection)).await;

    // v0.10.0: active connection に登録 (= server.broadcast の配信先)
    let connection_arc = Arc::clone(&connection);
    server
        .add_active_connection(connection_id, Arc::clone(&connection_arc))
        .await;

    // v0.10.0: datagram dispatcher を 1 connection に 1 個 spawn
    // 登録された datagram channel handler 全てに対し、 channel_id を register して
    // DatagramChannel を構築、 handler を別 task で起動
    let datagram_handlers = server.snapshot_datagram_handlers().await;
    // handler task は接続に紐づくので JoinHandle を掴んでおく。 dispatcher の drop で
    // 止まるのは recv task だけで、 `recv_event` を待たない handler (= 送信専用 /
    // timer loop) は abort しない限り接続終了後も回り続ける。
    let mut datagram_tasks: Vec<tokio::task::JoinHandle<()>> = Vec::new();
    let _datagram_dispatcher = if datagram_handlers.is_empty() {
        // datagram handler が無ければ dispatcher を spawn しない (= overhead 回避)
        None
    } else {
        let dispatcher = Arc::new(super::datagram_dispatcher::DatagramDispatcher::spawn(
            Arc::clone(&connection_arc),
        ));
        for (name, channel_id, handler) in datagram_handlers {
            let rx = dispatcher.register(channel_id, 256).await;
            let datagram_channel = super::datagram_channel::DatagramChannel::<
                crate::codec::JsonCodec,
            >::new(
                Arc::clone(&connection_arc), channel_id, name.clone(), rx
            );
            datagram_tasks.push(tokio::spawn(async move {
                handler(datagram_channel).await;
            }));
        }
        Some(dispatcher)
    };

    // Identity Handshake: 接続直後にServerIdentityを送信
    let identity = server.build_identity().await;
    ctx.set_identity(identity.clone()).await;

    let identity_msg = identity.to_protocol_message();
    match identity_msg.into_frame() {
        Ok(frame) => {
            let frame_bytes = frame.to_bytes();
            match connection.open_bi().await {
                Ok((mut send_stream, _recv_stream)) => {
                    if let Err(e) =
                        write_typed_frame(&mut send_stream, FRAME_TYPE_PROTOCOL, &frame_bytes).await
                    {
                        warn!("Failed to send identity: {}", e);
                    } else {
                        let _ = send_stream.finish().await;
                        info!("Identity sent to client");
                    }
                }
                Err(e) => {
                    warn!("Failed to open identity stream: {}", e);
                }
            }
            // 注: WebTransport セッションにも同一フローが適用される。
        }
        Err(e) => {
            warn!("Failed to serialize identity frame: {}", e);
        }
    }

    // 接続イベントを送信
    server.emit_connection_event(super::server::ConnectionEvent::Connected {
        connection_id,
        remote_addr,
        context: Arc::clone(&ctx),
    });

    loop {
        match connection.accept_bi().await {
            Ok((send_stream, mut recv_stream)) => {
                let server = Arc::clone(&server);
                let ctx = Arc::clone(&ctx);

                tokio::spawn(async move {
                    // typed frame で読み取り（type tag 付き）
                    let request_result = match read_typed_frame(&mut recv_stream).await {
                        Ok((FRAME_TYPE_PROTOCOL, frame_bytes)) => {
                            UnisonPacket::from_bytes(&frame_bytes)
                                .and_then(|frame| ProtocolMessage::from_frame(&frame))
                        }
                        Ok((frame_type, _)) => {
                            warn!("Unexpected frame type in handshake: 0x{:02x}", frame_type);
                            return;
                        }
                        Err(e) => {
                            error!("Failed to read handshake frame: {}", e);
                            return;
                        }
                    };

                    match request_result {
                        Ok(request) => {
                            // チャネルルーティング: __channel: プレフィックスをチェック
                            if let Some(channel_name) = request.method.strip_prefix("__channel:") {
                                let channel_name = channel_name.to_string();
                                let mut send_stream = send_stream;
                                if let Some(handler) =
                                    server.get_channel_handler(&channel_name).await
                                {
                                    // channel lifecycle の "open" 側ログ。
                                    // close 側 (= 下記の debug!) と対になり、 1 接続中の
                                    // channel 開閉 trace が debug level で揃う。
                                    // info level にしない理由: 1 接続で channel が頻繁に
                                    // open/close される設計 (= 1 request/response = 1 channel)
                                    // なので info noise になりがち。
                                    debug!("Channel '{}' opened", channel_name);

                                    // Phase 6c: open frame と同 stream へ open_ack
                                    // (= Response) を 1 本返す。 id は open request の
                                    // id を引き継ぎ、 クライアントが相関できるようにする。
                                    if let Err(e) = write_channel_ack(
                                        &mut send_stream,
                                        request.id,
                                        true,
                                        &channel_name,
                                    )
                                    .await
                                    {
                                        warn!(
                                            "Failed to send open_ack for '{}': {}",
                                            channel_name, e
                                        );
                                        return;
                                    }

                                    // チャネル用のUnisonStreamを作成（ストリームは生きたまま）
                                    let stream = UnisonStream::from_streams(
                                        request.id,
                                        request.method.clone(),
                                        send_stream,
                                        recv_stream,
                                    );
                                    if let Err(e) = handler(ctx, stream).await {
                                        // sender 側が request/response 完了後に正常 close した
                                        // end-of-stream は real error ではないので debug level に
                                        // degrade。 これにより毎 channel session の終端で発生する
                                        // ERROR log noise (= journal で大半を占める) を抑制。
                                        if e.is_normal_close() {
                                            debug!(
                                                "Channel '{}' closed normally (end of stream)",
                                                channel_name
                                            );
                                        } else {
                                            error!(
                                                "Channel handler error for '{}': {}",
                                                channel_name, e
                                            );
                                        }
                                    }
                                } else {
                                    // Phase 6c: 未登録 channel への open は nack
                                    // (= Error frame) を返してから stream を畳む。
                                    // これによりクライアントの open は silent に
                                    // hang せず channel-not-found で即 reject する。
                                    warn!("No channel handler for: {}", channel_name);
                                    if let Err(e) = write_channel_ack(
                                        &mut send_stream,
                                        request.id,
                                        false,
                                        &channel_name,
                                    )
                                    .await
                                    {
                                        warn!(
                                            "Failed to send open nack for '{}': {}",
                                            channel_name, e
                                        );
                                    } else {
                                        let _ = send_stream.finish().await;
                                    }
                                }
                                return;
                            }

                            // 非チャネルメッセージはサポート外
                            warn!(
                                "Non-channel message received (method: {}). Use channels instead.",
                                request.method
                            );
                        }
                        Err(e) => {
                            warn!("Failed to parse message: {}", e);
                        }
                    }
                });
            }
            Err(e) => {
                // accept_bi の Err = 接続終了 (= 正常な切断もエラー扱いで来る)。
                // transport を問わず接続ループを抜ける。
                info!("Connection closed ({}), client disconnected", e);
                server.emit_connection_event(super::server::ConnectionEvent::Disconnected {
                    connection_id,
                    remote_addr,
                });
                break;
            }
        }
    }

    // 接続終了時の後始末。 台帳から外して broadcast 配信先から除外し、 この接続に
    // 紐づく datagram handler task を明示的に abort する (dispatcher 自身の recv task は
    // `_datagram_dispatcher` の scope-exit drop で止まる)。
    server.remove_active_connection(connection_id).await;
    for task in datagram_tasks {
        task.abort();
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    /// 接続が終わったら datagram channel handler の task も止まること。
    ///
    /// handler は `recv_event` 以外 (= timer loop や送信専用) で回っていることが
    /// あり、 その場合 dispatcher の recv task を abort しても handler は生き残る。
    /// `handle_connection` は spawn した handler task を掴んでおき、 接続終了時に
    /// abort しなければならない。
    #[tokio::test]
    async fn datagram_handler_tasks_stop_when_connection_ends() {
        use super::super::test_support::MockConn;

        let ticks = Arc::new(AtomicUsize::new(0));
        let ticks_for_handler = Arc::clone(&ticks);

        let server = Arc::new(ProtocolServer::new());
        server
            .register_channel_datagram("ticker", 1, move |_chan| {
                let ticks = Arc::clone(&ticks_for_handler);
                async move {
                    // recv を待たずに回り続ける handler (= 送信専用 / timer loop)
                    loop {
                        ticks.fetch_add(1, Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                }
            })
            .await;

        let conn: Arc<dyn UnisonConn> = Arc::new(MockConn::new("127.0.0.1:4433".parse().unwrap()));
        let ctx = Arc::new(ConnectionContext::new());

        // MockConn の accept_bi は即 Err を返すので、 handle_connection は
        // identity 送信を試みたあと 1 周で抜ける。
        handle_connection(conn, Arc::clone(&server), ctx)
            .await
            .expect("handle_connection completes");

        // handler が確実に 1 回以上回ってから、 停止したかを見る
        tokio::time::sleep(Duration::from_millis(30)).await;
        let after_close = ticks.load(Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(50)).await;
        let later = ticks.load(Ordering::SeqCst);

        assert_eq!(
            later, after_close,
            "接続終了後も datagram handler task が回り続けている (task leak)"
        );
    }
}
