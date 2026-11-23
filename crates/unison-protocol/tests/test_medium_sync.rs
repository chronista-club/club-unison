//! Medium テスト: 本物の QUIC で `Origin` に入り、手元の `Slave`（`ConnectedSlave`）が自動同期すること、
//! および QUIC で下流を待ち受ける多段中継（`Origin ──QUIC──▶ Relay ──QUIC──▶ Leaf`）を検証する。
//!
//! `#[ignore = "Medium: 実 QUIC runtime が要る"]` 付き — `cargo test -- --ignored` で実行。
//! Memory: mem_1ChKdJTLL17QHAFpktCt6S

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};

use unison::network::{MessageType, UnisonChannel};
use unison::proto::{MessageType as ProtoMsgType, ProtocolMessage, ProtocolMessageView};
use unison::sync::{
    CONTROL_CHANNEL, ConnectedSlave, FEED_CHANNEL, JOIN, JoinError, JoinReply, JoinRequest,
    NAMESPACE, Origin, PROTOCOL_NAME, Relay, SET, SetReply, SetRequest, Slave, SlotState,
    SlotUpdate, Snapshot, SyncError, SyncServer,
};

const PATIENCE: Duration = Duration::from_secs(5);

fn state(rev: u64, value: Value) -> SlotState {
    SlotState::from_json(rev, &value)
}

fn update(life: u64, slot: &str, rev: u64, value: Value) -> SlotUpdate {
    SlotUpdate::from_json(life, slot, rev, &value)
}

async fn within<T>(what: &str, work: impl Future<Output = T>) -> T {
    tokio::time::timeout(PATIENCE, work)
        .await
        .unwrap_or_else(|_| panic!("時間切れ: {what}"))
}

async fn start_origin(life: u64) -> (Arc<Origin>, SyncServer) {
    let origin = Arc::new(Origin::new(life));
    let server = SyncServer::start(origin.clone(), "[::1]:0")
        .await
        .expect("サーバが立ち上がらない");
    (origin, server)
}

async fn join(server: &SyncServer) -> ConnectedSlave {
    within("入る", ConnectedSlave::join(server.local_addr()))
        .await
        .expect("入れない")
}

async fn until(slave: &mut Slave, what: &str, done: impl Fn(&Slave) -> bool) {
    within(what, async {
        while !done(slave) {
            if slave.changed().await.is_none() {
                panic!("外れた: {what}");
            }
        }
    })
    .await;
}

async fn until_dropped(slave: &mut Slave, what: &str) {
    within(what, async { while slave.changed().await.is_some() {} }).await;
    assert!(slave.is_closed(), "{what}");
}

async fn start_relay(upstream: &SyncServer) -> (Arc<Relay>, SyncServer) {
    let slave = join(upstream).await;
    let relay = Arc::new(Relay::new(slave));
    let server = SyncServer::start(relay.clone(), "[::1]:0")
        .await
        .expect("中継が待ち受けられない");
    (relay, server)
}

async fn refused_reason(server: &SyncServer) -> String {
    match within("入る", ConnectedSlave::join(server.local_addr())).await {
        Err(JoinError::Refused(reason)) => reason,
        Err(other) => panic!("断られずに失敗した: {other}"),
        Ok(_) => panic!("入れてしまった"),
    }
}

#[tokio::test]
#[ignore = "Medium: 実 QUIC runtime が要る"]
async fn join_gets_snapshot() {
    let (origin, server) = start_origin(7).await;
    origin.set("light", json!(0.8)).unwrap();

    let slave = join(&server).await;
    assert_eq!(slave.life(), 7);
    assert_eq!(slave.get("light"), Some(state(1, json!(0.8))));
    assert_eq!(slave.get("camera"), None);

    server.shutdown().await;
}

#[tokio::test]
#[ignore = "Medium: 実 QUIC runtime が要る"]
async fn origin_set_reaches_slave() {
    let (origin, server) = start_origin(1).await;
    let mut slave = join(&server).await;

    origin.set("camera", json!("wide")).unwrap();
    assert_eq!(
        within("変動", slave.changed()).await,
        Some(vec![update(1, "camera", 1, json!("wide"))])
    );
    assert_eq!(slave.get("camera"), Some(state(1, json!("wide"))));

    server.shutdown().await;
}

