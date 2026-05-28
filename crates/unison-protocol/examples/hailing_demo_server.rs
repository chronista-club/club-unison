//! Hailing α MCP demo server — runs a Unison server with `enable_discovery`,
//! registers a few sample channels, and waits for `unison-mcp` clients to
//! connect via the discovery channel and call synthesized typed tools.
//!
//! # Usage
//!
//! ```bash
//! cargo run -p club-unison --example hailing_demo_server
//! ```
//!
//! Server listens on `[::1]:7878` (= matches the default `unison.json` shipped
//! with `unison-mcp`). Press Ctrl-C to stop.
//!
//! From another terminal (or via Claude Code's `.mcp.json`):
//!
//! ```bash
//! cargo build -p unison-mcp --release
//! # Then add to .mcp.json:
//! # "unison": {
//! #   "command": "target/release/unison-mcp",
//! #   "args": ["--config", "crates/unison-mcp/examples/unison.json"]
//! # }
//! ```
//!
//! After that an AI agent in Claude Code sees these tools:
//! - `unison_ping` / `unison_call` / `unison_discover` (static escape hatches)
//! - `unison_greet_Hello` (synthesized, with `description="..."` from KDL)
//! - `unison_math_Add`
//! - `unison_echo_Echo`

use anyhow::Result;
use serde_json::json;
use tracing::{Level, info};

use unison::network::channel::UnisonChannel;
use unison::network::{MessageType, ProtocolServer};

/// Demo protocol KDL — small enough to read, rich enough to show all 3 basic
/// shapes (string, int, json) and the F11 description property in action.
const DEMO_KDL: &str = r#"
protocol "hailing-demo" version="0.1.0" {
    namespace "demo.hailing"

    // unison.discovery 自体を含めて self-describing にする (= MCP bridge が
    // synthesized `unison_unison_discovery_GetProtocol` を作れる、 ただし escape
    // hatch `unison_discover` のほうが使いやすい)
    channel "unison.discovery" from="client" lifetime="persistent" {
        request "GetProtocol" {
            field "format" type="string" required=#true
            returns "ProtocolDocument" {
                field "kdl" type="string" required=#true
                field "version" type="string" required=#true
                field "hash" type="string" required=#true
                field "codecs" type="json" required=#true
            }
        }
    }

    // String-typed request, with F11 description for LLM tool-selection
    channel "greet" from="client" lifetime="persistent" {
        request "Hello" description="Greet someone by name. Returns a polite greeting message." {
            field "name" type="string" required=#true
            returns "Greeting" {
                field "message" type="string" required=#true
            }
        }
    }

    // Int-typed request — exercises int validation + arithmetic
    channel "math" from="client" lifetime="persistent" {
        request "Add" description="Add two integers and return the sum." {
            field "a" type="int" required=#true
            field "b" type="int" required=#true
            returns "Sum" {
                field "result" type="int" required=#true
            }
        }
    }

    // Json-typed request — exercises the escape-hatch type
    channel "echo" from="client" lifetime="persistent" {
        request "Echo" description="Echo back any JSON payload unchanged. Useful for testing." {
            field "payload" type="json" required=#true
            returns "EchoResult" {
                field "echoed" type="json" required=#true
            }
        }
    }
}
"#;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .with_target(false)
        .init();

    let server = ProtocolServer::with_identity("hailing-demo", "0.1.0", "demo.hailing");

    // P1 deliverable: enable_discovery で server 自身の KDL を配信
    server.enable_discovery(DEMO_KDL).await?;
    info!("✓ unison.discovery channel enabled (= P1)");

    // greet.Hello handler
    server
        .register_channel("greet", |_ctx, stream| async move {
            let channel = UnisonChannel::new(stream);
            loop {
                match channel.recv().await {
                    Ok(msg) if msg.msg_type == MessageType::Request && msg.method == "Hello" => {
                        let payload = msg.payload_as_value().unwrap_or_default();
                        let name = payload
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("anon");
                        let reply = json!({ "message": format!("Hello, {name}! 👋") });
                        if channel
                            .send_response(msg.id, &msg.method, &reply)
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Ok(_) => continue,
                    Err(e) if e.is_normal_close() => return Ok(()),
                    Err(e) => return Err(e),
                }
            }
            Ok(())
        })
        .await;
    info!("✓ greet channel registered (Hello)");

    // math.Add handler
    server
        .register_channel("math", |_ctx, stream| async move {
            let channel = UnisonChannel::new(stream);
            loop {
                match channel.recv().await {
                    Ok(msg) if msg.msg_type == MessageType::Request && msg.method == "Add" => {
                        let payload = msg.payload_as_value().unwrap_or_default();
                        let a = payload.get("a").and_then(|v| v.as_i64()).unwrap_or(0);
                        let b = payload.get("b").and_then(|v| v.as_i64()).unwrap_or(0);
                        let reply = json!({ "result": a + b });
                        if channel
                            .send_response(msg.id, &msg.method, &reply)
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Ok(_) => continue,
                    Err(e) if e.is_normal_close() => return Ok(()),
                    Err(e) => return Err(e),
                }
            }
            Ok(())
        })
        .await;
    info!("✓ math channel registered (Add)");

    // echo.Echo handler
    server
        .register_channel("echo", |_ctx, stream| async move {
            let channel = UnisonChannel::new(stream);
            loop {
                match channel.recv().await {
                    Ok(msg) if msg.msg_type == MessageType::Request && msg.method == "Echo" => {
                        let payload = msg.payload_as_value().unwrap_or_default();
                        let inner = payload.get("payload").cloned().unwrap_or(json!(null));
                        let reply = json!({ "echoed": inner });
                        if channel
                            .send_response(msg.id, &msg.method, &reply)
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Ok(_) => continue,
                    Err(e) if e.is_normal_close() => return Ok(()),
                    Err(e) => return Err(e),
                }
            }
            Ok(())
        })
        .await;
    info!("✓ echo channel registered (Echo)");

    let addr = "[::1]:7878";
    info!("");
    info!("================================================================");
    info!("  Hailing α demo server — protocol \"hailing-demo\" v0.1.0");
    info!("  Listening on {addr}");
    info!("");
    info!("  Channels: unison.discovery + greet + math + echo");
    info!("  Synthesized MCP tools the agent will see:");
    info!("    - unison_unison_discovery_GetProtocol  (self-describing meta)");
    info!("    - unison_greet_Hello                   (Greet by name)");
    info!("    - unison_math_Add                      (Integer addition)");
    info!("    - unison_echo_Echo                     (JSON echo)");
    info!("  Plus 3 static escape hatches: unison_ping / unison_call / unison_discover");
    info!("");
    info!("  Try from another terminal:");
    info!("    cargo run -p unison-mcp -- --config crates/unison-mcp/examples/unison.json");
    info!("    (Then send MCP tools/list / tools/call requests via stdio)");
    info!("");
    info!("  Or via Claude Code .mcp.json — see crates/unison-mcp/DEMO.md");
    info!("");
    info!("  Press Ctrl-C to stop");
    info!("================================================================");

    server.listen(addr).await?;
    Ok(())
}
