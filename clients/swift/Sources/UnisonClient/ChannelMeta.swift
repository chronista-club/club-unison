import Foundation

/// channel の静的メタ情報 (= 生成 `ChannelMeta` の Swift 対応物)。
///
/// KDL schema → Swift codegen で各 channel の具象型が生成される想定 (当面手書き)。
/// app 固有の channel meta は consumer (例: VP の `VPProtocol`) が定義する。
public protocol ChannelMeta: Sendable {
    /// `__channel:` mux で使う channel 名。
    static var name: String { get }
}

/// stream backend の channel meta (= request/response + server push event)。
///
/// `Event` は wire payload (= 既定 JSON codec) から decode するため `Decodable`。
public protocol StreamChannelMeta: ChannelMeta {
    /// この channel が server から push する event の型。
    associatedtype Event: Sendable & Decodable
}

/// datagram backend の channel meta (= unreliable な server push event 専用)。
public protocol DatagramChannelMeta: ChannelMeta {
    /// この channel が server から push する event の型。
    associatedtype Event: Sendable & Decodable
}

/// channel 上で送る request。 method 名と response 型を静的に紐づける。
///
/// 生成 `ChannelMeta` の `requests` map に対応。 idiom 的に、 handoff sketch の
/// `request<R: M.Request>` を `request<R: UnisonRequest>` に置き換えている
/// (= associatedtype を generic constraint に使えない Swift の制約を、 caller 体験を
/// 損なわず回避)。
///
/// wire は既定 JSON codec のため request は `Encodable`、 response は `Decodable`。
public protocol UnisonRequest: Sendable & Encodable {
    /// この request に対する response の型。
    associatedtype Response: Sendable & Decodable
    /// wire 上の method 名。
    static var method: String { get }
}
