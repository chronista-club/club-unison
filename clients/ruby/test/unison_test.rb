# frozen_string_literal: true

require "minitest/autorun"
require "unison"

# Connection-lifecycle 段階のテスト。実サーバを必要としない範囲のみ:
# サーバ相手の connect 成功パスは E2E テスト（次フェーズ）で扱う。
class UnisonTest < Minitest::Test
  def test_protocol_target_matches_the_built_generation
    assert_equal "1.0.0-rc.1", Unison.protocol_target
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
end
