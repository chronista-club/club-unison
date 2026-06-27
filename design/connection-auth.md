# design/connection-auth.md — Connection-level Auth Primitive 設計

**バージョン**: 0.2 (設計確定 2026-06-27、 実装完了 2026-06-27)
**最終更新**: 2026-06-27
**ステータス**: 実装済 (= `network/auth.rs` + `enable_auth` + `connect_with_credential`、 E2E test 4件 green)
**対応仕様**: （未作成 — 必要なら `spec/05-auth/SPEC.md` を mirror）
**設計 SSOT**: Context Engine `mem_1CcTT4yxguA1KjGJXXHFor` / 実装 handoff `mem_1CcTTLKuuTYGfATSKdSo8J`

---

## 1. 背景

unison の全エンドポイント間通信（federation worlds channel / 連邦 wire / live streaming）に
認証を入れたい。ただし「どの層・どの粒度で認証するか」を誤ると、live streaming の小フレーム
fan-out を殺す。本設計は **認証を connection 確立時に1回行う primitive** として club-unison
に入れることを確定する。

これ待ちが 3 件ある:

- **hub federation**（chronista-hub ADR-020 §S3）: worlds channel + 連邦 wire の auth。
  hub は Creo ID JWKS verifier を policy として注入するだけにしたい。
- **VP wire-unison 移行**（vantage-point doc 27 B-4）: 同じ auth を各 protocol で再発明させない。
- **live streaming**（position / audio / asset の小フレーム fan-out）: per-message token だと
  frame が膨らむ → connection-level 必須（datagram は connection auth 以外に手段なし）。

---

## 2. 決定

### 2.1 auth = connection-level の club-unison primitive

- **authN（誰だ）= connection 確立時に1回**: client が credential（opaque bytes、例
  Creo ID JWT）を post-handshake で1回提示 → club-unison が **verifier callback（app 注入）**
  に渡す → 結果の **principal を [`ConnectionContext`] に保持**。
- **authZ（その動詞を許すか）= per-message**: channel handler が `ctx` の principal を引いて
  scope check。**フレームには 1 byte も足さない**。

### 2.2 mechanism / policy 分離（`CertSource` 哲学の踏襲）

- **mechanism = club-unison**: 「connection 確立直後に credential を1回受け取り、 verifier に
  渡し、 principal を ctx に立てる」配管。
- **policy = app（operator）**: verifier を注入。hub なら既存 Creo ID JWKS（client-cert PKI
  不要、 credential = Creo ID JWT）。
- これは [`CertSource`]（`network/cert.rs`）の「**library は trust model を選ばない、
  operator が選ぶ**」と完全同型。auth も同じ思想で設計する。

---

## 3. Mental model

### 3.1 authN は場への attach、 authZ は場の中の動詞

doctrine（場×動詞×規律）で言えば、auth =「誰がこの場に attach して tell/observe/ask できるか」
= 場への**アクセスの規律**。場に attach する瞬間（= connection 確立）に1回問うものであって、
場の中で流す各フレームに問うものではない。→ connection-level auth は doctrine の literal。

### 3.2 discovery primitive との対比

auth は `unison.discovery`（`design/datagram-channel.md` 兄弟、`spec/04-discovery`）と
**同型の reserved-channel パターン**で実装する。差分は handler が `ctx` を使う点のみ。

| | `unison.discovery` | `unison.auth`（本設計） |
|---|---|---|
| 有効化 API | `server.enable_discovery(kdl)` | `server.enable_auth(verifier)` |
| reserved channel 名 | `DISCOVERY_CHANNEL_NAME` | `AUTH_CHANNEL_NAME` |
| handler | `handle_channel(cache, stream)` | `handle_channel(verifier, ctx, stream)` |
| ctx の使用 | しない（`move \|_ctx, stream\|`） | **する**（principal を set） |
| client 側 | `client.open_channel(...)` | `client.connect_with_credential(...)` helper |
| policy 注入物 | protocol KDL | verifier callback |

---

## 4. なぜ connection-level か（= per-message でない理由）★ load-bearing

live streaming（origin→distribution→client 群 の fan-out、 position/audio/asset の小フレーム
高頻度）を unison transport の北極星に置くと、粒度の選択は一意に決まる:

