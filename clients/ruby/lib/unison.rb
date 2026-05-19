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
module Unison
end
