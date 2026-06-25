import Foundation

/// Unison client SDK のエラー。
public enum UnisonError: Error, Sendable {
    /// transport (QUIC handshake / 接続) が確立できなかった。
    case transport(String)
    /// 接続が既に切断されている状態で操作した。
    case notConnected
    /// channel の open がサーバーに拒否された (= channel-not-found 等)。
    case channelRejected(channel: String, reason: String)
    /// wire メッセージの encode / decode に失敗した。
    case codec(String)
    /// request がタイムアウトした。
    case timeout
    /// 未実装 (= scaffold 段階の stub 境界。 後続 pass で解消)。
    case notImplemented(String)
}