- **per-message token は dead-end**: position frame（数バイト）に token を載せると auth が
  ペイロードより大きい。高頻度で throughput を殺す。
- **connection-level は per-frame 0 bytes**: TLS/QUIC handshake + 1 回の auth 交換で済み、
  以降の全 frame は auth 無料。「最初の数 turn だけ払い以降 0」。
- **datagram は connection-level 以外に選択肢が無い**: datagram channel（position 等
  latest-wins）は stream を持たず per-datagram auth を打てない → QUIC connection が認証済み
  なら datagram は auth を無料継承。
- **fan-out スケール**: 各 hop = 1 connection を setup 時に 1 回認証。distribution server は
  downstream client を接続時 1 回認証 → fan-out frame は auth 0。100k client でもコストは
  「接続を受ける」分のみ、 per-frame に乗らない。

---

## 5. 実装設計

`unison.discovery` の配線を 1:1 で踏襲する。新規モジュール `network/auth.rs` を起こし、
`context.rs` / `server.rs` / `client.rs` に最小の足し込みを行う。

### 5.1 `ConnectionContext` に principal を追加（`network/context.rs`）

現状 `ConnectionContext` は `connection_id` / `identity` / `channels` のみで client identity を
持たない。ここに principal を足す:

```rust
pub struct ConnectionContext {
    pub connection_id: Uuid,
    identity: Arc<RwLock<Option<ServerIdentity>>>,
    channels: Arc<RwLock<HashMap<String, ChannelHandle>>>,
    /// 認証済み client principal（未認証なら None）。
    /// club-unison は中身を知らない（opaque）。app が downcast する。
    principal: Arc<RwLock<Option<Principal>>>,
}

/// opaque な principal。club-unison は型を一切解釈しない。
pub type Principal = Arc<dyn std::any::Any + Send + Sync>;

impl ConnectionContext {
    pub async fn set_principal(&self, p: Principal) { /* write guard */ }
    pub async fn principal(&self) -> Option<Principal> { /* read guard clone */ }
}
```

**型選択 — `Arc<dyn Any + Send + Sync>` を採用**（§6 で論拠）。`ConnectionContext<P>` と
ジェネリック化すると `register_channel` の handler 型 `Fn(Arc<ConnectionContext>, ...)` 経由で
`P` が dispatch 全体に伝播する。非ジェネリックな opaque 型に保つことで変更を `context.rs` に
局所化でき、「club-unison は principal の中身を知らない」も literal に満たせる。

### 5.2 `enable_auth(verifier)`（`network/server.rs`）

`enable_discovery` と同型。reserved channel `AUTH_CHANNEL_NAME` を登録する。verifier は
**async**（JWKS の network fetch を伴う実 use case に合わせ確定）。credential は所有権ごと
渡す（`async move` ブロックへ move できる）:

```rust
/// connection-level 認証を有効化する。client が接続直後に `unison.auth` を open →
/// credential を 1 request 送る → verifier (= app 注入) で検証 → 返った principal を
/// その connection の ctx に set する。
///
/// verifier = policy（app 注入）。club-unison は credential の中身を知らない。
pub async fn enable_auth<V, Fut>(&self, verifier: V)
where
    V: Fn(Vec<u8>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Option<Principal>> + Send + 'static,
{
    // closure を box 化して非ジェネリックな Verifier に詰める
    let verifier: super::auth::Verifier =
        Arc::new(move |cred| Box::pin(verifier(cred)) as _);
    self.register_channel(super::auth::AUTH_CHANNEL_NAME, move |ctx, stream| {
        let verifier = Arc::clone(&verifier);
        async move { super::auth::handle_channel(verifier, ctx, stream).await }
    })
    .await;
}
```

### 5.3 `network/auth.rs`（新規モジュール）

`discovery.rs` を template に:

