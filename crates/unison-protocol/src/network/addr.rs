//! アドレス文字列の解釈 — 接続先の解決と SNI 名の導出。
//!
//! `[::1]:8080` / `::1` / `1.2.3.4:8080` / `8080` / `localhost` /
//! `https://host:port` といった人が書く形を [`SocketAddr`] に落とし、
//! TLS の SNI に載せる名前を決める。 QUIC そのものとは独立した文字列の責務なので
//! [`super::quic`] から分けてある。
//!
//! SNI は「hostname で繋いだなら hostname、 IP リテラルで繋いだならその IP」。
//! 以前 `"localhost"` を固定で載せていて、 mesh の SAN 名前検証が通らなかった
//! (= M3 の修正、 回帰テストは `test_medium_mesh_trust.rs`)。

use anyhow::{Context, Result};
use std::net::SocketAddr;

/// Default port for QUIC connections
const DEFAULT_PORT: u16 = 8080;

/// アドレス文字列を SocketAddr に解決する共通関数。
///
/// IPv6 / IPv4 リテラル + DNS hostname を受け付け、必要に応じて DNS 解決する。
///
/// 対応形式:
/// - `[::1]:8080` — IPv6 リテラル + port
/// - `::1` — IPv6 のみ (DEFAULT_PORT 付与)
/// - `1.2.3.4:8080` — IPv4 リテラル + port
/// - `8080` — port のみ (IPv6 ループバック fallback)
/// - `localhost:8080` / `localhost` — DNS 解決
/// - `host.example.com:8080` / `host.example.com` — DNS 解決
/// - `https://host:port` / `http://host:port` / `quic://host:port` — scheme prefix を strip
///
/// DNS 解決時は最初の resolved address を返す (IPv4/IPv6 どちらでも、リゾルバの順)。
pub(crate) async fn resolve_socket_addr(addr: &str) -> Result<SocketAddr> {
    // URL scheme 剥がし
    let addr = strip_scheme(addr);

    // 1. IPv4/IPv6 リテラル + port を直接 parse
    if let Ok(socket_addr) = addr.parse::<SocketAddr>() {
        return Ok(socket_addr);
    }

    // 2. port のみ ("8080") → IPv6 ループバック (後方互換)
    if let Ok(port) = addr.parse::<u16>() {
        return Ok(SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 1], port)));
    }

    // 3. IPv6 リテラル、port なし ("::1")
    if addr.contains(':') && !addr.contains('[') && !addr.contains('.') {
        let with_port = format!("[{}]:{}", addr, DEFAULT_PORT);
        if let Ok(sa) = with_port.parse::<SocketAddr>() {
            return Ok(sa);
        }
    }

    // 4. [IPv6]:port (bracket notation で port パース失敗ケース対応)
    if addr.starts_with('[')
        && let Some(end) = addr.find(']')
    {
        let ipv6_str = &addr[1..end];
        let ipv6 = ipv6_str
            .parse::<std::net::Ipv6Addr>()
            .map_err(|_| anyhow::anyhow!("無効なIPv6アドレス: {}", ipv6_str))?;
        let port = if addr.len() > end + 1 && &addr[end + 1..end + 2] == ":" {
            let port_str = &addr[end + 2..];
            if port_str.is_empty() {
                DEFAULT_PORT
            } else {
                port_str
                    .parse::<u16>()
                    .map_err(|_| anyhow::anyhow!("無効なポート番号: {}", port_str))?
            }
        } else {
            DEFAULT_PORT
        };
        return Ok(SocketAddr::from((ipv6, port)));
    }

    // 5. DNS hostname (host or host:port)
    let lookup_target = if has_port(addr) {
        addr.to_string()
    } else {
        format!("{}:{}", addr, DEFAULT_PORT)
    };
    let mut iter = tokio::net::lookup_host(&lookup_target)
        .await
        .with_context(|| format!("DNS lookup 失敗: {}", lookup_target))?;
    iter.next()
        .with_context(|| format!("アドレスを解決できませんでした: {}", lookup_target))
}

/// `https://` / `http://` / `quic://` 前置詞を取り除く
fn strip_scheme(addr: &str) -> &str {
    addr.strip_prefix("https://")
        .or_else(|| addr.strip_prefix("http://"))
        .or_else(|| addr.strip_prefix("quic://"))
        .unwrap_or(addr)
}

