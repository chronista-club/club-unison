# frozen_string_literal: true

require "minitest/autorun"
require "unison"

# Connection-lifecycle 段階のテスト。実サーバを必要としない範囲のみ:
# サーバ相手の connect 成功パスは E2E テスト（次フェーズ）で扱う。
class UnisonTest < Minitest::Test
  def test_protocol_target_matches_the_built_generation
    assert_equal "1.0.0", Unison.protocol_target
  end

  def test_client_new_returns_a_client
    assert_instance_of Unison::Client, Unison::Client.new
  end

  def test_a_fresh_client_is_not_connected
    refute Unison::Client.new.connected?
  end

  def test_client_responds_to_open_channel
    assert_respond_to Unison::Client.new, :open_channel
  end

  def test_channel_class_is_defined
    assert_kind_of Class, Unison::Channel
  end

  def test_error_is_a_standard_error_subclass
    assert_operator Unison::Error, :<, StandardError
  end

  # tokio runtime のワーカーをコア数ぶん立てない。Ruby はプロセスを複数並べて
  # 動かすことが多く (Puma 等)、プロセス数 × コア数のワーカーが CPU を奪い合う。
  # runtime は process-wide の OnceLock で、同じプロセスの他のテストが先に
  # 初期化し得るので、別プロセスで「runtime 初期化で増えた OS スレッド数」を測る。
  def test_runtime_does_not_spawn_a_worker_per_core
    script = <<~RUBY
      require "unison"
      def os_threads
        if File.directory?("/proc/self/task")
          Dir.children("/proc/self/task").size
        else
          `ps -M \#{Process.pid}`.lines.size - 1
        end
      end
      before = os_threads
      Unison::Client.new.connected? # runtime を初期化する最初の呼び出し
      print os_threads - before
    RUBY
    lib = File.expand_path("../lib", __dir__)
    out = IO.popen([RbConfig.ruby, "-I", lib, "-e", script], &:read)

    assert_predicate $?, :success?
    assert_operator Integer(out), :<=, 2
  end
end