```rust
/// `unison.auth` channel name（schemas/auth.kdl 側と一致）
pub const AUTH_CHANNEL_NAME: &str = "unison.auth";
/// credential 提示メソッド
pub const AUTHENTICATE_METHOD: &str = "Authenticate";

/// app 注入の認証 verifier（= policy）。credential を所有権ごと受け取り、async で検証。
pub type Verifier =
    Arc<dyn Fn(Vec<u8>) -> Pin<Box<dyn Future<Output = Option<Principal>> + Send>> + Send + Sync>;

/// `unison.auth` channel handler。credential を 1 request 受け取り、verifier に
/// 渡し、principal を ctx に set し、ok/deny を返す。
pub async fn handle_channel(
    verifier: Verifier,
    ctx: Arc<ConnectionContext>,
    stream: UnisonStream,
) -> Result<(), NetworkError> {
    // 1. request 受信（AuthenticateRequest { credential: Vec<u8> }）
    // 2. verifier(credential).await → Option<Principal>
    // 3. Some(p) → ctx.set_principal(p) → AuthResult { ok: true }
    //    None    → AuthResult { ok: false }（principal は None のまま）
    //    malformed payload → ok: false（verifier に渡さない）
}
```

### 5.4 client が credential を提示（`network/client.rs`）

接続直後に `unison.auth` を open して送る helper を足す:

```rust
/// connect 後に credential を 1 回提示する。内部で unison.auth を open し
/// Authenticate request を送り、ok/deny を待つ。
pub async fn connect_with_credential(&self, url: &str, credential: &[u8])
    -> Result<(), NetworkError>
{
    self.connect(url).await?;
    let chan = self.open_channel(super::auth::AUTH_CHANNEL_NAME).await?;
    // Authenticate request を送り、deny なら Err
}
```

### 5.5 authZ は app 側（mechanism/policy 分離）

各 channel handler が `ctx.principal()` を引いて gate する。club-unison は principal を
**提供するだけ**で、何を要求するか（scope / role）は app が決める:

```rust
server.register_channel("worlds", |ctx, stream| async move {
    let principal = ctx.principal().await
        .and_then(|p| p.downcast_ref::<MyPrincipal>().cloned()); // app の型
    let Some(principal) = principal else {
        return Err(/* unauthenticated */);
    };
    // principal.scopes で per-message gate ...
});
```

### 5.6 ctx 共有の保証（実装前 検証済 ✅）

本設計は「`unison.auth` handler が立てた principal を worlds/wire/datagram handler が読める」
ことに依存する。これは **同一 connection の全 channel handler が同じ `Arc<ConnectionContext>`
を共有する**ことで成立する。`network/dispatch.rs` が connection ごとに ctx を 1 つ作り、
各 channel handler 起動時に `Arc::clone(&ctx)` を渡している（dispatch.rs の
`let ctx = Arc::clone(&ctx);` → `handler(ctx, stream).await`）ことを確認済み。

### 5.7 `schemas/auth.kdl`

`schemas/discovery.kdl` と同型で reserved channel を定義する:

```kdl
channel "unison.auth" from="client" lifetime="persistent" {
    request "Authenticate" {
        // credential は opaque bytes。KDL schema に native bytes 型が無く、 JSON codec
        // 上は byte 配列 ([u8]) として運ばれるため type="json"（任意 JSON 可）とする。
        field "credential" type="json" required=#true
        returns "AuthResult" {
            field "ok" type="bool" required=#true
        }
    }
}
```

### 5.8 クライアント API contract（言語横断 SSOT）

auth はクライアント側に専用 transport を要求しない。**`unison.auth` channel を open して
`Authenticate` request を送るだけ**で、stream channel + request を持つ全クライアントが認証できる
（Rust の `connect_with_credential` は便利ラッパー）。各言語 client はこの contract に従う。

#### Wire 不変条件（全クライアント共通・最重要）
- channel 名 = `unison.auth` / request method = `Authenticate`
- request payload = `{ "credential": <number[]> }` — credential は **u8 の JSON 数値配列**（各要素 0–255）。
  - ⚠️ **言語の「バイト列デフォルト JSON 表現」に任せないこと**。Rust 側は `Vec<u8>` = `serde_json`
    の数値配列 `[104,101,...]` を期待する。
    - **TS**: `Uint8Array` を直接入れると `{"0":104,...}` object 化 → `Array.from(bytes)` で `number[]` に。
    - **Swift**: `Data` を `Codable` に入れると **base64 string** 化 → `[UInt8]` 配列にして送る。
    - **Ruby**: binding 経由で Rust の `Vec<u8>` に直接渡るので native（String / bytes）でよい。
- response payload = `{ "ok": boolean }`。`ok == false` → 認証拒否（throw / Err）、principal は立たない。

