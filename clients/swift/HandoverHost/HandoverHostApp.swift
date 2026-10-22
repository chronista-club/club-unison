import SwiftUI

/// A host for the existing Swift Testing probe on physical iOS devices.
@main
struct HandoverHostApp: App {
    var body: some Scene {
        WindowGroup {
            VStack(spacing: 20) {
                Image(systemName: "antenna.radiowaves.left.and.right")
                    .font(.largeTitle)
                Text("Unison Handover")
                    .font(.title)
                Text("回線切り替えの検証用アプリ\n操作は案内に合わせて行ってください。")
                    .multilineTextAlignment(.center)
            }
            .padding()
            .onAppear { UIApplication.shared.isIdleTimerDisabled = true }
            .onDisappear { UIApplication.shared.isIdleTimerDisabled = false }
        }
    }
}
