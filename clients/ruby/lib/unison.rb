# frozen_string_literal: true

require_relative "unison/version"

# Load the Rust native extension (built by `rake compile` into
# `lib/unison_client/`). It defines the Rust-backed `Unison` module functions.
require "unison_client/unison_client"

# Unison — a Ruby client for the Unison protocol.
#
# This gem is a thin **language binding**, not a re-implementation: the
# protocol itself (QUIC transport, channel multiplexing, wire framing) lives
# in the Rust `club-unison` crate. The native extension (Magnus) wraps that
# crate's `ProtocolClient`.
#
# `Unison::Client` wraps the crate's `ProtocolClient` (connection lifecycle:
# `new` / `connect` / `connected?` / `disconnect` / `open_channel`), and
# `Unison::Channel` wraps `UnisonChannel` (`request` / `send_event` / `recv` /
# `close`). Channel payloads are native Ruby values.
#
# Connection-level auth (= v1.4.0) は native ext を変更せず **pure Ruby** で実装する。
# auth は `unison.auth` channel を open して `Authenticate` request を送るだけなので、
# 既存の `connect` / `open_channel` / `Channel#request` の上に積める。native ext は
# `club-unison` の公開版に対してビルドされるため、Rust 側 `connect_with_credential`
# (= 未公開 1.4.0) に依存しないこの形が decoupled で都合がよい。
# 設計: design/connection-auth.md §5.8。
module Unison
  # reserved auth channel 名 (= Rust `network::auth::AUTH_CHANNEL_NAME`)。
  AUTH_CHANNEL_NAME = "unison.auth"
  # credential 提示 method (= Rust `network::auth::AUTHENTICATE_METHOD`)。
  AUTHENTICATE_METHOD = "Authenticate"

  # native ext が定義した `Unison::Client` を再オープンして auth helper を足す。
  class Client
    # connect してから credential を 1 回提示して認証する
    # (= Rust `connect_with_credential` の対応物、 connection-level authN)。
    #
    # `credential` は String (= Creo ID JWT / API キー / 独自トークン、 binary 可)。
    # wire には **u8 数値配列** (= `String#bytes`) で運ばれ、 Rust の `Vec<u8>` と
    # 一致する。認証が拒否 (= `ok` が false) されたら `Unison::Error` を raise する。
    # server が `enable_auth` 未設定なら `unison.auth` の open が reject される。
    #
    # 認証成功後は他 channel が per-message gate を通過できる。**他 channel を open
    # する前に呼ぶこと**。
    def connect_with_credential(url, credential)
      connect(url)
      channel = open_channel(AUTH_CHANNEL_NAME)
      begin
        result = channel.request(AUTHENTICATE_METHOD, { "credential" => credential.bytes })
        raise Error, "authentication denied by server verifier" unless result["ok"]
      ensure
        channel.close
      end
    end
  end
end