#[tokio::test]
#[ignore = "Medium: 実 QUIC runtime が要る"]
async fn slave_set_is_confirmed_by_origin_and_reaches_others() {
    let (origin, server) = start_origin(1).await;
    let writer = join(&server).await;
    let mut other = join(&server).await;

    let confirmed = within("頼む", writer.set("light", json!(2))).await.unwrap();
    assert_eq!((confirmed.rev, &confirmed.value()), (1, &json!(2)));
    assert_eq!(writer.get("light"), Some(state(1, json!(2))));
    assert_eq!(origin.get("light"), Some(state(1, json!(2))));
    until(&mut other, "ほかの Slave に届く", |slave| {
        slave.get("light") == Some(state(1, json!(2)))
    })
    .await;

    server.shutdown().await;
}

#[tokio::test]
#[ignore = "Medium: 実 QUIC runtime が要る"]
async fn slots_merge_from_many_writers() {
    let (origin, server) = start_origin(1).await;
    let mut a = join(&server).await;
    let mut b = join(&server).await;
    let mut watcher = join(&server).await;

    let by_owner = async {
        for i in 0..30 {
            origin.set("light", json!(i)).unwrap();
            tokio::task::yield_now().await;
        }
    };
    let by_a = async {
        for i in 0..30 {
            a.set("camera", json!(i)).await.unwrap();
        }
    };
    let by_b = async {
        for i in 0..30 {
            b.set("light", json!(-i)).await.unwrap();
            b.set("sound", json!(i)).await.unwrap();
        }
    };
    within("置き換える", async {
        tokio::join!(by_owner, by_a, by_b)
    })
    .await;

    let expected = origin.snapshot().slots;
    assert_eq!(expected["light"].rev, 60);
    assert_eq!(expected["camera"].rev, 30);
    assert_eq!(expected["sound"].rev, 30);
    for (who, slave) in [("a", &mut a), ("b", &mut b), ("watcher", &mut watcher)] {
        until(slave, who, |slave| slave.snapshot().slots == expected).await;
    }

    server.shutdown().await;
}

