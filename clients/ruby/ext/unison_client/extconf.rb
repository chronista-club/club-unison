# frozen_string_literal: true

# Generates a Makefile that builds the Rust crate in this directory into the
# gem's native extension, via rb-sys.
require "mkmf"
require "rb_sys/mkmf"

create_rust_makefile("unison_client/unison_client")
