# Quinn Stream API 完全ガイド

## 概要

QuinnはRust向けのQUIC実装で、`SendStream`（送信ストリーム）と`RecvStream`（受信ストリーム）を通じて高性能な非同期通信を提供します。本ドキュメントでは、これらのストリームAPIの詳細と効果的な使用方法を解説します。

内容は **quinn 0.11 時点** のもの（club-unison の workspace が `quinn = "0.11"` を使っている）。

## SendStream - 送信ストリーム

### 基本的な書き込みメソッド

#### `write(&mut self, buf: &[u8]) -> Result<usize>`
部分的な書き込みを行い、実際に書き込まれたバイト数を返します。

```rust
// バッファに空きがない場合、書き込める分だけ書き込む
let bytes_written = send_stream.write(b"Hello").await?;
```

#### `write_all(&mut self, buf: &[u8]) -> Result<()>`
すべてのデータが書き込まれるまで待機する完全な書き込みです。

```rust
send_stream.write_all(b"Hello World").await?;
// 内部でwrite()を繰り返し呼び出し
```

#### `write_chunk(&mut self, bytes: Bytes) -> Result<()>`
ゼロコピー書き込みを実現し、`Bytes`型のデータを直接書き込みます。

```rust
use bytes::Bytes;
let data = Bytes::from_static(b"efficient data");
send_stream.write_chunk(data).await?;
```

#### `write_chunks(&mut self, bufs: &mut [Bytes]) -> Result<Written>`
複数のチャンクを一度に書き込むメソッドです。輻輳制御・フロー制御により一部だけ書かれることがあり、`Written` に書けたチャンク数・バイト数が入ります。全部書き切るなら `write_all_chunks` を使います。

```rust
let mut chunks = vec![
    Bytes::from("chunk1"),
    Bytes::from("chunk2"),
];
let written = send_stream.write_chunks(&mut chunks).await?;
```

### ストリーム制御メソッド

#### `finish(&mut self) -> Result<()>`
ストリームを正常終了し、FINフラグを送信します。

```rust
send_stream.finish()?;
// これ以降write()は使えない
```

#### `reset(&mut self, error_code: VarInt) -> Result<(), ClosedStream>`
ストリームを異常終了し、RESETフレームを送信します。

```rust
send_stream.reset(VarInt::from_u32(1))?;
// 未送信データは破棄される
```

#### `stopped(&mut self) -> Result<Option<VarInt>>`
相手側がストリームを停止したか確認します。

```rust
if let Some(error_code) = send_stream.stopped().await? {
    println!("Stream stopped by peer with code: {}", error_code);
}
```

#### `set_priority(&self, priority: i32) -> Result<()>` / `priority(&self) -> Result<i32>`
ストリームの優先度を設定・取得します（`i32`、既定 0。値が大きいストリームのデータほど先に送られる）。

```rust
send_stream.set_priority(100)?;  // 高優先度設定
let current = send_stream.priority()?;
```

## RecvStream - 受信ストリーム

### 基本的な読み取りメソッド

#### `read(&mut self, buf: &mut [u8]) -> Result<Option<usize>>`
バッファにデータを読み込みます。`None`はストリームの終了を示します。

```rust
let mut buf = [0u8; 1024];
match recv_stream.read(&mut buf).await? {
    Some(n) => println!("Read {} bytes", n),
    None => println!("Stream finished"),
}
```

#### `read_exact(&mut self, buf: &mut [u8]) -> Result<()>`
指定したバイト数を正確に読み込みます。

```rust
let mut header = [0u8; 4];
recv_stream.read_exact(&mut header).await?;
// 4バイト読むまでブロック
```

#### `read_chunk(&mut self, max_length: usize, ordered: bool) -> Result<Option<Chunk>>`
チャンク単位で読み込む低レベルAPIです。

```rust
while let Some(chunk) = recv_stream.read_chunk(4096, true).await? {
    let offset = chunk.offset;  // ストリーム内のオフセット
    let bytes = chunk.bytes;    // Bytes型のデータ
    println!("Received chunk at offset {}: {} bytes", offset, bytes.len());
}
```

#### `read_chunks(&mut self, bufs: &mut [Bytes]) -> Result<Option<usize>>`
複数のチャンクを一度に読み込みます。

```rust
// 読む数だけ要素を用意する（長さ 0 のスライスを渡すと何も読まずに Some(0) が返る）
let mut chunks = vec![Bytes::new(); 10];
if let Some(count) = recv_stream.read_chunks(&mut chunks).await? {
    println!("Read {} chunks", count);
}
```

#### `read_to_end(&mut self, size_limit: usize) -> Result<Vec<u8>>`
ストリーム全体を読み込みます（サイズ制限付き）。

```rust
let all_data = recv_stream.read_to_end(1024 * 1024).await?; // 1MB制限
```

### ストリーム制御メソッド

#### `stop(&mut self, error_code: VarInt) -> Result<()>`
受信を停止し、STOP_SENDINGフレームを送信します。

```rust
recv_stream.stop(VarInt::from_u32(2))?;
```

#### `received_reset(&mut self) -> Result<Option<VarInt>>`
相手がストリームをリセットしたか確認します。

```rust
if let Some(error_code) = recv_stream.received_reset().await? {
    println!("Stream reset by peer: {}", error_code);
}
```

## 双方向ストリーム（Bidirectional Stream）

### クライアント側での開設
```rust
let (mut send_stream, mut recv_stream) = connection.open_bi().await
    .context("Failed to open bidirectional QUIC stream")?;
```

