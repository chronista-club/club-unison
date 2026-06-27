//! ConnectionContext: QUIC接続ごとの状態管理
//!
//! 各接続に対して、Identity情報とアクティブチャネルを追跡する。
//! 複数のストリームハンドラーから並行アクセスされるため Arc<RwLock<>> で保護。

use std::any::Any;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

use super::identity::{ChannelDirection, ServerIdentity};

/// 認証済み client の principal。
///
/// connection-level auth (= `unison.auth` channel) で verifier が返した値を保持する。
/// **opaque** — このライブラリは中身の型を一切解釈しない。policy (= app) が
/// [`ConnectionContext::principal`] で取り出して `downcast_ref::<MyPrincipal>()` する。
///
/// この opacity が、ライブラリが特定の認証エコシステム (Creo ID / JWKS 等) に
/// 依存しないことを型レベルで保証する (= mechanism/policy 分離、`cert::CertSource` と同型)。
///
/// 設計: `design/connection-auth.md`
pub type Principal = Arc<dyn Any + Send + Sync>;

/// 接続ごとの状態を管理する構造体
#[derive(Debug)]
pub struct ConnectionContext {
    /// 接続の一意識別子
    pub connection_id: Uuid,
    /// サーバーから受信したIdentity情報
    identity: Arc<RwLock<Option<ServerIdentity>>>,
    /// アクティブなチャネルのマップ（チャネル名 → ハンドル）
    channels: Arc<RwLock<HashMap<String, ChannelHandle>>>,
    /// 認証済み client principal（未認証なら None）。
    ///
    /// `unison.auth` handler が verifier の結果を [`set_principal`](Self::set_principal) で
    /// 立て、 worlds/wire/datagram 等 **同一 connection の全 channel handler** が
    /// [`principal`](Self::principal) で読んで authZ gate に使う。
    principal: Arc<RwLock<Option<Principal>>>,
}

/// チャネルのメタデータ
#[derive(Debug, Clone)]
pub struct ChannelHandle {
    pub channel_name: String,
    pub stream_id: u64,
    pub direction: ChannelDirection,
}

impl ConnectionContext {
    /// 新しいConnectionContextを作成
    pub fn new() -> Self {
        Self {
            connection_id: Uuid::new_v4(),
            identity: Arc::new(RwLock::new(None)),
            channels: Arc::new(RwLock::new(HashMap::new())),
            principal: Arc::new(RwLock::new(None)),
        }
    }

    /// Identity情報を設定
    pub async fn set_identity(&self, identity: ServerIdentity) {
        let mut guard = self.identity.write().await;
        *guard = Some(identity);
    }

    /// Identity情報を取得
    pub async fn identity(&self) -> Option<ServerIdentity> {
        self.identity.read().await.clone()
    }

    /// 認証済み principal を設定する（`unison.auth` handler が verifier 成功時に呼ぶ）。
    pub async fn set_principal(&self, principal: Principal) {
        let mut guard = self.principal.write().await;
        *guard = Some(principal);
    }

    /// 認証済み principal を取得する（未認証なら None）。
    ///
    /// app handler は `ctx.principal().await.and_then(|p| p.downcast_ref::<T>().cloned())`
    /// のように自分の型へ downcast して authZ gate に使う。
    pub async fn principal(&self) -> Option<Principal> {
        self.principal.read().await.clone()
    }

    /// チャネルを登録
    pub async fn register_channel(&self, handle: ChannelHandle) {
        let mut channels = self.channels.write().await;
        channels.insert(handle.channel_name.clone(), handle);
    }

    /// チャネルを取得
    pub async fn get_channel(&self, name: &str) -> Option<ChannelHandle> {
        let channels = self.channels.read().await;
        channels.get(name).cloned()
    }

    /// チャネルを削除
    pub async fn remove_channel(&self, name: &str) -> Option<ChannelHandle> {
        let mut channels = self.channels.write().await;
        channels.remove(name)
    }

    /// 全チャネル名を取得
    pub async fn channel_names(&self) -> Vec<String> {
        let channels = self.channels.read().await;
        channels.keys().cloned().collect()
    }
}

impl Default for ConnectionContext {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_connection_context_creation() {
        let ctx = ConnectionContext::new();
        assert!(ctx.identity().await.is_none());
        assert!(ctx.channel_names().await.is_empty());
        assert!(ctx.principal().await.is_none());
    }

    #[tokio::test]
    async fn test_principal_set_and_downcast() {
        #[derive(Debug, PartialEq)]
        struct MyPrincipal {
            user_id: String,
        }

        let ctx = ConnectionContext::new();
        // 未認証は None
        assert!(ctx.principal().await.is_none());

        // opaque な型を set
        ctx.set_principal(Arc::new(MyPrincipal {
            user_id: "alice".to_string(),
        }))
        .await;

        // app は自分の型へ downcast して取り出せる
        let principal = ctx.principal().await.expect("principal should be set");
        let typed = principal
            .downcast_ref::<MyPrincipal>()
            .expect("downcast should succeed");
        assert_eq!(typed.user_id, "alice");

        // 異なる型への downcast は失敗する（opacity の確認）
        assert!(principal.downcast_ref::<String>().is_none());
    }

    #[tokio::test]
    async fn test_identity_set_and_get() {
        let ctx = ConnectionContext::new();
        let identity = ServerIdentity::new("test-server", "0.1.0", "test");
        ctx.set_identity(identity.clone()).await;

        let retrieved = ctx.identity().await.unwrap();
        assert_eq!(retrieved.name, "test-server");
        assert_eq!(retrieved.version, "0.1.0");
    }

    #[tokio::test]
    async fn test_channel_registration() {
        let ctx = ConnectionContext::new();

        let handle = ChannelHandle {
            channel_name: "events".to_string(),
            stream_id: 1,
            direction: ChannelDirection::ServerToClient,
        };
        ctx.register_channel(handle).await;

        let retrieved = ctx.get_channel("events").await.unwrap();
        assert_eq!(retrieved.stream_id, 1);
        assert_eq!(retrieved.direction, ChannelDirection::ServerToClient);

        let names = ctx.channel_names().await;
        assert_eq!(names, vec!["events"]);
    }

    #[tokio::test]
    async fn test_channel_removal() {
        let ctx = ConnectionContext::new();

        let handle = ChannelHandle {
            channel_name: "control".to_string(),
            stream_id: 2,
            direction: ChannelDirection::Bidirectional,
        };
        ctx.register_channel(handle).await;

        let removed = ctx.remove_channel("control").await;
        assert!(removed.is_some());
        assert!(ctx.get_channel("control").await.is_none());
    }
}
