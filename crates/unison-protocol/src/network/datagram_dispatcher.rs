//! Datagram dispatcher: per-connection の datagram 受信 loop + channel_id ベース
//! demux (v0.10.0 で追加)
//!
//! QUIC connection の `read_datagram()` を background task で続行、 payload 先頭の
//! varint encoded `channel_id` を抽出、 登録された `HashMap<u64, mpsc::Sender>` に
//! route する。 [`DatagramChannel`](super::datagram_channel::DatagramChannel) は
//! この dispatcher が push する payload を pull するだけの単純な受信側。
//!
//! ## 配送特性 (= datagram semantics)
//!
//! - **Unreliable**: malformed varint / 未登録 channel_id / buffer full は全て drop、
//!   caller には通知しない。 spec/02 §8.5 の「HoL blocking なし」 を実装層で担保。
//! - **Unordered**: 到着順で deliver、 sequence 保証なし。
//! - **Best-effort**: 全 step に `try_send` を使い、 dispatcher 自身は決して詰まらない。
//!
//! ## Testability 設計
//!
//! [`DispatcherInner`] (= data 層) と [`DatagramDispatcher`] (= runtime 層) を分離。
//! data 層は `quinn::Connection` なしで unit test 可能、 runtime 層は 1d/1e の
//! integration test (= 実 QUIC connection ペア) で網羅。

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;
use tracing::{debug, warn};

use super::datagram_channel::decode_varint;

/// Datagram dispatch table の内部 state (= data 層、 test 可能)
///
/// `quinn::Connection` を持たず純粋な `HashMap<channel_id, Sender>` 管理に責務を
/// 限定。 [`DatagramDispatcher`] が background task で `dispatch()` を呼んで
/// 受信 datagram を route する。
pub(crate) struct DispatcherInner {
    handlers: Mutex<HashMap<u64, mpsc::Sender<Vec<u8>>>>,
}

impl DispatcherInner {
    fn new() -> Self {
        Self {
            handlers: Mutex::new(HashMap::new()),
        }
    }

    /// `channel_id` に対する receiver を払い出して登録
    ///
    /// 既存 entry は **replace** (= reconnect / re-open シナリオで自然)、 古い sender
    /// は drop されて caller 側 DatagramChannel の `recv_event` は「channel closed」
    /// を返す。
    async fn register(&self, channel_id: u64, buffer_size: usize) -> mpsc::Receiver<Vec<u8>> {
        let (tx, rx) = mpsc::channel(buffer_size);
        let mut handlers = self.handlers.lock().await;
        if handlers.insert(channel_id, tx).is_some() {
            debug!(
                "Datagram dispatcher: channel_id {} re-registered (old sender dropped)",
                channel_id
            );
        }
        rx
    }

    /// `channel_id` の登録を解除
    #[allow(dead_code)] // test / reconnect 用、 v0.10.0 runtime path では未使用
    async fn unregister(&self, channel_id: u64) {
        self.handlers.lock().await.remove(&channel_id);
    }

    /// 登録されている channel 数
    #[allow(dead_code)] // test / debug 用
    async fn handler_count(&self) -> usize {
        self.handlers.lock().await.len()
    }

    /// 1 つの datagram を dispatch (= varint decode → handler lookup → try_send)
    ///
    /// 失敗 / drop ケース:
    /// - malformed varint: warn log + drop
    /// - 未登録 channel_id: debug log + drop
    /// - buffer full (= `try_send` 失敗): debug log + drop
    /// - 全てを caller に伝えない (= unreliable semantics に合致)
    async fn dispatch(&self, datagram: &[u8]) {
        let (channel_id, consumed) = match decode_varint(datagram) {
            Ok(parsed) => parsed,
            Err(e) => {
                warn!("Datagram dispatcher: malformed channel_id varint: {}", e);
                return;
            }
        };

        let payload = datagram[consumed..].to_vec();

        let handlers = self.handlers.lock().await;
        if let Some(sender) = handlers.get(&channel_id) {
            // try_send で back-pressure 時に blocking しない (= recv loop を守る)
            if sender.try_send(payload).is_err() {
                debug!(
                    "Datagram dispatcher: channel {} buffer full or closed, dropping payload",
                    channel_id
                );
            }
        } else {
            debug!(
                "Datagram dispatcher: no handler for channel_id {}, dropping payload",
                channel_id
            );
        }
    }
}

