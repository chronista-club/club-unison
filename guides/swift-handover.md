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

Swift Package 単体のテストは物理 iPhone で実行できないため、
`clients/swift/HandoverHost` のアプリをテストホストに使う。
このホストは同じ `HandoverLiveTests.swift` を読み込む。

1. XcodeGen を用意し（`brew install xcodegen`）、repo root から次を実行する。

```bash
export UNISON_DEVELOPMENT_TEAM='YOUR_APPLE_TEAM_ID'
export UNISON_HANDOVER_HOST='YOUR_SERVER_HOST'
export UNISON_HANDOVER_CERT_HEX='YOUR_SERVER_CERT_HEX'
cd clients/swift/HandoverHost
xcodegen generate --spec project.json
open UnisonHandover.xcodeproj
```

生成された project は Git 管理外。サーバーの pin を更新したら再生成する。

2. `Handover` scheme を選ぶ。iPhone は USB 接続にし、Developer Mode / 信頼 / 署名の設定を済ませる。
   Wi-Fi デバッグ接続自体を切ってしまうことを避ける。Simulator は携帯回線検証に使わない。
   Mac の「一般 → 共有 → コンテンツキャッシュ → インターネット接続」がオンだと、
   iPhone は USB 経由で Mac の回線を使う場合がある。`PATH` の `wired=true` はこの
   検証では除外する。共有の一時停止は利用者と調整し、検証後に元の設定へ戻す。
   端末の 4G / 5G 表示だけで携帯回線への移行を判定しない。
3. Scheme → Edit Scheme → Test → Arguments の環境変数を確認する。
   上記の生成時に次の値が設定される。

| 環境変数 | 値 |
|---|---|
| `UNISON_HANDOVER` | `1` |
| `UNISON_HANDOVER_HOST` | 両回線から届くホスト名または公開 IP（URL ではない） |
| `UNISON_HANDOVER_PORT` | `17878` |
| `UNISON_HANDOVER_CERT_HEX` | サーバーが表示した pin の `=` より後の文字列 |
| `UNISON_HANDOVER_SAMPLES` | `120`（2〜300、応答後に1秒待つ） |

4. ロック解除した実機を destination にし、Test navigator の `HandoverDeviceTests` から
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
要求ループの期限は `samples + 90` 秒で、期限切れをテスト失敗として記録し、
チャネルを明示的に閉じて停止中の要求を解除する。接続の group cancellation だけでは
既存 stream の終了を保証しない。接続・チャネルを開く段階はこの期限の対象外なので、
そこで停止した場合は Xcode の Stop で終了し、その段階を記録する。
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

## iPhone 実機の結果（2026-10-22）

iPhone 16 Pro / iOS 27.2、USB デバッグ、端末 VPN オフで実施。
サーバーは Quinn のまま。ホストアプリの署名・インストール・実機実行を確認した。
Mac の USB 向けインターネット共有を一時停止し、終了後に元のオンへ戻した。

最終試験 `iphone-handover4` は **PASS**。16:55:19〜17:00:30 JST に300要求を実行し、
Wi-Fi → 携帯回線 → Wi-Fi をまたいで、両側のログで `seq` / `received` の1〜300と
次の ID が一貫していることを確認した。再接続・チャネル再作成は行っていない。

- connection: `963d07f4-4b52-4135-aa2e-9999139a528d`
- channel: `69db2d97-e34b-403c-a0ac-338551e12ea0`
- サーバーの UDP 受信元: Wi-Fi 側 → 16:55:50 JST に携帯側 → 16:56:17 JST に Wi-Fi 側。
  `PATH` も `en0` → `pdp_ip0` → `en0`、全期間 `wired=false` を確認。
- 最大 RTT: 3.574 秒。最大応答間隔: 4.629 秒（通常の1秒待機を含む）。
  接続継続は確認できたが、無停止・一定遅延を保証する結果ではない。
- 最終試験では期限切れなし。300要求のテスト終了も成功。

初回の120要求は USB 共有経由だったため、handover の証拠から除外した。
別の長い試験では期限切れをログ出力するだけで成功扱いになる検証コードの不備が判明した。
期限切れの失敗記録と明示的なチャネル close を追加し、1 ms の期限を意図的に入れた確認で
要求が解除され、成功扱いにならないことを確認。通常の期限へ戻して Swift テスト25件
（live の一部は opt-in のため skip）と、署名付き iOS host build-for-testing が通過した。
この検証コード修正は SDK の通信実装を変更していない。

検証後は対象の名前・IDを最終確認し、
専用の VM・ディスク・packet filter と Tailscale 登録を削除して、残存がないことを確認する。
停止状態で検証用 VM を残さない。
