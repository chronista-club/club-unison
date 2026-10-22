# Swift SDK の回線切り替えを実機で確認する

Swift SDK は `NWParameters.multipathServiceType = .handover` で QUIC connection
migration を opt-in する。サーバーは Quinn のまま。Apple の設定名に multipath が
含まれるが、noq の QUIC Multipath 拡張ではない。

根拠: [Apple WWDC22、6:29](https://developer.apple.com/videos/play/wwdc2022/10078/?time=389)。

## 何を判定するか

`HandoverLiveTests` は接続と `handover-probe` チャネルを最初に一度だけ開き、
1 秒間隔で要求する。再接続・チャネル再作成は行わない。サーバーが返す接続 ID、
チャネル ID、受信連番が維持されることを検査し、応答時間と応答間隔を記録する。

同じ回線で成功しただけでは handover の証明にならない。切り替え操作の時刻と
`PATH` ログを対応づける。`NWPathMonitor` は OS の経路情報であり、個々の QUIC
パケットが実際に通ったインターフェースの証明ではない。必要ならサーバー側の
パケット取得で送信元の変化を照合する。

## サーバー

両回線から同じ公開 IP / ホスト名へ UDP 17878 で到達できる検証用ホストを使う。
Wi-Fi 内のプライベートアドレスでは携帯回線側の検証にならない。
さくらクラウド等の scratch 環境では OS firewall と cloud packet filter の両方を
確認する。通常の HTTPS reverse proxy はこの UDP を中継しない。
VPN / Tailscale 内の経路で試す場合は、トンネル自体の handover と区別して記録する。

```bash
# club-unison のリポジトリルート。既定は loopback のみ。
cargo run -p club-unison --example handover_probe_server -- '[::1]:17878'

# 公開する scratch ホスト上で使う場合（配置・公開ポートは別途決定する）
cargo run -p club-unison --example handover_probe_server -- '0.0.0.0:17878'
```

起動時の `CERT_HEX=...` は公開証明書の DER 全体を hex 化した pin。
秘密鍵は出力しない。再起動で証明書が変わるので pin も更新する。
サーバーは診断用で認証なし。検証後は Ctrl-C で停止し、一時的な公開設定を戻す。
既存サービスやそのポートを流用しない。

## ローカルの疎通確認

サーバーを別ターミナルで起動してから、同じリポジトリルートで実行する。

```bash
UNISON_HANDOVER=1 \
UNISON_HANDOVER_HOST=::1 \
UNISON_HANDOVER_PORT=17878 \
UNISON_HANDOVER_SAMPLES=3 \
swift test --filter HandoverLiveTests
```

loopback のみ pin 省略時に検証を省略できる。リモートでは必ず
`UNISON_HANDOVER_CERT_HEX` を設定する。このローカル確認では回線を変更しない。

## iPhone 実機

1. Xcode で repo root の `Package.swift` を開き、`UnisonClient` scheme を選ぶ。
2. iPhone は USB 接続にし、Developer Mode / 信頼 / 署名の設定を済ませる。
   Wi-Fi デバッグ接続自体を切ってしまうことを避ける。Simulator は携帯回線検証に使わない。
3. Scheme → Edit Scheme → Test → Arguments の環境変数に次を設定する。

| 環境変数 | 値 |
|---|---|
| `UNISON_HANDOVER` | `1` |
| `UNISON_HANDOVER_HOST` | 両回線から届くホスト名または公開 IP（URL ではない） |
| `UNISON_HANDOVER_PORT` | `17878` |
| `UNISON_HANDOVER_CERT_HEX` | サーバーが表示した pin の `=` より後の文字列 |
| `UNISON_HANDOVER_SAMPLES` | `120`（2〜300、応答後に1秒待つ） |

4. 実機を destination にし、Test navigator から
   `HandoverLiveTests.sameConnectionAndChannelSurviveManualSwitch` だけ実行する。
5. Wi-Fi で応答が数回続いたら、コントロールセンターで Wi-Fi 接続を切り、
   携帯回線に移す。すぐテスト画面に戻し、端末をロックしない。
6. 応答が再開してから Wi-Fi を戻す。逆方向の移行が実際に起きたかは別に記録する。
   `.handover` は Wi-Fi の復帰だけで即座の逆移行を保証する設定ではない。
7. Xcode の `PROBE` / `PATH` ログとサーバーログを保存する。

成功条件は「切り替え後にも同じ `connection` と `channel`、連続する `seq` の
応答が得られる」。`rtt` は要求の往復時間、`gap` は前回応答からの空白時間。
`CONTINUITY_COMPLETE` はチャネル継続確認であり、回線を切り替えた証明ではない。

SDK の `connectionEvents` は現在、明示的な disconnect 以外の切断検出に制約があるため、
このプローブの合否には使わない。要求失敗・ID変更・連番リセット・全体期限切れは失敗。
全体期限は `samples + 90` 秒で、停止中の要求も接続を閉じて解除する。
Quinn 側の idle timeout は60秒、相手との交渉で短くなる場合がある。
圏外、アプリのバックグラウンド停止、サーバー再起動は別の検証項目とする。

## 記録するもの

- 日時、端末 / iOS、携帯キャリア、Wi-Fi、接続先、SDK commit。
- 各切り替え操作の時刻、前後の PATH、同じ接続・チャネル ID、最大 gap。
- ローカル疎通、実機の同一回線、Wi-Fi → 携帯、携帯 → Wi-Fi を別々に PASS / FAIL / 未確認で記す。
- 失敗した場合は、応答停止か、要求エラーか、接続 ID の変更かを区別する。

## 準備段階の検証（2026-10-22）

- macOS: `UNISON_LIVE=1 swift test` で通常テストと既存 mock との QUIC 往復が通過。
  `FIELD_LIVE` と手動 handover はこの実行では無効。
- 上記の専用サーバーと証明書 pin を使用し、3要求の連番・接続ID・チャネルIDの維持を確認。
  回線変更は行っていない。
- `cargo clippy -p club-unison --example handover_probe_server -- -D warnings` と
  example の rustfmt 検査が通過。
- `xcodebuild -scheme UnisonClient -destination 'generic/platform=iOS'
  CODE_SIGNING_ALLOWED=NO build-for-testing` が成功。実機向けコンパイルのみで、
  署名・端末インストール・Wi-Fi / 携帯回線の切り替えは未確認。

- 公開 scratch サーバーへの配置後、macOS の Wi-Fi から証明書 pin 付きで10要求が通過。
  接続ID・チャネルID・受信連番の維持をクライアントとサーバー両方のログで確認。
  公開 IP 宛ての経路は `en0`。この検証でも回線変更は行っていない。

iPhone の Wi-Fi / 携帯回線切り替えが次の段階。検証後は対象の名前・IDを最終確認し、
専用の VM・ディスク・packet filter と Tailscale 登録を削除して、残存がないことを確認する。
停止状態で検証用 VM を残さない。
