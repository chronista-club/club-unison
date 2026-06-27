//! Auth channel handler — server-side handler for `unison.auth`.
//!
//! 役割: connection 確立直後に client が提示する credential を1回受け取り、
//! app 注入の [`Verifier`] に渡し、 成功なら [`Principal`] を
//! [`ConnectionContext`] に立てる (= connection-level authN)。
//!
//! authZ (= per-message scope check) は app 側の責務。各 channel handler が
//! [`ConnectionContext::principal`] を引いて gate する (= mechanism/policy 分離)。
//!
//! # mechanism / policy 分離
//!
//! - **mechanism = このモジュール**: credential を受け取り verifier に渡し principal を
//!   立てる配管。credential / principal の中身は一切解釈しない (= opaque)。
//! - **policy = app (operator)**: [`Verifier`] を注入。Creo ID JWKS でも静的 API キーでも
//!   独自トークンでも良い。これにより library は特定の認証エコシステムに依存しない
//!   ([`super::cert::CertSource`] が trust model を選ばないのと同型)。
//!
//! 設計: `design/connection-auth.md`
//! KDL: `schemas/auth.kdl`
//!
//! # 典型使用 (server 側)
//!
//! ```ignore
//! let server = ProtocolServer::new();
//! server.enable_auth(|credential: Vec<u8>| async move {
//!     // app の policy: credential を検証して principal を返す
//!     (credential == b"secret-token")
//!         .then(|| std::sync::Arc::new(MyUser::admin()) as Principal)
//! }).await;
//! // 以降、 client は connect_with_credential(addr, cred) で認証し、
//! // 各 channel handler は ctx.principal() で authZ gate する。
//! ```

use serde::{Deserialize, Serialize};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use super::context::{ConnectionContext, Principal};
use super::quic::UnisonStream;
use super::{MessageType, NetworkError, UnisonChannel};

/// `unison.auth` channel name (= `schemas/auth.kdl` 側と一致)
pub const AUTH_CHANNEL_NAME: &str = "unison.auth";

/// `Authenticate` request method name (= `schemas/auth.kdl` 側と一致)
pub const AUTHENTICATE_METHOD: &str = "Authenticate";

/// `Authenticate` request payload (client → server)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthenticateRequest {
    /// opaque な credential bytes (= 例 Creo ID JWT、 API キー、 独自トークン)。
    /// library は中身を解釈せず verifier にそのまま渡す。
    pub credential: Vec<u8>,
}

/// `AuthResult` response payload (server → client)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthResult {
    /// verifier が principal を返した (= 認証成功) なら true。
    pub ok: bool,
}

/// app 注入の認証 verifier (= policy)。
///
/// credential bytes を受け取り、 認証成功なら [`Principal`] を、 失敗なら `None` を返す。
/// async — Creo ID JWKS の cache miss など network fetch を伴う検証を許容する。
///
/// credential は所有権ごと渡される (= async block へ move できる)。返す future は
/// `'static` (= boxed) なので、 verifier は credential を借用したまま future に
/// 抱え込めない (所有権を move すること)。
pub type Verifier =
    Arc<dyn Fn(Vec<u8>) -> Pin<Box<dyn Future<Output = Option<Principal>> + Send>> + Send + Sync>;