#[tokio::test]
#[ignore = "Medium: 実 QUIC runtime が要る"]
async fn slow_consumer_slave_is_always_current() {
    let (origin, server) = start_origin(1).await;
    let mut slave = join(&server).await;

    for i in 1..=200 {
        origin.set("light", json!(i)).unwrap();
    }
    within("Slave が追いつく", async {
        while slave.get("light") != Some(state(200, json!(200))) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    assert_eq!(
        within("変動", slave.changed()).await,
        Some(vec![update(1, "light", 200, json!(200))])
    );

    server.shutdown().await;
}

#[tokio::test]
#[ignore = "Medium: 実 QUIC runtime が要る"]
async fn relay_over_a_quic_slave() {
    let (origin, server) = start_origin(1).await;
    let relay = join(&server).await;
    let mut downstream = Slave::follow(&*relay);

    let update = origin.set("light", json!(1)).unwrap();
    until(&mut downstream, "下流に届く", |slave| {
        slave.get("light") == Some(state(update.rev, json!(1)))
    })
    .await;

    let confirmed = within("下流から頼む", downstream.set("camera", json!("wide")))
        .await
        .unwrap();
    assert_eq!(
        origin.get("camera"),
        Some(state(confirmed.rev, json!("wide")))
    );
    assert_eq!(
        relay.get("camera"),
        Some(state(confirmed.rev, json!("wide")))
    );
    assert_eq!(
        downstream.get("camera"),
        Some(state(confirmed.rev, json!("wide")))
    );

    relay.leave().await;
    until_dropped(&mut downstream, "下流も外れる").await;

    server.shutdown().await;
}

#[tokio::test]
#[ignore = "Medium: 実 QUIC runtime が要る"]
async fn stop_drops_everyone() {
    let (origin, server) = start_origin(1).await;
    let mut slave = join(&server).await;

    origin.close();
    until_dropped(&mut slave, "止まったことが伝わる").await;
    assert_eq!(slave.set("light", json!(1)).await, Err(SyncError::Closed));
    assert_eq!(refused_reason(&server).await, "stopped");

    server.shutdown().await;
}

#[tokio::test]
#[ignore = "Medium: 実 QUIC runtime が要る"]
async fn bad_set_is_refused() {
    let (_origin, server) = start_origin(1).await;
    let slave = join(&server).await;
    assert_eq!(
        slave.set("", json!(1)).await,
        Err(SyncError::Refused("bad_request".into()))
    );
    server.shutdown().await;
}

#[tokio::test]
#[ignore = "Medium: 実 QUIC runtime が要る"]
async fn set_without_join_and_double_join_are_refused() {
    let (_origin, server) = start_origin(1).await;

    let client = unison::ProtocolClient::insecure_localhost().unwrap();
    within("つなぐ", client.connect(&server.local_addr().to_string()))
        .await
        .unwrap();
    let channel = within("開く", client.open_channel(CONTROL_CHANNEL))
        .await
        .unwrap();
    let reply: SetReply = within(
        "頼む",
        channel.request(SET, &SetRequest::from_json("light", &json!(1))),
    )
    .await
    .unwrap();
    assert!(
        matches!(&reply, SetReply::Refused { error } if error == "not_joined"),
        "{reply:?}"
    );

    let first: JoinReply = within("入る", channel.request(JOIN, &JoinRequest::default()))
        .await
        .unwrap();
    assert!(matches!(first, JoinReply::Joined(_)), "{first:?}");
    let second: JoinReply = within(
        "もう一度入る",
        channel.request(JOIN, &JoinRequest::default()),
    )
    .await
    .unwrap();
    assert!(
        matches!(&second, JoinReply::Refused { error } if error == "already_joined"),
        "{second:?}"
    );

    let _ = client.disconnect().await;
    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "Medium: 実 QUIC runtime が要る"]
async fn many_concurrent_sets_are_all_confirmed() {
    const SETS: u64 = 3000;
    let (origin, server) = start_origin(1).await;
    let slave = Arc::new(join(&server).await);

    let mut sets = tokio::task::JoinSet::new();
    for i in 0..SETS {
        let slave = slave.clone();
        sets.spawn(async move { slave.set("light", json!(i)).await });
    }
    let confirmed = tokio::time::timeout(Duration::from_secs(20), sets.join_all())
        .await
        .expect("時間切れ: 頼みが捨てられた");
    assert!(
        confirmed.iter().all(Result::is_ok),
        "確定しなかった頼みがある"
    );
    assert_eq!(origin.get("light").unwrap().rev, SETS);

    server.shutdown().await;
}

#[tokio::test]
#[ignore = "Medium: 実 QUIC runtime が要る"]
async fn client_without_feed_is_dropped() {
    let (origin, origin_server) = start_origin(1).await;
    let (relay, relay_server) = start_relay(&origin_server).await;

    // feed の受け口を置かずに relay へ入る
    let client = unison::ProtocolClient::insecure_localhost().unwrap();
    within(
        "つなぐ",
        client.connect(&relay_server.local_addr().to_string()),
    )
    .await
    .unwrap();
    let channel = within("開く", client.open_channel(CONTROL_CHANNEL))
        .await
        .unwrap();
    let reply: JoinReply = within("入る", channel.request(JOIN, &JoinRequest::default()))
        .await
        .unwrap();
    assert!(matches!(reply, JoinReply::Joined(_)), "{reply:?}");
    assert_eq!(relay.visitors(), 1);

    // 変動を流し続けると、feed に書けなくなった時点で外される
    within("外される", async {
        let mut i = 0;
        while relay.visitors() != 0 {
            origin.set("light", json!(i)).unwrap();
            i += 1;
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;

    let after: Result<SetReply, _> = within(
        "頼む",
        channel.request(SET, &SetRequest::from_json("light", &json!(-1))),
    )
    .await;
    assert!(after.is_err(), "{after:?}");

    let _ = client.disconnect().await;
    relay_server.shutdown().await;
    origin_server.shutdown().await;
}

#[tokio::test]
#[ignore = "Medium: 実 QUIC runtime が要る"]
async fn failed_join_leaves_no_connection() {
    let server = Arc::new(unison::ProtocolServer::with_identity(
        "not-sync",
        "0.0.0",
        "example.not_sync",
    ));
    let handle = server
        .clone()
        .listener("[::1]:0")
        .spawn()
        .await
        .expect("サーバが立ち上がらない");

    let failed = within("入る", ConnectedSlave::join(handle.local_addr())).await;
    assert!(
        matches!(failed, Err(JoinError::Network(_))),
        "{:?}",
        failed.err()
    );
    within("つながりが閉じる", async {
        while server.active_connection_count().await != 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;

    let _ = handle.shutdown().await;
}

// ── 中継: QUIC で下流を待ち受ける ──

#[tokio::test]
#[ignore = "Medium: 実 QUIC runtime が要る"]
async fn relay_delivers_origin_updates_downstream() {
    let (origin, origin_server) = start_origin(7).await;
    origin.set("light", json!(0.8)).unwrap();
    let (relay, relay_server) = start_relay(&origin_server).await;

    let mut leaf = join(&relay_server).await;
    assert_eq!(leaf.life(), 7);
    assert_eq!(leaf.get("light"), Some(state(1, json!(0.8))));
    assert_eq!(relay.visitors(), 1);

    origin.set("light", json!(0.5)).unwrap();
    origin.set("camera", json!("wide")).unwrap();
    let expected = origin.snapshot().slots;
    until(&mut leaf, "中継を経て届く", |slave| {
        slave.snapshot().slots == expected
    })
    .await;
    assert_eq!(leaf.get("light"), Some(state(2, json!(0.5))));

    relay_server.shutdown().await;
    origin_server.shutdown().await;
}

#[tokio::test]
#[ignore = "Medium: 実 QUIC runtime が要る"]
async fn leaf_set_through_relay_reaches_everyone() {
    let (origin, origin_server) = start_origin(1).await;
    let (relay, relay_server) = start_relay(&origin_server).await;
    let writer = join(&relay_server).await;
    let mut other_leaf = join(&relay_server).await;
    let mut direct = join(&origin_server).await;

    let confirmed = within("中継を経て頼む", writer.set("light", json!(2)))
        .await
        .unwrap();
    assert_eq!((confirmed.rev, &confirmed.value()), (1, &json!(2)));
    assert_eq!(origin.get("light"), Some(state(1, json!(2))));
    assert_eq!(
        relay.snapshot().unwrap().slots.get("light"),
        Some(&state(1, json!(2)))
    );
    assert_eq!(writer.get("light"), Some(state(1, json!(2))));
    for (who, slave) in [
        ("別の Leaf", &mut other_leaf),
        ("直結の Slave", &mut direct),
    ] {
        until(slave, who, |slave| {
            slave.get("light") == Some(state(1, json!(2)))
        })
        .await;
    }

    assert_eq!(
        writer.set("", json!(1)).await,
        Err(SyncError::Refused("bad_request".into()))
    );

    relay_server.shutdown().await;
    origin_server.shutdown().await;
}

#[tokio::test]
#[ignore = "Medium: 実 QUIC runtime が要る"]
async fn concurrent_leaf_sets_converge() {
    let (origin, origin_server) = start_origin(1).await;
    let (relay, relay_server) = start_relay(&origin_server).await;
    let mut a = join(&relay_server).await;
    let mut b = join(&relay_server).await;
    let mut direct = join(&origin_server).await;

    let by_owner = async {
        for i in 0..30 {
            origin.set("light", json!(i)).unwrap();
            tokio::task::yield_now().await;
        }
    };
    let by_a = async {
        for i in 0..30 {
            a.set("light", json!(-i)).await.unwrap();
            a.set("camera", json!(i)).await.unwrap();
        }
    };
    let by_b = async {
        for i in 0..30 {
            b.set("light", json!(100 + i)).await.unwrap();
        }
    };
    let by_direct = async {
        for i in 0..30 {
            direct.set("sound", json!(i)).await.unwrap();
        }
    };
    within("置き換える", async {
        tokio::join!(by_owner, by_a, by_b, by_direct)
    })
    .await;

    let expected = origin.snapshot().slots;
    assert_eq!(expected["light"].rev, 90);
    assert_eq!(expected["camera"].rev, 30);
    assert_eq!(expected["sound"].rev, 30);
    for (who, slave) in [("a", &mut a), ("b", &mut b), ("直結", &mut direct)] {
        until(slave, who, |slave| slave.snapshot().slots == expected).await;
    }
    within("中継が揃う", async {
        while relay.snapshot().unwrap().slots != expected {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;

    relay_server.shutdown().await;
    origin_server.shutdown().await;
}

#[tokio::test]
#[ignore = "Medium: 実 QUIC runtime が要る"]
async fn leaf_leaving_keeps_others_running() {
    let (origin, origin_server) = start_origin(1).await;
    let (relay, relay_server) = start_relay(&origin_server).await;
    let leaving = join(&relay_server).await;
    let mut staying = join(&relay_server).await;
    assert_eq!(relay.visitors(), 2);

    leaving.leave().await;
    within("出ていったことが中継に伝わる", async {
        while relay.visitors() != 1 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;

    assert!(!relay.is_stopped());
    origin.set("light", json!(1)).unwrap();
    until(&mut staying, "残った Leaf に届く", |slave| {
        slave.get("light") == Some(state(1, json!(1)))
    })
    .await;
    assert!(staying.set("camera", json!(2)).await.is_ok());

    relay_server.shutdown().await;
    origin_server.shutdown().await;
}

#[tokio::test]
#[ignore = "Medium: 実 QUIC runtime が要る"]
async fn relay_stop_drops_only_its_leaves() {
    let (origin, origin_server) = start_origin(1).await;
    let (relay, relay_server) = start_relay(&origin_server).await;
    let mut leaf = join(&relay_server).await;
    let mut direct = join(&origin_server).await;

    relay.stop().await;
    until_dropped(&mut leaf, "Leaf が外れる").await;
    assert_eq!(leaf.set("light", json!(1)).await, Err(SyncError::Closed));
    // Origin と直結の Slave は動き続ける
    origin.set("light", json!(1)).unwrap();
    until(&mut direct, "直結の Slave に届く", |slave| {
        slave.get("light") == Some(state(1, json!(1)))
    })
    .await;
    // 止まった中継には入れない
    assert_eq!(refused_reason(&relay_server).await, "stopped");

    relay_server.shutdown().await;
    origin_server.shutdown().await;
}

#[tokio::test]
#[ignore = "Medium: 実 QUIC runtime が要る"]
async fn origin_stop_reaches_leaves_through_relay() {
    let (origin, origin_server) = start_origin(1).await;
    let (relay, relay_server) = start_relay(&origin_server).await;
    let mut leaf = join(&relay_server).await;
    let mut watching = relay.observe().unwrap();

    origin.close();
    within("中継が外れる", async {
        while watching.next().await.is_some() {}
    })
    .await;
    assert!(relay.is_stopped());
    until_dropped(&mut leaf, "Leaf が外れる").await;
    assert_eq!(leaf.set("light", json!(1)).await, Err(SyncError::Closed));
    assert_eq!(refused_reason(&relay_server).await, "stopped");

    relay.stop().await;
    relay_server.shutdown().await;
    origin_server.shutdown().await;
}

#[tokio::test]
#[ignore = "Medium: 実 QUIC runtime が要る"]
async fn relay_chain_over_quic() {
    let (origin, origin_server) = start_origin(1).await;
    let (first, first_server) = start_relay(&origin_server).await;
    let (second, second_server) = start_relay(&first_server).await;
    let mut leaf = join(&second_server).await;

    origin.set("light", json!(1)).unwrap();
    until(&mut leaf, "3 段めに届く", |slave| {
        slave.get("light") == Some(state(1, json!(1)))
    })
    .await;

    let confirmed = within("3 段めから頼む", leaf.set("camera", json!("wide")))
        .await
        .unwrap();
    assert_eq!(confirmed.rev, 1);
    assert_eq!(origin.get("camera"), Some(state(1, json!("wide"))));
    for (who, relay) in [("Relay 1", &first), ("Relay 2", &second)] {
        assert_eq!(
            relay.snapshot().unwrap().slots.get("camera"),
            Some(&state(1, json!("wide"))),
            "{who}"
        );
    }
    assert_eq!(leaf.get("camera"), Some(state(1, json!("wide"))));

    second.stop().await;
    until_dropped(&mut leaf, "Leaf が外れる").await;
    assert!(!first.is_stopped());
    assert!(first.set("light", json!(2)).await.is_ok());

    second_server.shutdown().await;
    first.stop().await;
    first_server.shutdown().await;
    origin_server.shutdown().await;
}

async fn start_stalling_upstream() -> (
    unison::network::server::ServerHandle,
    Arc<tokio::sync::Notify>,
) {
    let heard_slow = Arc::new(tokio::sync::Notify::new());
    let server = unison::ProtocolServer::with_identity(PROTOCOL_NAME, "0.0.0", NAMESPACE);
    let heard = heard_slow.clone();
    server
        .register_channel(CONTROL_CHANNEL, move |ctx, stream| {
            let heard = heard.clone();
            async move {
                let channel = UnisonChannel::<unison::codec::ProtoCodec>::new(stream);
                let mut rev = 0;
                let mut _feed = None;
                loop {
                    let Ok(message) = channel.recv().await else {
                        return Ok(());
                    };
                    if message.msg_type != MessageType::Request {
                        continue;
                    }
                    match message.method.as_str() {
                        JOIN => {
                            _feed = Some(ctx.open_server_stream(FEED_CHANNEL).await?);
                            let snapshot = Snapshot {
                                life: 1,
                                slots: BTreeMap::new(),
                            };
                            channel
                                .send_response(message.id, JOIN, &JoinReply::Joined(snapshot))
                                .await?;
                        }
                        SET => {
                            let request = <SetRequest as unison::codec::Decodable<
                                unison::codec::ProtoCodec,
                            >>::decode(&message.payload)
                            .unwrap();
                            if request.slot == "slow" {
                                heard.notify_one();
                                continue;
                            }
                            rev += 1;
                            let update = SlotUpdate {
                                life: 1,
                                slot: request.slot,
                                rev,
                                payload: request.payload,
                            };
                            channel
                                .send_response(message.id, SET, &SetReply::Set(update))
                                .await?;
                        }
                        _ => {}
                    }
                }
            }
        })
        .await;
    let handle = Arc::new(server)
        .listener("[::1]:0")
        .spawn()
        .await
        .expect("偽の上流が立ち上がらない");
    (handle, heard_slow)
}

#[tokio::test]
#[ignore = "Medium: 実 QUIC runtime が要る"]
async fn relay_waiting_upstream_holds_up_no_other_leaf() {
    let (upstream, heard_slow) = start_stalling_upstream().await;
    let slave = within("上流に入る", ConnectedSlave::join(upstream.local_addr()))
        .await
        .unwrap();
    let relay = Arc::new(Relay::new(slave));
    let relay_server = SyncServer::start(relay.clone(), "[::1]:0").await.unwrap();

    let stuck = Arc::new(join(&relay_server).await);
    let slow = tokio::spawn({
        let stuck = stuck.clone();
        async move { stuck.set("slow", json!(1)).await }
    });
    within("頼みが上流に届く", heard_slow.notified()).await;
    assert!(!slow.is_finished());

    let other = join(&relay_server).await;
    let fast = within("ほかの Leaf の頼み", other.set("fast", json!(2)))
        .await
        .unwrap();
    assert_eq!((fast.rev, &fast.value()), (1, &json!(2)));
    assert!(!slow.is_finished());
    assert_eq!(
        other.set("", json!(1)).await,
        Err(SyncError::Refused("bad_request".into()))
    );

    relay.stop().await;
    let slow = within("待っていた頼みが終わる", slow).await.unwrap();
    assert!(slow.is_err(), "{slow:?}");

    relay_server.shutdown().await;
    let _ = upstream.shutdown().await;
}

#[tokio::test]
#[ignore = "Medium: 実 QUIC runtime が要る"]
async fn quic_proto_sync_and_no_alloc_view_through_relay() {
    let (origin, origin_server) = start_origin(1).await;
    let (relay, relay_server) = start_relay(&origin_server).await;
    let leaf = join(&relay_server).await;
    let mut direct = join(&origin_server).await;

    let msg = ProtocolMessage {
        id: 555,
        method: "stage.lighting".into(),
        msg_type: ::buffa::EnumValue::Known(ProtoMsgType::EVENT),
        payload: vec![0xDE, 0xAD, 0xBE, 0xEF],
        ..Default::default()
    };

    // Leaf から Relay 経由で Origin へ buffa protobuf メッセージを書き込む
    let confirmed = within("buffa 頼み", leaf.set_proto("lighting", &msg))
        .await
        .unwrap();
    assert_eq!(confirmed.rev, 1);
    let confirmed_view: ProtocolMessageView<'_> = confirmed.decode_view().unwrap();
    assert_eq!(confirmed_view.id, 555);
    assert_eq!(confirmed_view.method, "stage.lighting");
    assert_eq!(confirmed_view.payload, &[0xDE, 0xAD, 0xBE, 0xEF]);

    // Origin / Relay / 直結 Slave の全員にバイナリのまま届き、MessageView で zero-copy 読み取りできる
    assert!(
        origin
            .with_slot("lighting", |s| {
                let v: ProtocolMessageView<'_> = s.decode_view().unwrap();
                s.rev == 1 && v.id == 555 && v.method == "stage.lighting"
            })
            .unwrap()
    );
    assert_eq!(
        relay
            .snapshot()
            .unwrap()
            .slots
            .get("lighting")
            .unwrap()
            .decode_view::<ProtocolMessageView>()
            .unwrap()
            .method,
        "stage.lighting"
    );

    until(&mut direct, "直結 Slave に届く", |s| {
        s.get("lighting").is_some()
    })
    .await;
    assert!(
        direct
            .with_slot("lighting", |s| {
                let v: ProtocolMessageView<'_> = s.decode_view().unwrap();
                v.id == 555 && v.payload == [0xDE, 0xAD, 0xBE, 0xEF]
            })
            .unwrap()
    );

    relay_server.shutdown().await;
    origin_server.shutdown().await;
}
