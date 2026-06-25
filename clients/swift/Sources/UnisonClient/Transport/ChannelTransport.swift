import Foundation

/// 双方向 byte stream の抽象 (= TS `BidiStream` の Swift 対応物)。
///
/// NWProtocolQUIC の実 stream と in-memory test stream を同一 interface で扱い、
/// channel mux / handshake / request-response のロジックを transport から切り離す
/// (= 最もテストしたいプロトコル状態機械を、 QUIC の不確実性から独立させる)。
public protocol ChannelStream: Sendable {
    /// 完成した typed frame バイト列を 1 本送る。
    func send(_ bytes: Data) async throws
    /// 次の受信 chunk を返す。 EOF (stream 終端) なら nil。
    func receive() async throws -> Data?
    /// stream を閉じる。
    func close() async
}

/// stream を開ける / 受け入れる接続の抽象 (= TS `Connection` の stream 部)。
///
/// - `openStream`: client 起点の bidi stream (= channel open / request 用)。
/// - `acceptStream`: server 起点の bidi stream (= identity handshake 用)。
public protocol ChannelTransport: Sendable {
    /// client 起点の bidi stream を開く。
    func openStream() async throws -> any ChannelStream
    /// server 起点の bidi stream を 1 本受け入れる。 接続が閉じたら nil。
    func acceptStream() async throws -> (any ChannelStream)?
    /// transport を閉じる (= 接続切断)。
    func close() async
}
