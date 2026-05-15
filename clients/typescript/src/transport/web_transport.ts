/**
 * WebTransport adapter (= Phase 2b scaffold)。
 *
 * 全 method は stub (= "not yet implemented (Phase 2b WIP)" throw)。 型 surface のみ
 * 確定、 実 impl は cert pinning / trust mode / reconnect 等の hearing 後に着手する。
 */

import type {
  BidiStream,
  Connection,
  ConnectionEvent,
  ConnectOptions,
  Transport,
  UniStream,
} from "./types.js";

const NOT_IMPL = "not yet implemented (Phase 2b WIP)";

/** WebTransport-backed Connection の concrete impl */
export class WebTransportConnection implements Connection {
  readonly #url: string;
  readonly #opts: ConnectOptions;

  constructor(url: string, opts: ConnectOptions) {
    this.#url = url;
    this.#opts = opts;
  }

  /** 接続先 URL (= debug / log 用に expose) */
  get url(): string {
    return this.#url;
  }

  /** 構築時 opts (= debug / introspection 用) */
  get options(): ConnectOptions {
    return this.#opts;
  }

  openBidiStream(): Promise<BidiStream> {
    throw new Error(NOT_IMPL);
  }

  acceptUniStreams(): AsyncIterable<UniStream> {
    throw new Error(NOT_IMPL);
  }

  sendDatagram(_payload: Uint8Array): Promise<void> {
    throw new Error(NOT_IMPL);
  }

  datagrams(): AsyncIterable<Uint8Array> {
    throw new Error(NOT_IMPL);
  }

  events(): AsyncIterable<ConnectionEvent> {
    throw new Error(NOT_IMPL);
  }

  close(_reason?: string): Promise<void> {
    throw new Error(NOT_IMPL);
  }
}

/** WebTransport `Transport` 実装 (= SDK default) */
export class WebTransportClient implements Transport {
  connect(_opts: ConnectOptions): Promise<Connection> {
    throw new Error(NOT_IMPL);
  }
}

/** Convenience factory (= caller がこれを呼ぶ pattern が main) */
export function connect(url: string, opts: ConnectOptions): Promise<Connection> {
  void new WebTransportConnection(url, opts); // 型 surface の usage 例
  throw new Error(NOT_IMPL);
}