/// `unison.auth` channel handler loop。
///
/// 1 connection 毎に 1 回起動され、 channel が close するまで request を待ち受ける。
/// `Authenticate` request に対して verifier を呼び、 成功なら `ctx` に principal を立てて
/// `AuthResult { ok: true }` を、 失敗なら `AuthResult { ok: false }` を返す (principal は
/// None のまま)。他 method / event は debug log を吐いて無視する (= forward-compat)。
///
/// `ctx` は **connection 単位で全 channel handler と共有** される ([`super::dispatch`] が
/// `Arc::clone` で配る) ため、 ここで立てた principal を worlds/wire/datagram handler が
/// 読める。
pub async fn handle_channel(
    verifier: Verifier,
    ctx: Arc<ConnectionContext>,
    stream: UnisonStream,
) -> Result<(), NetworkError> {
    let channel = UnisonChannel::new(stream);
    loop {
        match channel.recv().await {
            Ok(msg) if msg.msg_type == MessageType::Request => {
                if msg.method == AUTHENTICATE_METHOD {
                    let credential = match msg.payload_as_value() {
                        Ok(value) => match serde_json::from_value::<AuthenticateRequest>(value) {
                            Ok(req) => req.credential,
                            Err(e) => {
                                // malformed payload は認証拒否 (= verifier に渡さない)。
                                tracing::debug!(error = %e, "auth: malformed Authenticate payload");
                                channel
                                    .send_response(
                                        msg.id,
                                        AUTHENTICATE_METHOD,
                                        &serde_json::to_value(AuthResult { ok: false })?,
                                    )
                                    .await?;
                                continue;
                            }
                        },
                        Err(e) => {
                            tracing::debug!(error = %e, "auth: non-JSON Authenticate payload");
                            channel
                                .send_response(
                                    msg.id,
                                    AUTHENTICATE_METHOD,
                                    &serde_json::to_value(AuthResult { ok: false })?,
                                )
                                .await?;
                            continue;
                        }
                    };

                    let principal = verifier(credential).await;
                    let ok = principal.is_some();
                    if let Some(p) = principal {
                        ctx.set_principal(p).await;
                    }
                    channel
                        .send_response(
                            msg.id,
                            AUTHENTICATE_METHOD,
                            &serde_json::to_value(AuthResult { ok })?,
                        )
                        .await?;

                    tracing::debug!(ok, "auth: served Authenticate");
                } else {
                    tracing::warn!(
                        method = %msg.method,
                        "auth: unknown request method, ignoring (= forward-compat)"
                    );
                }
            }
            Ok(msg) => {
                tracing::debug!(
                    method = %msg.method,
                    msg_type = ?msg.msg_type,
                    "auth: ignored non-request"
                );
            }
            Err(e) if e.is_normal_close() => return Ok(()),
            Err(e) => return Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// const が schemas/auth.kdl と一致していることの guard test。
    /// schemas を編集したら必ずこの test も更新する。
    #[test]
    fn auth_names_match_kdl_schema() {
        assert_eq!(AUTH_CHANNEL_NAME, "unison.auth");
        assert_eq!(AUTHENTICATE_METHOD, "Authenticate");
    }

    /// AuthenticateRequest の JSON serde round-trip
    #[test]
    fn authenticate_request_serde_round_trip() {
        let req = AuthenticateRequest {
            credential: b"creo-id-jwt".to_vec(),
        };
        let value = serde_json::to_value(&req).unwrap();
        let restored: AuthenticateRequest = serde_json::from_value(value).unwrap();
        assert_eq!(restored.credential, req.credential);
    }

    /// AuthResult の JSON serde round-trip
    #[test]
    fn auth_result_serde_round_trip() {
        for ok in [true, false] {
            let value = serde_json::to_value(AuthResult { ok }).unwrap();
            let restored: AuthResult = serde_json::from_value(value).unwrap();
            assert_eq!(restored.ok, ok);
        }
    }

    /// Verifier の async closure が credential を所有権ごと受け取り principal を返せる
    #[tokio::test]
    async fn verifier_async_closure_returns_principal() {
        let verifier: Verifier = Arc::new(|cred: Vec<u8>| {
            Box::pin(async move {
                (cred == b"good").then(|| Arc::new("alice".to_string()) as Principal)
            })
        });

        let ok = verifier(b"good".to_vec()).await;
        assert!(ok.is_some());
        assert_eq!(
            ok.unwrap().downcast_ref::<String>().map(String::as_str),
            Some("alice")
        );

        let denied = verifier(b"bad".to_vec()).await;
        assert!(denied.is_none());
    }
}