### サーバー側での受け入れ
```rust
match connection.accept_bi().await {
    Ok((mut send_stream, mut recv_stream)) => {
        // 双方向ストリームの処理
    }
    Err(e) => {
        // 接続が閉じられた等
        eprintln!("accept_bi failed: {}", e);
    }
}
```

## 複数チャンクをまとめて書く（`write_chunks`）

### 概念
`write_chunks` / `write_all_chunks` は、複数の `Bytes` を 1 回の呼び出しでストリームに積むメソッドです。名前は Scatter-Gather I/O に似ていますが、quinn では呼び出し回数とシステムコールの回数は対応しません。

### write の回数と送信の関係

quinn の `write` 系メソッドは、データをコネクションの送信バッファに積んでコネクションの driver を起こすだけで、その場では送信しません。UDP パケットをいつ・何個組み立てて送るかは driver が輻輳制御・フロー制御に従って決めます。複数回の write が 1 パケットにまとまることも、1 回の write が複数パケットに分かれることもあり、「write 3 回 = システムコール 3 回」にはなりません。

#### 分けて書く
```rust
// 呼び出しごとにコネクションのロックを取り、&[u8] を送信バッファへコピーする
send_stream.write_all(&header).await?;
send_stream.write_all(&metadata).await?;
send_stream.write_all(&payload).await?;
```

#### まとめて書く
```rust
// 呼び出しは 1 回。Bytes は参照カウントを渡すだけで中身をコピーしない
let mut chunks = [header, metadata, payload]; // いずれも Bytes
send_stream.write_all_chunks(&mut chunks).await?;
```

### まとめて書く利点

1. **呼び出し回数とロック取得が減る**
   - `write` 系は呼び出しごとにコネクション状態のロックを取る。まとめれば（フロー制御で待たされなければ）1 回で済む

2. **コピーしない**
   - `write` / `write_all` は渡した `&[u8]` を送信バッファ用にコピーする
   - `write_chunk` / `write_chunks` は `Bytes` をそのまま（参照カウントで）受け取る

3. **送信回数は driver が決める**
   - まとめて書いても UDP の送信回数が減るとは限らない。パケットの組み立ては driver の仕事

### 実装例：プロトコルメッセージの送信

```rust
async fn send_message(stream: &mut SendStream, header: Bytes, body: Bytes) -> anyhow::Result<()> {
    // 長さ prefix だけは新しく作る（4 バイトのコピー）。header / body は Bytes のまま渡す
    let len = Bytes::copy_from_slice(&(body.len() as u32).to_be_bytes());
    let mut chunks = [header, len, body];

    // write_chunks は一部しか書けないことがあるので、書き切る write_all_chunks を使う
    stream.write_all_chunks(&mut chunks).await?;
    Ok(())
}
```

## 高度な使用パターン

### ゼロコピー転送
```rust
// 送信側
let data = Bytes::from_static(b"zero-copy data");
send_stream.write_chunk(data).await?;

// 受信側
if let Some(chunk) = recv_stream.read_chunk(8192, true).await? {
    // chunk.bytesは参照カウント方式で効率的
    process_bytes(chunk.bytes);
}
```

### 大きなファイルのストリーミング
```rust
let mut file = tokio::fs::File::open("large.dat").await?;
let mut buf = vec![0u8; 64 * 1024]; // 64KB buffer

loop {
    let n = file.read(&mut buf).await?;
    if n == 0 { break; }
    send_stream.write_all(&buf[..n]).await?;
}
send_stream.finish()?;
```

### 優先度制御
```rust
// 重要なデータを先に送信
let mut important = connection.open_uni().await?;
important.set_priority(200)?;  // 高優先度

let mut normal = connection.open_uni().await?;
normal.set_priority(0)?;  // 通常優先度
```

### 詳細なエラーハンドリング
```rust
match recv_stream.read(&mut buf).await {
    Ok(Some(n)) => { /* データ処理 */ },
    Ok(None) => { /* 正常終了 */ },
    Err(quinn::ReadError::Reset(code)) => {
        // 相手がリセット
        println!("Stream reset with code: {}", code);
    },
    Err(e) => {
        // その他のエラー
        eprintln!("Read error: {}", e);
    }
}
```

## パフォーマンス最適化のポイント

1. **ゼロコピー転送**: `write_chunk`/`read_chunk`を使用
2. **バッチ処理**: `write_chunks`/`read_chunks`で複数チャンクを一括処理
3. **適切なバッファサイズ**: 大きなデータは`write`のループで制御
4. **優先度管理**: `set_priority`で重要なストリームを優先
5. **メモリ制限**: `read_to_end`は小さなデータのみに使用

## ストリームの特徴

- **非同期**: すべての操作は`async/await`で非同期実行
- **順序保証**: 同一ストリーム内のデータは順序が保証される
- **独立性**: 複数のストリームは互いに独立して動作
- **フロー制御**: QUICレベルで自動的にフロー制御
- **エラー分離**: 個別のストリームエラーは接続全体に影響しない

## 使用上の注意点

1. **メモリ使用量**: 多くのチャンクを保持するとメモリ使用量が増える
2. **レイテンシー vs スループット**: バッチ処理は待機時間を生む可能性
3. **プラットフォーム依存**: OSによって最適なチャンクサイズが異なる

このドキュメントは、QuinnのストリームAPIを効果的に使用するための包括的なガイドです。プロジェクトの要件に応じて、適切なメソッドと最適化手法を選択してください。