/**
 * Transport 抽象化 (= Phase 2b)。
 *
 * SDK は `Transport` を 1 個生成し、 `connect()` で `Connection` を取得する。
 * 現状の concrete impl は WebTransport のみ (= `./web_transport.ts`)、 将来
 * WebSocket fallback / Node native QUIC adapter を別 impl として差し込む余地を
 * 残すため interface で分離。
 */

/**
 * 接続の信頼モード (= TLS cert 検証の policy)。
 *
 * browser WebTransport の security model 上、 非 CA path は cert hash pinning
 * (= `serverCertificateHashes`) のみ。「全 cert 検証 skip」は browser に存在しない。
 */
export type TrustMode =
  | "system" // システム CA store による標準検証 (= default)
  | { certHash: string }; // 明示 cert pinning (= DER server cert の SHA-256、 hex 文字列)

/** `connect()` への入力 */
export interface ConnectOptions {
  /** 接続先 URL (= https://host:port、 WebTransport は https-only) */
  url: string;
  /** TLS trust policy (= default: "system") */
  trust?: TrustMode;
  /**
   * caller 制御の cancellation。 connection lifetime の kill-switch:
   * abort で connect() 中断、 確立後は connection + 配下全 stream を tear down。
   */
  signal?: AbortSignal;
}

/** Connection lifecycle event */
export type ConnectionEvent =
  | { type: "connected"; remoteAddr: string }
  | { type: "disconnected"; reason: string }
  | { type: "error"; error: Error };

/** Bidirectional stream (= QUIC bi-stream に対応) */
export interface BidiStream {
  readable: ReadableStream<Uint8Array>;
  writable: WritableStream<Uint8Array>;
  close(): Promise<void>;
}

/** 確立済み Connection */
export interface Connection {
  /** Bidi stream を open (= request/response channel 用) */
  openBidiStream(): Promise<BidiStream>;
  /** Datagram 1 件送信 (= MTU 超過は reject) */
  sendDatagram(payload: Uint8Array): Promise<void>;
  /** 受信 datagram の連続 stream */
  datagrams(): AsyncIterable<Uint8Array>;
  /** Connection event stream (= connected / disconnected / error) */
  events(): AsyncIterable<ConnectionEvent>;
  /** 明示 close (= 双方向、 reason 文字列を peer に伝達) */
  close(reason?: string): Promise<void>;
}

/** Transport factory (= concrete impl を SDK が選択) */
export interface Transport {
  connect(opts: ConnectOptions): Promise<Connection>;
}
