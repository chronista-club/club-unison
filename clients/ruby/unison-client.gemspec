# frozen_string_literal: true

require_relative "lib/unison/version"

Gem::Specification.new do |spec|
  spec.name = "unison-client"
  spec.version = Unison::VERSION
  spec.authors = ["Mako"]
  spec.email = ["mito@chronista.club"]

  spec.summary = "Ruby client for the Unison protocol."
  spec.description =
    "A thin Ruby binding over the Rust `club-unison` crate's ProtocolClient, " \
    "via a Magnus native extension. The protocol — QUIC transport, channel " \
    "multiplexing, wire framing — is implemented in Rust; this gem is the " \
    "language binding."
  spec.homepage = "https://github.com/chronista-club/club-unison"
  spec.license = "MIT"
  spec.required_ruby_version = ">= 3.4.0"

  spec.files = Dir[
    "lib/**/*.rb",
    "ext/**/*.{rs,rb,toml}",
    "Cargo.toml",
    "Cargo.lock",
    "README.md",
  ]
  spec.require_paths = ["lib"]
  spec.extensions = ["ext/unison_client/extconf.rb"]

  # Rust native-extension build toolchain.
  spec.add_dependency "rb_sys", "~> 0.9"
end