#### 各言語の API 形（Rust `connect_with_credential` の対応物）
| 言語 | API |
|------|-----|
| Rust | `client.connect_with_credential(url, &[u8]) -> Result<()>` |
| TypeScript | `connect({ ..., credential: Uint8Array })` + `client.authenticate(credential: Uint8Array)` |
| Swift | `UnisonClient.connect(to:trust:credential:)` + `Connection.authenticate(_ credential: [UInt8])` |
| Ruby | `client.connect_with_credential(url, credential)`（pure Ruby で `Client` 再オープン、 `open_channel`+`request` の上に実装。native ext は公開版 club-unison に対しビルドされるため、 未公開 Rust `connect_with_credential` に依存しない） |

詳細は各 client design doc の auth 節（`design/typescript-client-api.md` / `design/swift-client-api.md`）。

---

## 6. 型・API の確定事項（実装済）

| 論点 | 確定 | 論拠 |
|------|------|------|
| principal の型 | `Arc<dyn Any + Send + Sync>` | 非ジェネリック維持で変更を context.rs に局所化。完全 opaque。 |
| verifier の sync/async | **async**（`Fn(Vec<u8>) -> Future<Option<Principal>>`） | hub の Creo ID JWKS が cache miss 時に network fetch する実態に合う。最初から正しい形で API 破壊を回避。 |
| credential の wire 型 | `Vec<u8>`（KDL は `json`） | club-unison は中身を知らない。JWT も binary token も載る。接続時1回のみで per-frame でない。 |
| 未認証時の挙動 | principal = None のまま接続維持 | gate は app 責務。connection は切らず handler が拒否。 |

> 2026-06-27 の plan phase で user 承認のうえ確定・実装。

---

## 7. Acceptance criteria

- credential 無し / 不正で接続 → `ctx.principal()` = None → app handler が gated method を拒否。✅
- 正当 credential → principal set → per-message scope check 通過、 **per-frame に auth byte 0**。✅
- verifier は app 注入（club-unison は Creo ID を知らない）。✅
- `unison.discovery` と同様、`enable_auth` を呼ばない server は従来通り（auth 無効・非破壊）。✅

E2E 検証: `crates/unison-protocol/tests/test_integ_auth.rs`（4 ケース、全 green）。

### 既知の制約 / 将来拡張
- **datagram の principal 継承（transport のみ）**: 認証済み QUIC connection 上を datagram は
  そのまま流れる（= per-datagram auth 不要）。ただし stream channel handler が `Fn(Arc<ctx>,
  stream)` で ctx を受け取るのに対し、 **datagram channel handler は `Fn(DatagramChannel)` で
  ctx を受け取らない**（`network/server.rs::register_channel_datagram`）。そのため datagram
  handler 内から `ctx.principal()` を引いた app-level gate は現状できない。datagram に
  principal-based gate が必要になったら、 datagram handler に ctx を渡す拡張を別途行う
  （本 primitive の範囲外、 forward-compatible）。

---

## 8. E2E（別レイヤー、 当面不要）

distribution が **trusted infra** なら hop-by-hop connection auth で十分。distribution が
**untrusted（公開リレー）** で「origin 署名を client が検証」したい場合のみ、**content 署名**
（stream/chunk 単位、 per-frame でない）を transport auth と直交に足す。Chronista の
trusted-peer-mesh 前提では当面不要。

---

## 9. 短期 fallback（club-unison 実装前に hub が unblock 要時）

hub-protocol 内の **channel-open auth**（worlds channel の最初の message を Authenticate に）で
代替可。attach-time なので本 primitive と **forward-compatible**（後で剥がす痛みなし）。
**per-message token には降りない**。

---

## 10. 関連

- 設計 SSOT: Context Engine `mem_1CcTT4yxguA1KjGJXXHFor`
- 実装 handoff: Context Engine `mem_1CcTTLKuuTYGfATSKdSo8J`
- mechanism/policy 先例: [`CertSource`]（`network/cert.rs`）
- reserved-channel 先例: `unison.discovery`（`network/discovery.rs`、`spec/04-discovery`）
- ctx 共有: `network/dispatch.rs`
- 依存: chronista-hub federation ADR-020 / VP wire-unison 移行（B-4）

[`ConnectionContext`]: ../crates/unison-protocol/src/network/context.rs
[`CertSource`]: ../crates/unison-protocol/src/network/cert.rs
