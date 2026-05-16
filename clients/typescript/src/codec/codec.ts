/**
 * Codec 抽象 (= Phase 2d)。
 *
 * `Codec` は channel payload (= `ProtocolMessage.payload` バイト列) の
 * encode/decode を担う。 wire の packet framing は transport 層の責務であり、
 * codec はその内側のアプリケーションメッセージ部分のみを扱う。
 *
 * Rust 側の `Encodable<C>` / `Decodable<C>` トレイトペアに対応する。 channel は
 * `Codec<T>` を 1 個保持し、 codec を差し替えることで JSON / protobuf wire を
 * 切り替える (= `UnisonChannel<M>` が codec に対してジェネリック)。
 */

/** codec layer error の基底 (= encode/decode 双方の失敗を表す) */
export class CodecError extends Error {
  constructor(message: string, options?: ErrorOptions) {
    super(message, options);
    this.name = new.target.name;
  }
}

/**
 * メッセージ値 `T` を wire バイト列に相互変換する codec。
 *
 * 1 個の codec instance は 1 種類のメッセージ型に束縛される。 `JsonCodec` は
 * 構造的に任意の値を扱えるため共有可能だが、 `ProtoCodec` は proto descriptor
 * ごとに instance を持つ (= buf protobuf が descriptor 駆動のため)。
 */
export interface Codec<T> {
  /** wire format 識別子 (= "json" / "proto"、 診断・negotiation 用) */
  readonly format: CodecFormat;
  /** 値をバイト列にエンコード (= 失敗時 `CodecError`) */
  encode(value: T): Uint8Array;
  /** バイト列を値にデコード (= 失敗時 `CodecError`) */
  decode(bytes: Uint8Array): T;
}

/** 対応 wire format (= connection-level codec 選択肢、 design §5) */
export type CodecFormat = "json" | "proto";
