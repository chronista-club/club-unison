/**
 * Channel 抽象 (= Phase 2c)。
 *
 * 全通信は channel 経由 (RPC は廃止)。 channel には 2 系統ある:
 * - `UnisonChannel` — stream backend (= QUIC bidi stream)、 request/response + event
 * - `DatagramChannel` — datagram backend (= 共有 datagram path)、 broadcast event のみ
 *
 * 両 interface は Phase 1 codegen が出力する `<Name>ChannelMeta` const に対して
 * generic。 `as const` literal narrowing により、 caller code で event 名 /
 * request 名が compile-time に絞り込まれる。
 *
 * codegen は meta に **phantom `__types` field** を埋め込む (= 生成 interface への
 * link)。 `EventType<M>` / `RequestType<M,N>` / `ResponseType<M,N>` はこの field
 * 経由で event/request 名 → 実 interface を解決する (= design doc §3.2/§4.2)。
 */

/**
 * meta が `__types` 経由で運ぶ payload 型 map。 codegen が
 * `<Channel>ChannelEventTypes` / `<Channel>ChannelRequestTypes` を生成し、
 * これを `{ events; requests }` として束ねる。 runtime 値は `undefined`。
 */
export interface ChannelTypeMap {
  readonly events: Readonly<Record<string, unknown>>;
  readonly requests: Readonly<
    Record<string, { request: unknown; response: unknown }>
  >;
}

/** Stream backend の channel meta 形状 (= Phase 1 codegen 出力の構造的 subset) */
export interface ChannelMeta {
  readonly name: string;
  readonly backend: "stream";
  readonly from: "client" | "server" | "either";
  readonly lifetime: "transient" | "persistent";
  readonly events: readonly string[];
  readonly requests: Readonly<
    Record<string, { readonly request: string; readonly response: string }>
  >;
  /** codegen 埋め込みの phantom 型 carrier (= runtime undefined、 型のみ) */
  readonly __types?: ChannelTypeMap;
}

/** Datagram backend の channel meta 形状 (= `channelId` 必須、 `requests` 空) */
export interface DatagramChannelMeta {
  readonly name: string;
  readonly backend: "datagram";
  readonly channelId: number;
  readonly from: "client" | "server" | "either";
  readonly lifetime: "transient" | "persistent";
  readonly events: readonly string[];
  readonly requests: Readonly<Record<string, never>>;
  /** codegen 埋め込みの phantom 型 carrier (= runtime undefined、 型のみ) */
  readonly __types?: ChannelTypeMap;
}

/** meta の `events` 配列から event 名 union を導出 */
export type EventName<M> = M extends { events: readonly (infer N)[] }
  ? N & string
  : never;

/** meta の `requests` map から request 名 union を導出 */
export type RequestName<M> = M extends { requests: infer R }
  ? keyof R & string
  : never;

/**
 * Channel payload の構造型 fallback。 codegen meta を渡さなかった場合
 * (= raw `ChannelMeta` で開いた場合) の event/request payload 型。
 */
export type ChannelPayload = Record<string, unknown>;

/**
 * meta `M` の全 event payload 型 union (= `events()` が yield する型)。
 * `__types` を持つ生成 meta なら実 interface の union、 持たなければ
 * `ChannelPayload` に degrade する。
 */
export type EventType<M> = M extends { __types?: infer T }
  ? T extends ChannelTypeMap
    ? T["events"][keyof T["events"]]
    : ChannelPayload
  : ChannelPayload;

/** meta `M` の event `N` の payload 型 (= `sendEvent()` 引数) */
export type EventPayload<M, N extends PropertyKey> = M extends {
  __types?: infer T;
}
  ? T extends ChannelTypeMap
    ? N extends keyof T["events"]
      ? T["events"][N]
      : ChannelPayload
    : ChannelPayload
  : ChannelPayload;

/** meta `M` の request `N` の request payload 型 */
export type RequestType<M, N extends PropertyKey> = M extends {
  __types?: infer T;
}
  ? T extends ChannelTypeMap
    ? N extends keyof T["requests"]
      ? T["requests"][N]["request"]
      : ChannelPayload
    : ChannelPayload
  : ChannelPayload;

/** meta `M` の request `N` の response payload 型 */
export type ResponseType<M, N extends PropertyKey> = M extends {
  __types?: infer T;
}
  ? T extends ChannelTypeMap
    ? N extends keyof T["requests"]
      ? T["requests"][N]["response"]
      : ChannelPayload
    : ChannelPayload
  : ChannelPayload;

/**
 * Stream channel — request/response + server-pushed event。
 *
 * QUIC bidi stream に対応、 ordered + reliable。 `request()` は length-prefixed
 * frame を送り response frame を await、 `events()` は server push を AsyncIterable
 * で配る。 payload 型は meta の `__types` 経由で生成 interface に narrow される。
 */
export interface UnisonChannel<M extends ChannelMeta = ChannelMeta> {
  /** KDL schema 上の channel 名 */
  readonly name: M["name"];

  /**
   * Request を送り response を await する (= ordered/reliable)。
   * `name` は `M["requests"]` の key に narrow、 戻り値は `ResponseType<M, N>`。
   */
  request<N extends RequestName<M>>(
    name: N,
    payload: RequestType<M, N>,
  ): Promise<ResponseType<M, N>>;

  /** Server push event の購読 (= `for await`、 break で channel close cascade) */
  events(): AsyncIterableIterator<EventType<M>>;

  /** Event を送信 (= client → server、 応答なし) */
  sendEvent<N extends EventName<M>>(
    name: N,
    payload: EventPayload<M, N>,
  ): Promise<void>;

  /** Channel を閉じる (= 配下 stream を tear down、 idempotent) */
  close(): Promise<void>;
}

/**
 * Datagram channel — broadcast event のみ (= request 不可)。
 *
 * 共有 datagram path 上の virtual stream、 `channelId` varint prefix で demux。
 * unordered + unreliable。 caller は基本 `events()` で subscribe するのみ。
 */
export interface DatagramChannel<
  M extends DatagramChannelMeta = DatagramChannelMeta,
> {
  /** KDL schema 上の channel 名 */
  readonly name: M["name"];

  /** schema-time fixed の demux 識別子 (= varint prefix として wire 出現) */
  readonly channelId: M["channelId"];

  /** Datagram broadcast event の購読 (= unordered/unreliable) */
  events(): AsyncIterableIterator<EventType<M>>;

  /** Event を datagram で送信 (= best-effort、 MTU 超過は reject) */
  sendEvent<N extends EventName<M>>(
    name: N,
    payload: EventPayload<M, N>,
  ): Promise<void>;

  /** Channel を閉じる (= dispatcher から unregister、 idempotent) */
  close(): Promise<void>;
}
