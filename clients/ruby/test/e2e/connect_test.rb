# frozen_string_literal: true

require "minitest/autorun"
require "socket"
require "tempfile"
require "unison"

# 実 Unison サーバ（`unison mock`）相手の E2E テスト。
#
# `unison mock` を subprocess で起動し、Ruby client から QUIC 接続して
# connect / open_channel / request / send_event / disconnect を検証する。
#
# `unison` バイナリが見つからない場合は skip するので、binary 未ビルドの環境
# でも `rake test:e2e` は緑のまま（バイナリは `cargo build -p unison-cli` で
# 生成、または `UNISON_MOCK_BIN` で明示）。
class ConnectE2ETest < Minitest::Test
  SCHEMA = File.expand_path("../fixtures/ping_pong.kdl", __dir__)

  def setup
    @bin = find_unison_bin
    skip "unison binary not found — build it: cargo build -p unison-cli" unless @bin

    @port = free_udp_port
    @addr = "[::1]:#{@port}"
    @log = Tempfile.new("unison-mock")
    @server = spawn(@bin, "mock", "--schema", SCHEMA, "--addr", @addr,
                    out: @log.path, err: [:child, :out])
    wait_until_listening
  end

  def teardown
    if @server
      Process.kill("TERM", @server)
      Process.wait(@server)
    end
  rescue Errno::ESRCH, Errno::ECHILD
    # プロセスは既に終了・回収済み
  ensure
    # rescue 経路でも Tempfile を確実に unlink する。
    @log&.close!
  end

  def test_connect_open_channel_request_disconnect
    client = Unison::Client.new
    client.connect("quic://#{@addr}")
    assert client.connected?, "client should be connected after #connect"

    ch = client.open_channel("ping-pong")

    # `unison mock` は schema の `returns` 型から決定的な stub を返す
    # （string→"" / int→0 / json→{}）。
    assert_equal({ "reply" => "", "timestamp" => "" },
                 ch.request("Ping", { "message" => "hello" }))
    assert_equal({ "status" => "", "uptime_ms" => 0 },
                 ch.request("Health", {}))
    assert_equal({ "data" => {} },
                 ch.request("Echo", { "data" => { "k" => "v" } }))

    # fire-and-forget event — mock は受け流すだけ。送信が raise しないこと。
    ch.send_event("Ping", { "message" => "evt" })

    ch.close
    client.disconnect
    refute client.connected?, "client should be disconnected after #disconnect"
  end

  private

  # `unison` バイナリを探す: UNISON_MOCK_BIN → target/release → target/debug。
  def find_unison_bin
    env = ENV["UNISON_MOCK_BIN"]
    return env if env && File.executable?(env)

    root = File.expand_path("../../../..", __dir__) # club-unison repo root
    %w[release debug].each do |profile|
      path = File.join(root, "target", profile, "unison")
      return path if File.executable?(path)
    end
    nil
  end

  # QUIC は UDP。空き UDP ポートを 1 つ確保して返す。
  def free_udp_port
    sock = UDPSocket.new(Socket::AF_INET6)
    sock.bind("::1", 0)
    sock.addr[1]
  ensure
    sock&.close
  end

  # mock が "listening on" を出力するまで待つ。即死した場合はログ付きで失敗。
  def wait_until_listening(timeout: 15)
    deadline = Time.now + timeout
    loop do
      raise "unison mock did not become ready within #{timeout}s" if Time.now > deadline
      return if File.read(@log.path).include?("listening on")

      if Process.wait2(@server, Process::WNOHANG)
        @server = nil
        raise "unison mock exited early:\n#{File.read(@log.path)}"
      end
      sleep 0.1
    end
  end
end
