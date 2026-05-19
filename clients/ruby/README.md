# unison-client (Ruby)

[Unison protocol](https://github.com/chronista-club/club-unison) の Ruby client。

## アーキテクチャ

これは **言語バインディング**であって protocol の再実装ではない。
QUIC トランスポート・channel 多重化・wire framing は Rust の `club-unison`
crate が実装しており、この gem はそれを **Magnus**（Rust 製 Ruby native
extension）経由で薄く包む。

```
Ruby (require "unison")
  └─ native ext  (Magnus, ext/unison_client/)
       └─ club-unison crate  (ProtocolClient — QUIC / channel / wire)
```

理由: Ruby に成熟した native QUIC スタックが無いため、protocol を Ruby で
再実装するより Rust core を FFI で binding する方が経済的。TS SDK は browser
の WebTransport に乗れたので完全再実装したが、Ruby にはその前提が無い。

## 状態

接続ライフサイクルと channel 層を実装済み。

```ruby
require "unison"

client = Unison::Client.new
client.connect("https://127.0.0.1:4433")

ch = client.open_channel("greeter")
ch.request("Hello", { "name" => "Mako" })   #=> レスポンス Hash
ch.send_event("Ping", { "seq" => 1 })        # 応答不要
ch.recv                                      # 次の event を待つ（Hash）
ch.close

client.disconnect
```

channel payload は native な Ruby 値（`Hash` / `Array` / scalar）で渡せる。
Rust 側で `serde_magnus` が `serde_json::Value` へ双方向変換し、channel の
JSON codec が処理する。

async は extension 内に埋めた tokio runtime で `block_on` する。現状は呼び出し中
Ruby VM をブロックする（GVL 解放は今後の refinement。特に `recv` は event 到着
まで待つため影響が大きい）。

次フェーズ: 実 Unison サーバ相手の E2E テスト、GVL 解放、`Unison::Error` 例外階層。

## ビルド・テスト

```
bundle install
bundle exec rake compile   # native 拡張をビルド
bundle exec rake test      # compile → minitest
```

**Ruby 3.4 以上が必須。** 開発環境の version は `.mise.toml` に固定（現在 3.4.9）。

## 対応 protocol 世代

`1.0.0-rc.1` — npm `@chronista-club/unison-client` / crates.io `club-unison`
と同世代。
