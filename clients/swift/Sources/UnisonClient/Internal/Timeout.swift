import Foundation

/// `operation` を `timeout` と競わせる。 先に期限が来たら `UnisonError.timeout` を投げ、
/// `operation` の Task を cancel する。 `timeout` が nil なら期限を付けない。
///
/// `operation` は cancel に応じて戻ること (= 応じない待ちがあると、 期限が来ても
/// ここから戻れない)。 SDK 内の待ち (open / request / accept / stream の開始) は
/// すべて cancel に応じる。
///
/// 呼び出した Task が cancel された場合は、 `operation` がどの error で戻っても
/// `CancellationError` を投げる (= cancel で起こされた待ちは、 stream を閉じた結果の
/// `notConnected` などで戻ることがある。 caller に見せる error を揃える)。
func withTimeout<T: Sendable>(
    _ timeout: Duration?,
    _ operation: @escaping @Sendable () async throws -> T
) async throws -> T {
    do {
        guard let timeout else {
            return try await operation()
        }
        return try await withThrowingTaskGroup(of: T.self) { group in
            group.addTask { try await operation() }
            group.addTask {
                try await Task.sleep(for: timeout)
                throw UnisonError.timeout
            }
            defer { group.cancelAll() }
            // 子は 2 つあるので next() は nil にならない。
            return try await group.next()!
        }
    } catch {
        try Task.checkCancellation()
        throw error
    }
}