/// アドレスが `host:port` 形式 (末尾に port が付いている) か判定。
/// IPv6 リテラルは bracket notation 限定で判定する (生 `::1` は port 無し扱い)。
fn has_port(addr: &str) -> bool {
    if addr.starts_with('[') {
        return addr.contains("]:");
    }
    // 単純な hostname or IPv4 — 末尾の `:NNN` を port として認識
    if let Some(colon) = addr.rfind(':') {
        // host:port の host 側に ':' が無い (= IPv6 ではない) ことを担保
        if !addr[..colon].contains(':') {
            return addr[colon + 1..].parse::<u16>().is_ok();
        }
    }
    false
}

/// TLS SNI / 証明書検証に渡す server name を URL から抽出する。
///
/// `Endpoint::connect` の server_name は rustls の証明書 **名前検証** に使われる。
/// 以前はリテラル `"localhost"` 固定で、(a) mesh CA が実ホスト名で発行した
/// 証明書への接続が常に名前不一致で失敗し、(b) `localhost` SAN を持つ証明書なら
/// 任意のホストになりすませる name-pinning 不全を生んでいた。
///
/// 解決方針:
/// - hostname 入力 (`example.com:443`) → hostname を SNI に (= DNS SAN 照合)
/// - IP リテラル入力 (`[::1]:8080` / `127.0.0.1:8080` / `::1`) → IP 文字列を SNI に
///   (rustls は `ServerName::IpAddress` として IP SAN を照合)
/// - port のみ (`8080`) → 解決後の loopback IP を SNI に
pub(crate) fn sni_server_name(addr: &str, resolved: SocketAddr) -> String {
    let addr = strip_scheme(addr);

    // IPv6 bracket リテラル ("[::1]:8080" / "[::1]") → 括弧内の IP
    if let Some(rest) = addr.strip_prefix('[')
        && let Some(end) = rest.find(']')
    {
        return rest[..end].to_string();
    }

    // port のみ ("8080") → 解決済み loopback IP
    if addr.parse::<u16>().is_ok() {
        return resolved.ip().to_string();
    }

    // bracket なし IPv6 リテラル ("::1", "fd7a:115c::1")
    if addr.contains(':') && !addr.contains('.') && addr.parse::<std::net::Ipv6Addr>().is_ok() {
        return addr.to_string();
    }

    // host:port → host 部分、port なし → そのまま (hostname or IPv4 リテラル)
    if has_port(addr)
        && let Some(colon) = addr.rfind(':')
    {
        return addr[..colon].to_string();
    }
    addr.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn resolve_ipv6_literal_with_port() {
        let sa = resolve_socket_addr("[::1]:8080").await.unwrap();
        assert!(matches!(sa, SocketAddr::V6(_)));
        assert_eq!(sa.port(), 8080);
    }

    #[tokio::test]
    async fn resolve_ipv6_literal_without_port_uses_default() {
        let sa = resolve_socket_addr("::1").await.unwrap();
        assert!(matches!(sa, SocketAddr::V6(_)));
        assert_eq!(sa.port(), DEFAULT_PORT);
    }

    #[tokio::test]
    async fn resolve_ipv4_literal_with_port_is_now_supported() {
        let sa = resolve_socket_addr("127.0.0.1:8080").await.unwrap();
        assert!(matches!(sa, SocketAddr::V4(_)));
        assert_eq!(sa.port(), 8080);
    }

    #[tokio::test]
    async fn resolve_port_only_falls_back_to_ipv6_loopback() {
        let sa = resolve_socket_addr("8080").await.unwrap();
        assert!(matches!(sa, SocketAddr::V6(_)));
        assert_eq!(sa.port(), 8080);
    }

    #[tokio::test]
    async fn resolve_localhost_with_port_via_dns() {
        let sa = resolve_socket_addr("localhost:8080").await.unwrap();
        // tokio::net::lookup_host が IPv4 / IPv6 のどちらを返すかは環境依存だが
        // port は確実に 8080
        assert_eq!(sa.port(), 8080);
    }

    #[tokio::test]
    async fn resolve_strips_https_scheme() {
        let sa = resolve_socket_addr("https://[::1]:4510").await.unwrap();
        assert!(matches!(sa, SocketAddr::V6(_)));
        assert_eq!(sa.port(), 4510);
    }

    #[tokio::test]
    async fn resolve_strips_http_scheme() {
        let sa = resolve_socket_addr("http://127.0.0.1:8080").await.unwrap();
        assert!(matches!(sa, SocketAddr::V4(_)));
    }

    #[tokio::test]
    async fn resolve_strips_quic_scheme() {
        let sa = resolve_socket_addr("quic://[::1]:9999").await.unwrap();
        assert_eq!(sa.port(), 9999);
    }

    #[tokio::test]
    async fn resolve_unresolvable_hostname_errors() {
        let res = resolve_socket_addr("definitely-not-a-real-host-12345.invalid:8080").await;
        assert!(res.is_err(), "unresolvable hostname should error");
    }

    #[test]
    fn has_port_recognizes_ipv4_with_port() {
        assert!(has_port("127.0.0.1:8080"));
        assert!(has_port("example.com:443"));
    }

    #[test]
    fn has_port_recognizes_ipv6_bracket_with_port() {
        assert!(has_port("[::1]:8080"));
        assert!(has_port("[fd7a:115c:a1e0::f936:d97b]:4510"));
    }

    #[test]
    fn has_port_rejects_bare_ipv6_without_brackets() {
        // "::1" は port 無しと扱う (IPv6 リテラルは bracket 必須)
        assert!(!has_port("::1"));
        assert!(!has_port("fd7a:115c:a1e0::f936:d97b"));
    }

    #[test]
    fn has_port_rejects_hostname_without_port() {
        assert!(!has_port("example.com"));
        assert!(!has_port("localhost"));
    }

    #[test]
    fn strip_scheme_removes_known_prefixes() {
        assert_eq!(strip_scheme("https://example.com:443"), "example.com:443");
        assert_eq!(strip_scheme("http://example.com:80"), "example.com:80");
        assert_eq!(strip_scheme("quic://example.com:4510"), "example.com:4510");
    }

    #[test]
    fn strip_scheme_keeps_address_when_no_prefix() {
        assert_eq!(strip_scheme("example.com:443"), "example.com:443");
        assert_eq!(strip_scheme("[::1]:8080"), "[::1]:8080");
    }

    // ─────────────────────────────────────────
    // sni_server_name — SNI / 証明書名前検証に渡す名前の導出
    // ─────────────────────────────────────────

    fn sa(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    #[test]
    fn sni_hostname_uses_hostname_not_resolved_ip() {
        // hostname 入力は (DNS 解決後の IP ではなく) hostname を SNI にする
        assert_eq!(
            sni_server_name("example.com:443", sa("93.184.216.34:443")),
            "example.com"
        );
        assert_eq!(
            sni_server_name("localhost:8080", sa("127.0.0.1:8080")),
            "localhost"
        );
    }

    #[test]
    fn sni_ipv6_bracket_literal_uses_ip() {
        assert_eq!(sni_server_name("[::1]:8080", sa("[::1]:8080")), "::1");
        assert_eq!(
            sni_server_name("[fd7a:115c::1]:4510", sa("[fd7a:115c::1]:4510")),
            "fd7a:115c::1"
        );
    }

    #[test]
    fn sni_bare_ipv6_literal_uses_ip() {
        assert_eq!(sni_server_name("::1", sa("[::1]:8080")), "::1");
    }

    #[test]
    fn sni_ipv4_literal_uses_ip() {
        assert_eq!(
            sni_server_name("127.0.0.1:8080", sa("127.0.0.1:8080")),
            "127.0.0.1"
        );
    }

    #[test]
    fn sni_port_only_uses_resolved_loopback_ip() {
        assert_eq!(sni_server_name("8080", sa("[::1]:8080")), "::1");
    }

    #[test]
    fn sni_strips_scheme_first() {
        assert_eq!(
            sni_server_name("quic://example.com:4510", sa("10.0.0.1:4510")),
            "example.com"
        );
        assert_eq!(
            sni_server_name("https://[::1]:4510", sa("[::1]:4510")),
            "::1"
        );
    }

    #[test]
    fn sni_is_never_hardcoded_localhost() {
        // regression guard: 旧実装の "localhost" 固定に戻らないこと
        assert_ne!(
            sni_server_name("cp.fleetstage.cloud:4510", sa("10.1.2.3:4510")),
            "localhost"
        );
    }
}
