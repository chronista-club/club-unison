# frozen_string_literal: true

# Ruby client ベンチマーク。
#
# `unison mock` を subprocess 起動し、2 つを計測して structured-log KDL を
# stdout に出力する（進捗ログは stderr）:
#
#   1. channel suite — Channel#request の RTT / throughput
#   2. gvl suite     — GVL 解放の効果。背景スレッドの spin-count throughput を
#      main が ①idle ②Unison request ③純 Ruby CPU ループ の 3 シナリオで比較。
#      ② が高く ③ がほぼ 0 なら「network 待ち中に GVL を解放している」の証拠。
#
# 使い方:
#   ruby bench/bench.rb > bench/runs/<date>-<tag>.kdl
#
# `unison` バイナリが必要（`cargo build -p unison-cli`、または `UNISON_MOCK_BIN`）。

require "socket"
require "tempfile"

# `lib/` を load path に追加（unison.rb 内の `require "unison_client/..."` 用）。
$LOAD_PATH.unshift(File.expand_path("../lib", __dir__))
require "unison"

VERSION = "1.0.0-rc.2"
DATE = "2026-05-19"

def clock = Process.clock_gettime(Process::CLOCK_MONOTONIC)

def log(msg) = warn(msg)

def median(values) = values.sort[values.size / 2]

def find_unison_bin
  env = ENV["UNISON_MOCK_BIN"]
  return env if env && File.executable?(env)

  root = File.expand_path("../../..", __dir__)
  %w[release debug].each do |profile|
    path = File.join(root, "target", profile, "unison")
    return path if File.executable?(path)
  end
  abort "unison binary not found — build it: cargo build -p unison-cli"
end

def free_udp_port
  sock = UDPSocket.new(Socket::AF_INET6)
  sock.bind("::1", 0)
  sock.addr[1]
ensure
  sock&.close
end

# 背景スレッドに spin-count させ、`yield`（main の作業）の間に進んだ
# カウントと所要時間を返す。GVL を main が手放している割合がそのまま
# 背景スレッドの進捗に表れる。
def measure_background
  counter = 0
  stop = false
  bg = Thread.new do
    counter += 1 until stop
  end
  Thread.pass # 背景スレッドを確実に走らせてから計測へ
  started = clock
  yield
  elapsed = clock - started
  stop = true
  bg.join
  [counter, elapsed]
end

# --- mock サーバ起動 ---------------------------------------------------------
bin = find_unison_bin
schema = File.expand_path("../test/fixtures/ping_pong.kdl", __dir__)
port = free_udp_port
addr = "[::1]:#{port}"
mock_log = Tempfile.new("unison-bench-mock")
log "starting `unison mock` on #{addr} …"
server = spawn(bin, "mock", "--schema", schema, "--addr", addr,
               out: mock_log.path, err: %i[child out])

deadline = clock + 15
until File.read(mock_log.path).include?("listening on")
  abort "unison mock failed to start:\n#{File.read(mock_log.path)}" if clock > deadline
  sleep 0.05
end

begin
  client = Unison::Client.new
  client.connect("quic://#{addr}")
  channel = client.open_channel("ping-pong")
  payload = { "message" => "bench" }

  # === 1. request RTT / throughput ==========================================
  log "warming up …"
  300.times { channel.request("Ping", payload) }

  samples_n = 3000
  log "measuring request RTT (#{samples_n} calls) …"
  rtt_ms = Array.new(samples_n)
  loop_start = clock
  samples_n.times do |i|
    t0 = clock
    channel.request("Ping", payload)
    rtt_ms[i] = (clock - t0) * 1000.0
  end
  loop_elapsed = clock - loop_start
  rtt_ms.sort!
  req_hz = (samples_n / loop_elapsed).round
  req_mean = (rtt_ms.sum / samples_n).round(4)
  req_p50 = rtt_ms[samples_n / 2].round(4)
  req_p99 = rtt_ms[(samples_n * 0.99).to_i].round(4)

  # === 2. GVL 並行性 =========================================================
  # 各ラウンドで idle / unison / cpu を計測し、idle を 100% としたラウンド内
  # 比率を出す。spin-counter はスケジューラ依存でノイズが乗るため複数ラウンド
  # の中央値を採る。
  gvl_reqs = 2000
  cpu_iters = 30_000_000
  rounds = 5

  unison_pcts = []
  cpu_pcts = []
  rounds.times do |r|
    log "measuring GVL: round #{r + 1}/#{rounds} …"

    # idle baseline — main は sleep（sleep も GVL を解放 → 背景はほぼ 100%）
    c, e = measure_background { sleep 0.6 }
    rate_idle = c / e

    # Unison request 中 — block_on の network 待ちで GVL が解放される
    c, e = measure_background { gvl_reqs.times { channel.request("Ping", payload) } }
    unison_pcts << (c / e) / rate_idle * 100

    # 純 Ruby CPU ループ中 — main が GVL を握る対照群（GVL タイムスライス分のみ漏れる）
    c, e = measure_background do
      x = 0
      cpu_iters.times { x += 1 }
    end
    cpu_pcts << (c / e) / rate_idle * 100
  end

  pct_unison = median(unison_pcts).round(1)
  pct_cpu = median(cpu_pcts).round(1)

  # === KDL 出力 ==============================================================
  arch = RUBY_PLATFORM.split("-").first
  puts <<~KDL
    // Unison Ruby client bench run — immutable snapshot。1 file = 1 run。
    // 構造: benchmark > suite > case。
    // - channel: Channel#request の hz(req/sec) / mean_ms / p50_ms / p99_ms。
    // - gvl: 背景スレッドの spin-count throughput を idle=100% として正規化した
    //   pct（5 ラウンド中央値）。unison > cpu の差が network 待ち中の GVL 解放分。
    //   cpu が 0 でないのは CRuby の GVL タイムスライスによる漏れ。
    benchmark version="#{VERSION}" date="#{DATE}" client="ruby-client" {
        machine arch="#{arch}" ruby="#{RUBY_VERSION}" server="unison-mock"

        suite "channel" {
            case "Channel.request" hz=#{req_hz} mean_ms=#{req_mean} p50_ms=#{req_p50} p99_ms=#{req_p99} samples=#{samples_n}
        }
        suite "gvl" {
            case "bg-throughput/idle"   pct=100.0
            case "bg-throughput/unison" pct=#{pct_unison}
            case "bg-throughput/cpu"    pct=#{pct_cpu}
        }
    }
  KDL
ensure
  begin
    Process.kill("TERM", server)
    Process.wait(server)
  rescue Errno::ESRCH, Errno::ECHILD
    # mock は既に終了済み
  end
  mock_log&.close!
end
