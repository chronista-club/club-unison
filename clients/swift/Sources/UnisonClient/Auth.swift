import Foundation

/// Connection-level auth (= v1.4.0)。
///
/// Rust `enable_auth` / `connect_with_credential` の Swift 対応。auth は専用 transport を
/// 要求せず、 reserved `unison.auth` channel を open して `Authenticate` request を 1 回
/// 送るだけ (= stream channel + request を持てば認証できる)。
///
/// wire 不変条件・各言語 API の SSOT は `design/connection-auth.md` §5.8。

/// reserved auth channel 名 (= Rust `network::auth::AUTH_CHANNEL_NAME`)。
public let authChannelName = "unison.auth"

/// credential 提示 method (= Rust `network::auth::AUTHENTICATE_METHOD`)。
public let authenticateMethod = "Authenticate"

/// server push event を持たない channel 用のプレースホルダ event 型。
public struct NoEvent: Sendable, Decodable {}

/// `unison.auth` の reserved channel meta (= codegen 不要、 SDK 内蔵)。
public struct AuthChannelMeta: StreamChannelMeta {
    public typealias Event = NoEvent
    public static let name = authChannelName
    public init() {}
}

/// `Authenticate` request (= wire: `{ "credential": [u8...] }`)。
///
/// credential は **`[UInt8]`** で持つ。⚠️ `Data` にすると `JSONEncoder` が
/// base64 string 化し、 Rust の `Vec<u8>` (= serde_json の数値配列) と **非互換**に
/// なるため使わないこと。library は credential の中身を解釈しない。
public struct AuthenticateRequest: UnisonRequest {
    public typealias Response = AuthResult
    public static let method = authenticateMethod
    public let credential: [UInt8]
    public init(credential: [UInt8]) {
        self.credential = credential
    }
}

/// `AuthResult` response (= server の verifier 結果)。
public struct AuthResult: Sendable, Decodable {
    public let ok: Bool
}