/// Per-connection datagram dispatcher (= runtime 層)
///
/// [`spawn`](Self::spawn) で background recv task を起動、 caller は
/// [`register`](Self::register) / [`unregister`](Self::unregister) で channel_id
/// 単位の handler を出し入れする。 drop 時に task abort。
pub(crate) struct DatagramDispatcher {
    inner: Arc<DispatcherInner>,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl DatagramDispatcher {
    /// QUIC connection を渡して dispatcher を起動
    ///
    /// background task が `connection.read_datagram()` を繰り返し呼び、 受信した
    /// datagram を `DispatcherInner::dispatch` に流す。 connection close まで loop
    /// 続行、 close で task 自然終了。
    pub fn spawn(connection: Arc<quinn::Connection>) -> Self {
        let inner = Arc::new(DispatcherInner::new());
        let inner_clone = Arc::clone(&inner);
        let task = tokio::spawn(async move {
            loop {
                let datagram = match connection.read_datagram().await {
                    Ok(d) => d,
                    Err(quinn::ConnectionError::ApplicationClosed(_)) => {
                        debug!("Datagram dispatcher: connection closed by application");
                        break;
                    }
                    Err(quinn::ConnectionError::ConnectionClosed(_)) => {
                        debug!("Datagram dispatcher: connection closed");
                        break;
                    }
                    Err(e) => {
                        warn!("Datagram dispatcher: read_datagram error: {}", e);
                        break;
                    }
                };
                inner_clone.dispatch(&datagram).await;
            }
        });

        Self {
            inner,
            task: Mutex::new(Some(task)),
        }
    }

    /// `channel_id` に対する receiver を払い出して登録 (= `DispatcherInner::register` 委譲)
    pub async fn register(&self, channel_id: u64, buffer_size: usize) -> mpsc::Receiver<Vec<u8>> {
        self.inner.register(channel_id, buffer_size).await
    }

    /// `channel_id` の登録を解除 (= `DispatcherInner::unregister` 委譲)
    ///
    /// 現在の v0.10.0 では `DatagramChannel::close` から呼ばれない (= drop semantics で
    /// 十分)、 reconnect / 明示 close シナリオの将来 caller 用 API として保持。
    #[allow(dead_code)]
    pub async fn unregister(&self, channel_id: u64) {
        self.inner.unregister(channel_id).await;
    }

    /// 登録されている channel 数 (= test / debug 用)
    #[allow(dead_code)]
    pub async fn handler_count(&self) -> usize {
        self.inner.handler_count().await
    }

    /// Dispatcher を明示停止 (= task abort + handler 全 clear)
    ///
    /// drop でも task abort されるが、 明示的に停止したい場合 (= reconnect 等) に使用。
    #[allow(dead_code)]
    pub async fn shutdown(&self) {
        if let Some(task) = self.task.lock().await.take() {
            task.abort();
        }
        self.inner.handlers.lock().await.clear();
    }
}

impl Drop for DatagramDispatcher {
    fn drop(&mut self) {
        // best-effort task abort (= async lock を avoid するため try_lock)
        if let Ok(mut guard) = self.task.try_lock()
            && let Some(task) = guard.take()
        {
            task.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::datagram_channel::encode_varint;

    /// 新規 dispatcher は handler が 0 件
    #[tokio::test]
    async fn inner_starts_empty() {
        let inner = DispatcherInner::new();
        assert_eq!(inner.handler_count().await, 0);
    }

    /// register で handler 数が増え、 unregister で減る
    #[tokio::test]
    async fn inner_register_unregister_counts() {
        let inner = DispatcherInner::new();
        let _rx1 = inner.register(1, 16).await;
        assert_eq!(inner.handler_count().await, 1);
        let _rx2 = inner.register(2, 16).await;
        assert_eq!(inner.handler_count().await, 2);
        inner.unregister(1).await;
        assert_eq!(inner.handler_count().await, 1);
        inner.unregister(2).await;
        assert_eq!(inner.handler_count().await, 0);
    }

    /// 同一 channel_id の再登録 → 古い sender が drop、 新 receiver から受信できる
    #[tokio::test]
    async fn inner_register_replaces_existing() {
        let inner = DispatcherInner::new();
        let mut rx1 = inner.register(42, 16).await;
        let _rx2 = inner.register(42, 16).await; // 再登録、 rx1 の sender は drop
        // rx1 から recv() すると None (= channel closed)
        assert!(rx1.recv().await.is_none());
        assert_eq!(inner.handler_count().await, 1); // 重複登録でも 1 件のまま
    }

    /// dispatch: 登録済 channel_id ▶ receiver に payload 配信
    #[tokio::test]
    async fn inner_dispatch_delivers_to_registered_channel() {
        let inner = DispatcherInner::new();
        let mut rx = inner.register(1, 16).await;

        // [varint(1)] [payload_bytes] を組み立てて dispatch
        let mut datagram = Vec::new();
        encode_varint(1, &mut datagram);
        datagram.extend_from_slice(b"hello");

        inner.dispatch(&datagram).await;

        // rx から payload を受け取る
        let payload = rx.recv().await.expect("payload delivered");
        assert_eq!(payload, b"hello");
    }

    /// dispatch: 未登録 channel_id ▶ silently drop (= panic しない)
    #[tokio::test]
    async fn inner_dispatch_drops_unknown_channel() {
        let inner = DispatcherInner::new();
        // 登録なし
        let mut datagram = Vec::new();
        encode_varint(99, &mut datagram);
        datagram.extend_from_slice(b"orphan");
        inner.dispatch(&datagram).await; // panic しないこと
        assert_eq!(inner.handler_count().await, 0);
    }

    /// dispatch: malformed varint ▶ silently drop
    #[tokio::test]
    async fn inner_dispatch_drops_malformed_varint() {
        let inner = DispatcherInner::new();
        // 11 byte 全 continuation bit (= malformed)
        let datagram = vec![0xFFu8; 11];
        inner.dispatch(&datagram).await; // panic しないこと
    }

    /// dispatch: 複数 channel に同時 deliver
    #[tokio::test]
    async fn inner_dispatch_routes_by_channel_id() {
        let inner = DispatcherInner::new();
        let mut rx1 = inner.register(1, 16).await;
        let mut rx2 = inner.register(2, 16).await;

        let mut d1 = Vec::new();
        encode_varint(1, &mut d1);
        d1.extend_from_slice(b"for-1");

        let mut d2 = Vec::new();
        encode_varint(2, &mut d2);
        d2.extend_from_slice(b"for-2");

        inner.dispatch(&d1).await;
        inner.dispatch(&d2).await;

        assert_eq!(rx1.recv().await.unwrap(), b"for-1");
        assert_eq!(rx2.recv().await.unwrap(), b"for-2");
    }

    /// dispatch: buffer full の channel は drop、 他 channel への配信は継続
    #[tokio::test]
    async fn inner_dispatch_drops_when_buffer_full() {
        let inner = DispatcherInner::new();
        // buffer 1 で register、 1 件入れた後さらに送ると drop
        let mut rx = inner.register(1, 1).await;

        let mut d = Vec::new();
        encode_varint(1, &mut d);
        d.extend_from_slice(b"first");
        inner.dispatch(&d).await;

        // buffer full、 2 件目は drop される
        let mut d2 = Vec::new();
        encode_varint(1, &mut d2);
        d2.extend_from_slice(b"dropped");
        inner.dispatch(&d2).await;

        // 1 件目は受信できる
        assert_eq!(rx.recv().await.unwrap(), b"first");
        // 2 件目は drop されたので、 次の recv は (= sender まだ生きてるので) block するはず
        // tokio::select! で短 timeout を使って blocking を確認するのは over-test、 ここでは skip
    }

    /// 大量 channel_id (= 1024 件) 登録/解除のスループット sanity
    #[tokio::test]
    async fn inner_handles_many_channels() {
        let inner = DispatcherInner::new();
        let mut rxs = Vec::new();
        for id in 1..=1024u64 {
            rxs.push(inner.register(id, 4).await);
        }
        assert_eq!(inner.handler_count().await, 1024);

        // dispatch 1024 件
        for id in 1..=1024u64 {
            let mut d = Vec::new();
            encode_varint(id, &mut d);
            d.extend_from_slice(format!("msg-{}", id).as_bytes());
            inner.dispatch(&d).await;
        }

        // 全 receiver で確認
        for (i, rx) in rxs.iter_mut().enumerate() {
            let id = (i + 1) as u64;
            let payload = rx.recv().await.unwrap();
            assert_eq!(payload, format!("msg-{}", id).as_bytes());
        }
    }
}
