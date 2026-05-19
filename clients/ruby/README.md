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
client.connect("quic://[::1]:7878")

ch = client.open_channel("greeter")
ch.request("Hello", { "name" => "Mako" })   #=> レスポンス Hash
ch.send_event("Ping", { "seq" => 1 })        # 応答不要
ch.recv                                      # 次の event を待つ（Hash）
ch.close

client.disconnect
```

> **注意**: `Unison::Client.new` は証明書検証を行わない insecure な client を
> 構築する（loopback / 開発用途）。trust anchor を明示する secure constructor は
> 今後のフェーズ。

channel payload は native な Ruby 値（`Hash` / `Array` / scalar）で渡せる。
Rust 側で `serde_magnus` が `serde_json::Value` へ双方向変換し、channel の
JSON codec が処理する。

async は extension 内に埋めた tokio runtime で `block_on` する。ブロッキング
呼び出しは `rb_thread_call_without_gvl` で **GVL を解放**するため、待機中も他の
Ruby スレッドは動き続ける（呼び出し自体の中断・タイムアウトは未対応 — 今後の
refinement）。

失敗はすべて `Unison::Error`（`< StandardError`）として raise される。

次フェーズ: GVL 解放中の呼び出しの中断（unblock function）、`recv` の timeout 版。

## ビルド・テスト

```
bundle install
bundle exec rake compile    # native 拡張をビルド
bundle exec rake test       # compile → 単体テスト（ネットワーク不要）
bundle exec rake test:e2e   # compile → E2E（`unison mock` を subprocess 起動）
```

`rake test:e2e` は `unison` バイナリ（`cargo build -p unison-cli` で生成、または
`UNISON_MOCK_BIN` で指定）を要する。見つからない場合は skip される。

**Ruby 3.4 以上が必須。** 開発環境の version は `.mise.toml` に固定（現在 3.4.9）。

## ベンチマーク

```
ruby bench/bench.rb > bench/runs/<date>-<tag>.kdl
```

`unison mock` を subprocess 起動し、(1) `Channel#request` の RTT / throughput と
(2) GVL 解放の効果（ブロッキング呼び出し中に背景スレッドが進む割合）を計測し、
structured-log KDL を出力する。run は `bench/runs/` に immutable に蓄積し、
`bench/index.kdl` が append-only インデックスとして参照する。

## 対応 protocol 世代

`1.0.0-rc.1` — npm `@chronista-club/unison-client` / crates.io `club-unison`
と同世代。
