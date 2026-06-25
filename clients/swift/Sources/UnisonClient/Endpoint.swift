import Foundation

/// 接続先の指定。 `design/typescript-client-api.md` の URL ベースと異なり、
/// Swift では型安全な enum で表現する (= caller が誤った URL 文字列を作れない)。
public enum Endpoint: Sendable, Equatable {
    /// loopback の local daemon (= `[::1]:port`)。 dev quickstart 用。
    case localDaemon(port: UInt16)
    /// 任意 host:port。 host は DNS 名 / IP リテラル (IPv6 は角括弧なしで渡す)。
    case host(String, port: UInt16)
    /// Bonjour service name による discovery (= `_unison._udp` 等、 将来拡張)。
    case bonjour(String)
}

/// server 証明書の信頼方針。 raw QUIC の TLS1.3 検証に適用する。
///
/// `pinned` / `skipVerify` は raw QUIC の cert pinning に対応する
/// (Rust 側 `TrustAnchors` / TS 側 `TrustMode` の Swift 対応物)。
public enum TrustPolicy: Sendable, Equatable {
    /// OS の system trust store で検証 (= public server 向け)。
    case system
    /// **DEV ONLY** — 証明書検証を skip (= `dev_localhost` 自己署名サーバー向け)。
    /// loopback 以外で使うべきでない。
    case skipVerify
    /// 指定した cert (DER) に pin (= internal mesh / self-issued)。
    case pinned(Data)
}
