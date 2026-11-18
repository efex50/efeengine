//! Protocol layer tests plus end-to-end tests against a headless engine over a real socket.

use std::net::TcpStream;
use std::time::Duration;

use efeengine_core::engine::components::{EditorId, Name};
use efeengine_core::engine::types::entity_to_bits;
use efeengine_core::prelude::*;
use efeengine_editor_link::protocol::ComponentInfo;
use efeengine_editor_link::*;
use serde_json::json;

#[script_component(name = "Door")]
#[derive(Default)]
struct Door {
    open: ScriptBool,
    _pad: [u8; 3],
    speed: f32,
}

fn round_trip<T: serde::Serialize + for<'de> serde::Deserialize<'de> + PartialEq + std::fmt::Debug>(v: T) {
    let mut buf = Vec::new();
    send(&mut buf, &v).unwrap();
    assert_eq!(&buf[..4], &((buf.len() - 4) as u32).to_be_bytes());
    let back: T = recv(&mut buf.as_slice()).unwrap();
    assert_eq!(back, v);
}

#[test]
fn every_message_round_trips() {
    round_trip(ClientMessage::Hello { protocol: 1, client: "t".into() });
    for r in [
        Request::Entities,
        Request::Schema,
        Request::Stats,
        Request::Logs { count: 5 },
        Request::Components { entity: Some(9), editor_id: None },
        Request::Components { entity: None, editor_id: Some("a".into()) },
        Request::Systems,
        Request::Scene,
        Request::SetMode { mode: Mode::Edit, keep_changes: false },
        Request::SetMode { mode: Mode::Play, keep_changes: true },
        Request::Stop { keep_changes: true },
        Request::Fidelity,
        Request::SaveScene { path: "a.ron".into() },
        Request::LoadScene { path: "a.ron".into() },
        Request::SetField { entity: Some(1), editor_id: None, component: "Door".into(), field: "speed".into(), value: json!(2.5) },
        Request::Spawn { components: Default::default(), editor_id: Some("3".into()) },
        Request::Despawn { entity: None, editor_id: Some("3".into()) },
    ] {
        round_trip(ClientMessage::Request { id: 3, request: r });
    }
    round_trip(ServerMessage::Hello { protocol: 1, game: "g".into() });
    round_trip(ServerMessage::Error { code: "c".into(), message: "m".into() });
    for r in [
        Reply::Entities { entities: vec![EntityInfo { editor_id: None, entity: 1, name: Some("n".into()) }] },
        Reply::Components { entity: 1, components: vec![ComponentInfo { name: "A".into(), id: 2, fields: Default::default() }] },
        Reply::Schema { schema: json!({"format": "efe-schema"}) },
        Reply::Stats { frame: 1, frame_time: 0.5, entity_count: 2 },
        Reply::Logs { lines: vec![LogLine { level: "info".into(), text: "x".into() }] },
        Reply::error("unknown_entity", "nope"),
        Reply::Systems { stages: vec![StageInfo { stage: "Update".into(), systems: vec![SystemInfo { name: "s".into(), enabled: true, global: false }] }] },
        Reply::Scene { name: Some("a".into()), path: None, mode: Mode::Play, project_dir: ".".into() },
        Reply::Mode { mode: Mode::Edit },
        Reply::Fidelity { gaps: vec!["a::B".into()], not_covered: vec!["events".into()], snapshot: true },
        Reply::Saved { path: "a.ron".into() },
        Reply::Loaded { path: "a.ron".into(), entities: 2 },
        Reply::FieldSet { entity: 1, old: json!([1.0, 2.0]) },
        Reply::Spawned { entity: 1, editor_id: "4".into() },
        Reply::Despawned { owned: true, editor_id: None, components: vec![] },
    ] {
        round_trip(ServerMessage::Reply { id: 1, reply: r });
    }
}

#[test]
fn wire_shapes() {
    let v = serde_json::to_value(ClientMessage::Hello { protocol: 1, client: "x".into() }).unwrap();
    assert_eq!(v, json!({"hello": {"protocol": 1, "client": "x"}}));
    let v = serde_json::to_value(Reply::error("c", "m")).unwrap();
    assert_eq!(v, json!({"error": {"code": "c", "message": "m"}}));
}

#[test]
fn handshake_rejects_wrong_version() {
    let ok = handshake(&ClientMessage::Hello { protocol: PROTOCOL_VERSION, client: "t".into() }, "g");
    assert_eq!(ok, Ok(ServerMessage::Hello { protocol: PROTOCOL_VERSION, game: "g".into() }));
    let bad = handshake(&ClientMessage::Hello { protocol: 99, client: "t".into() }, "g");
    assert!(matches!(bad, Err(ServerMessage::Error { ref code, .. }) if code == "protocol_mismatch"));
    let early = handshake(&ClientMessage::Request { id: 1, request: Request::Stats }, "g");
    assert!(matches!(early, Err(ServerMessage::Error { ref code, .. }) if code == "handshake_required"));
}

#[test]
fn frame_reader_handles_split_and_oversized_frames() {
    let mut data = Vec::new();
    write_frame(&mut data, b"{\"a\":1}").unwrap();
    let mut fr = FrameReader::default();
    assert_eq!(fr.poll(&mut &data[..3]).unwrap(), None);
    assert_eq!(fr.poll(&mut &data[3..]).unwrap().unwrap(), b"{\"a\":1}");
    let huge = ((MAX_FRAME + 1) as u32).to_be_bytes();
    assert!(FrameReader::default().poll(&mut &huge[..]).is_err());
}

fn engine() -> (Engine, std::sync::Arc<std::sync::atomic::AtomicU16>) {
    let mut e = Engine::with_config(EngineConfig { fixed_dt: 1.0 / 60.0, ..EngineConfig::headless() }).expect("headless engine");
    e.register::<Door>().unwrap();
    let link = EditorLink::new(0);
    let port = link.port_handle();
    e.add_plugin(link).unwrap();
    (e, port)
}

fn connect(port: u16) -> TcpStream {
    let s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s
}

fn hello(s: &mut TcpStream) -> ServerMessage {
    send(s, &ClientMessage::Hello { protocol: PROTOCOL_VERSION, client: "test".into() }).unwrap();
    recv(s).unwrap()
}

/// Run `client` on a thread against the engine's frames until it finishes.
fn drive<T: Send + 'static>(e: &mut Engine, client: impl FnOnce() -> T + Send + 'static) -> T {
    let h = std::thread::spawn(client);
    while !h.is_finished() {
        e.run_frames(1, 1.0 / 60.0);
        std::thread::sleep(Duration::from_millis(2));
    }
    h.join().unwrap()
}

fn ask(s: &mut TcpStream, id: u64, request: Request) -> Reply {
    send(s, &ClientMessage::Request { id, request }).unwrap();
    match recv::<ServerMessage>(s).unwrap() {
        ServerMessage::Reply { id: rid, reply } => {
            assert_eq!(rid, id);
            reply
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn smoke_handshake_and_stats() {
    let (mut e, port) = engine();
    let port = port.load(std::sync::atomic::Ordering::Relaxed);
    assert_ne!(port, 0);
    e.world_mut().spawn((Name::new("hero"),));
    let (h, r) = drive(&mut e, move || {
        let mut s = connect(port);
        let h = hello(&mut s);
        (h, ask(&mut s, 1, Request::Stats))
    });
    assert!(matches!(h, ServerMessage::Hello { protocol: PROTOCOL_VERSION, .. }));
    match r {
        Reply::Stats { entity_count, .. } => assert_eq!(entity_count, 1),
        other => panic!("{other:?}"),
    }
}

#[test]
fn wrong_protocol_over_socket_is_refused_and_closed() {
    let (mut e, port) = engine();
    let port = port.load(std::sync::atomic::Ordering::Relaxed);
    let (first, second) = drive(&mut e, move || {
        let mut s = connect(port);
        send(&mut s, &ClientMessage::Hello { protocol: 42, client: "old".into() }).unwrap();
        let first: ServerMessage = recv(&mut s).unwrap();
        (first, recv::<ServerMessage>(&mut s).is_err())
    });
    assert!(matches!(first, ServerMessage::Error { ref code, .. } if code == "protocol_mismatch"));
    assert!(second, "connection must be closed");
}

#[test]
fn entities_components_schema_logs_and_errors() {
    let (mut e, port) = engine();
    let port = port.load(std::sync::atomic::Ordering::Relaxed);
    let door = e.world_mut().spawn((Name::new("front door"), Door { open: true.into(), _pad: [0; 3], speed: 2.5 }));
    let bits = entity_to_bits(door);
    e.logger.info("hello from the test");
    let mut stale = e.world_mut().spawn(());
    let stale_bits = entity_to_bits(stale);
    e.world_mut().despawn(stale).unwrap();
    stale = door; // silence unused-assign lint
    let _ = stale;

    let replies = drive(&mut e, move || {
        let mut s = connect(port);
        hello(&mut s);
        vec![
            ask(&mut s, 1, Request::Entities),
            ask(&mut s, 2, Request::Components { entity: Some(bits), editor_id: None }),
            ask(&mut s, 3, Request::Schema),
            ask(&mut s, 4, Request::Logs { count: 100 }),
            ask(&mut s, 5, Request::Components { entity: Some(stale_bits), editor_id: None }),
            ask(&mut s, 6, Request::Components { entity: Some(12345), editor_id: None }),
            ask(&mut s, 7, Request::Components { entity: None, editor_id: None }),
        ]
    });

    match &replies[0] {
        Reply::Entities { entities } => {
            assert!(entities.iter().any(|x| x.entity == bits && x.name.as_deref() == Some("front door")));
        }
        o => panic!("{o:?}"),
    }
    match &replies[1] {
        Reply::Components { components, .. } => {
            let d = components.iter().find(|c| c.name == "Door").expect("Door");
            assert_eq!(d.fields["open"], json!(true));
            assert_eq!(d.fields["speed"], json!(2.5));
        }
        o => panic!("{o:?}"),
    }
    match &replies[2] {
        Reply::Schema { schema } => {
            assert_eq!(schema["format"], "efe-schema");
            assert!(schema["components"].as_array().unwrap().iter().any(|c| c["name"] == "Door"));
        }
        o => panic!("{o:?}"),
    }
    match &replies[3] {
        Reply::Logs { lines } => assert!(lines.iter().any(|l| l.text == "hello from the test")),
        o => panic!("{o:?}"),
    }
    for r in &replies[4..] {
        assert!(matches!(r, Reply::Error { .. }), "{r:?}");
    }
    assert!(matches!(&replies[4], Reply::Error { code, .. } if code == "unknown_entity"));
}

fn err_code(r: &Reply) -> &str {
    match r {
        Reply::Error { code, .. } => code,
        o => panic!("expected error, got {o:?}"),
    }
}

#[test]
fn editor_id_lookup_and_entities_fill_it() {
    let (mut e, port) = engine();
    let port = port.load(std::sync::atomic::Ordering::Relaxed);
    let door = e.world_mut().spawn((Name::new("d"), EditorId { id: 42 }, Door { open: true.into(), _pad: [0; 3], speed: 1.5 }));
    let plain = e.world_mut().spawn((Name::new("p"),));
    let (door_bits, plain_bits) = (entity_to_bits(door), entity_to_bits(plain));
    let replies = drive(&mut e, move || {
        let mut s = connect(port);
        hello(&mut s);
        vec![
            ask(&mut s, 1, Request::Entities),
            ask(&mut s, 2, Request::Components { entity: None, editor_id: Some("42".into()) }),
            ask(&mut s, 3, Request::Components { entity: None, editor_id: Some("43".into()) }),
            ask(&mut s, 4, Request::Components { entity: None, editor_id: Some("abc".into()) }),
        ]
    });
    match &replies[0] {
        Reply::Entities { entities } => {
            let d = entities.iter().find(|x| x.entity == door_bits).unwrap();
            assert_eq!(d.editor_id.as_deref(), Some("42"));
            assert_eq!(entities.iter().find(|x| x.entity == plain_bits).unwrap().editor_id, None);
        }
        o => panic!("{o:?}"),
    }
    match &replies[1] {
        Reply::Components { entity, components } => {
            assert_eq!(*entity, door_bits);
            assert!(components.iter().any(|c| c.name == "Door"));
            assert!(components.iter().any(|c| c.name == "EditorId" && c.fields["id"] == json!(42)));
        }
        o => panic!("{o:?}"),
    }
    assert_eq!(err_code(&replies[2]), "unknown_editor_id");
    assert_eq!(err_code(&replies[3]), "unknown_editor_id");
}

#[test]
fn systems_lists_flags() {
    let (mut e, port) = engine();
    let port = port.load(std::sync::atomic::Ordering::Relaxed);
    e.add_system(Stage::Update, |_| {}).name("plain_sys");
    e.add_system(Stage::Update, |_| {}).name("off_sys").enabled(false);
    e.add_system(Stage::Update, |_| {}).name("global_sys");
    e.set_global_system("global_sys").unwrap();
    let r = drive(&mut e, move || {
        let mut s = connect(port);
        hello(&mut s);
        ask(&mut s, 1, Request::Systems)
    });
    let Reply::Systems { stages } = r else { panic!("{r:?}") };
    let find = |n: &str| stages.iter().flat_map(|s| s.systems.iter()).find(|s| s.name == n).cloned().unwrap_or_else(|| panic!("{n}"));
    assert_eq!(find("plain_sys"), SystemInfo { name: "plain_sys".into(), enabled: true, global: false });
    assert!(!find("off_sys").enabled);
    assert!(find("global_sys").global);
    assert!(find("editor_link_drain").global && find("editor_link_drain").enabled);
    assert!(stages.iter().find(|s| s.stage == "Update").unwrap().systems.iter().any(|s| s.name == "plain_sys"));
}

fn temp_project(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("efe_editor_link_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn mode_engine(dir: &std::path::Path) -> (Engine, std::sync::Arc<std::sync::atomic::AtomicU16>) {
    let mut e = Engine::with_config(EngineConfig { fixed_dt: 1.0 / 60.0, ..EngineConfig::headless() }).expect("headless engine");
    e.register::<Door>().unwrap();
    let link = EditorLink::new(0).with_project_dir(dir);
    let port = link.port_handle();
    e.add_plugin(link).unwrap();
    (e, port)
}

fn counter(e: &mut Engine, name: &'static str) -> std::sync::Arc<std::sync::atomic::AtomicU32> {
    let c = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let c2 = c.clone();
    e.add_system(Stage::Update, move |_| {
        c2.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    })
    .name(name);
    c
}

#[test]
fn set_mode_edit_and_play() {
    use std::sync::atomic::Ordering::Relaxed;
    let dir = temp_project("mode");
    std::fs::write(
        dir.join("a.ron"),
        r#"Scene(format: 1, name: "a", systems: ["scene_sys"], entities: [(components: {"Door": {"speed": 2.5}})])"#,
    )
    .unwrap();
    let (mut e, port) = mode_engine(&dir);
    let port = port.load(Relaxed);
    let plain = counter(&mut e, "plain_sys");
    let scene_sys = counter(&mut e, "scene_sys");
    let global = counter(&mut e, "global_sys");
    e.set_global_system("global_sys").unwrap();
    e.set_system_enabled("scene_sys", false).unwrap();

    let go = |e: &mut Engine, reqs: Vec<Request>| -> Vec<Reply> {
        drive(e, move || {
            let mut s = connect(port);
            hello(&mut s);
            reqs.into_iter().enumerate().map(|(i, r)| ask(&mut s, i as u64 + 1, r)).collect()
        })
    };

    // Play mode: load the scene (enables scene_sys), then enter edit mode.
    let r = go(&mut e, vec![Request::LoadScene { path: "a.ron".into() }, Request::SetMode { mode: Mode::Edit, keep_changes: false }]);
    assert_eq!(r[0], Reply::Loaded { path: "a.ron".into(), entities: 1 });
    assert_eq!(r[1], Reply::Mode { mode: Mode::Edit });
    assert_eq!(e.is_system_enabled("plain_sys"), Some(false));
    assert_eq!(e.is_system_enabled("scene_sys"), Some(false));
    assert_eq!(e.is_system_enabled("global_sys"), Some(true));
    assert_eq!(e.is_system_enabled("editor_link_drain"), Some(true));
    let (p0, s0, g0) = (plain.load(Relaxed), scene_sys.load(Relaxed), global.load(Relaxed));
    e.run_frames(5, 1.0 / 60.0);
    assert_eq!((plain.load(Relaxed), scene_sys.load(Relaxed)), (p0, s0), "game systems must not run in edit mode");
    assert_eq!(global.load(Relaxed), g0 + 5);

    // The link still answers in edit mode; scene info reports the mode; reloading keeps edit mode.
    let r = go(&mut e, vec![Request::Scene, Request::LoadScene { path: "a.ron".into() }, Request::SetMode { mode: Mode::Edit, keep_changes: false }]);
    match &r[0] {
        Reply::Scene { name, path, mode, .. } => {
            assert_eq!((name.as_deref(), path.as_deref(), *mode), (Some("a"), Some("a.ron"), Mode::Edit));
        }
        o => panic!("{o:?}"),
    }
    assert!(matches!(r[1], Reply::Loaded { .. }));
    assert_eq!(r[2], Reply::Mode { mode: Mode::Edit });
    assert_eq!(e.is_system_enabled("scene_sys"), Some(false), "a scene switch in edit mode must not run systems");

    // Back to play: the scene's system and the previously enabled one run again.
    let r = go(&mut e, vec![Request::SetMode { mode: Mode::Play, keep_changes: false }]);
    assert_eq!(r[0], Reply::Mode { mode: Mode::Play });
    assert_eq!(e.is_system_enabled("plain_sys"), Some(true));
    assert_eq!(e.is_system_enabled("scene_sys"), Some(true));
    let (p1, s1) = (plain.load(Relaxed), scene_sys.load(Relaxed));
    e.run_frames(3, 1.0 / 60.0);
    assert_eq!((plain.load(Relaxed), scene_sys.load(Relaxed)), (p1 + 3, s1 + 3));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn save_and_load_scene_and_rejected_paths() {
    let dir = temp_project("save");
    std::fs::write(
        dir.join("a.ron"),
        r#"Scene(format: 1, name: "a", entities: [(components: {"Door": {"speed": 2.5}}), (components: {"Door": {"speed": 4.0}})])"#,
    )
    .unwrap();
    std::fs::write(dir.join("bad.ron"), "not a scene").unwrap();
    let (mut e, port) = mode_engine(&dir);
    let port = port.load(std::sync::atomic::Ordering::Relaxed);
    let r = drive(&mut e, move || {
        let mut s = connect(port);
        hello(&mut s);
        let reqs = vec![
            Request::LoadScene { path: "a.ron".into() },
            Request::SaveScene { path: "out.ron".into() },
            Request::SaveScene { path: "/tmp/efe_abs_escape.ron".into() },
            Request::SaveScene { path: "../escape.ron".into() },
            Request::SaveScene { path: "x/../../escape.ron".into() },
            Request::SaveScene { path: "".into() },
            Request::LoadScene { path: "../a.ron".into() },
            Request::LoadScene { path: "missing.ron".into() },
            Request::LoadScene { path: "bad.ron".into() },
            Request::SaveScene { path: "nodir/out.ron".into() },
            Request::LoadScene { path: "out.ron".into() },
        ];
        reqs.into_iter().enumerate().map(|(i, r)| ask(&mut s, i as u64 + 1, r)).collect::<Vec<_>>()
    });
    assert_eq!(r[0], Reply::Loaded { path: "a.ron".into(), entities: 2 });
    assert_eq!(r[1], Reply::Saved { path: "out.ron".into() });
    for (i, reply) in r.iter().enumerate().take(7).skip(2) {
        assert_eq!(err_code(reply), "bad_path", "{i}: {reply:?}");
    }
    assert_eq!(err_code(&r[7]), "scene_error");
    assert_eq!(err_code(&r[8]), "scene_error");
    assert_eq!(err_code(&r[9]), "scene_error");
    assert_eq!(r[10], Reply::Loaded { path: "out.ron".into(), entities: 2 });
    assert!(!std::path::Path::new("/tmp/efe_abs_escape.ron").exists());
    assert!(!dir.parent().unwrap().join("escape.ron").exists());
    let saved = std::fs::read_to_string(dir.join("out.ron")).unwrap();
    assert!(saved.contains("2.5") && saved.contains("4"), "{saved}");
    let _ = std::fs::remove_dir_all(&dir);
}

struct Unregistered(#[allow(dead_code)] u32);

fn door_speeds(e: &Engine) -> Vec<f32> {
    e.world().query::<&Door>().iter().map(|d| d.speed).collect()
}

fn set_door_speed(e: &mut Engine, v: f32) {
    for d in e.world_mut().query_mut::<&mut Door>() {
        d.speed = v;
    }
}

#[test]
fn play_then_stop_restores_and_keep_changes_keeps() {
    use std::sync::atomic::Ordering::Relaxed;
    let dir = temp_project("play");
    let (mut e, port) = mode_engine(&dir);
    let port = port.load(Relaxed);
    e.world_mut().spawn((Door { speed: 1.0, ..Default::default() },));
    let go = |e: &mut Engine, req: Request| -> Reply {
        drive(e, move || {
            let mut s = connect(port);
            hello(&mut s);
            ask(&mut s, 1, req)
        })
    };
    let edit = Request::SetMode { mode: Mode::Edit, keep_changes: false };
    let play = Request::SetMode { mode: Mode::Play, keep_changes: false };
    assert_eq!(go(&mut e, edit.clone()), Reply::Mode { mode: Mode::Edit });

    // play, change, stop: back to the old value.
    assert_eq!(go(&mut e, play.clone()), Reply::Mode { mode: Mode::Play });
    assert_eq!(go(&mut e, play.clone()), Reply::Mode { mode: Mode::Play }, "play while playing is a no-op");
    set_door_speed(&mut e, 9.0);
    assert_eq!(go(&mut e, Request::Stop { keep_changes: false }), Reply::Mode { mode: Mode::Edit });
    assert_eq!(door_speeds(&e), vec![1.0]);
    assert_eq!(e.is_system_enabled("editor_link_drain"), Some(true));

    // play, change, stop with keep_changes: the change stays.
    assert_eq!(go(&mut e, play.clone()), Reply::Mode { mode: Mode::Play });
    set_door_speed(&mut e, 9.0);
    assert_eq!(go(&mut e, Request::SetMode { mode: Mode::Edit, keep_changes: true }), Reply::Mode { mode: Mode::Edit });
    assert_eq!(door_speeds(&e), vec![9.0]);

    // The snapshot is gone: stop is an error and changes nothing.
    let r = go(&mut e, Request::Stop { keep_changes: false });
    assert_eq!(err_code(&r), "no_snapshot");
    assert_eq!(door_speeds(&e), vec![9.0]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn stop_without_play_is_an_error() {
    use std::sync::atomic::Ordering::Relaxed;
    let dir = temp_project("stop");
    let (mut e, port) = mode_engine(&dir);
    let port = port.load(Relaxed);
    let r = drive(&mut e, move || {
        let mut s = connect(port);
        hello(&mut s);
        ask(&mut s, 1, Request::Stop { keep_changes: false })
    });
    assert_eq!(err_code(&r), "no_snapshot");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn fidelity_lists_unregistered_resources() {
    use std::sync::atomic::Ordering::Relaxed;
    let dir = temp_project("fid");
    let (mut e, port) = mode_engine(&dir);
    let port = port.load(Relaxed);
    e.insert_resource(Unregistered(1));
    let r = drive(&mut e, move || {
        let mut s = connect(port);
        hello(&mut s);
        ask(&mut s, 1, Request::Fidelity)
    });
    match r {
        Reply::Fidelity { gaps, not_covered, snapshot } => {
            assert!(gaps.iter().any(|g| g.ends_with("Unregistered")), "{gaps:?}");
            assert!(!not_covered.is_empty());
            assert!(!snapshot);
        }
        o => panic!("{o:?}"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

fn set_field_req(entity: u64, component: &str, field: &str, value: serde_json::Value) -> Request {
    Request::SetField { entity: Some(entity), editor_id: None, component: component.into(), field: field.into(), value }
}

fn door_of(e: &Engine, ent: Entity) -> (bool, f32) {
    let d = e.world().get::<&Door>(ent).unwrap();
    (d.open.get(), d.speed)
}

#[test]
fn set_field_in_edit_and_play_and_rejections() {
    let (mut e, port) = engine();
    let port = port.load(std::sync::atomic::Ordering::Relaxed);
    let door = e.world_mut().spawn((EditorId { id: 5 }, Door { open: false.into(), _pad: [0; 3], speed: 1.0 }, Transform::default()));
    let other = e.world_mut().spawn((EditorId { id: 6 },));
    let bits = entity_to_bits(door);
    let gone = e.world_mut().spawn(());
    let gone_bits = entity_to_bits(gone);
    e.world_mut().despawn(gone).unwrap();
    let replies = drive(&mut e, move || {
        let mut s = connect(port);
        hello(&mut s);
        vec![
            ask(&mut s, 1, Request::SetMode { mode: Mode::Edit, keep_changes: false }),
            ask(&mut s, 2, set_field_req(bits, "Door", "speed", json!(2.5))),
            ask(&mut s, 3, Request::SetField { entity: None, editor_id: Some("5".into()), component: "Door".into(), field: "open".into(), value: json!(true) }),
            ask(&mut s, 4, Request::SetMode { mode: Mode::Play, keep_changes: false }),
            ask(&mut s, 5, set_field_req(bits, "Door", "speed", json!(4.0))),
            // rejections
            ask(&mut s, 6, set_field_req(bits, "Door", "speed", json!("fast"))),
            ask(&mut s, 7, set_field_req(bits, "Door", "speed", json!([1, 2]))),
            ask(&mut s, 8, set_field_req(bits, "Door", "open", json!(7))),
            ask(&mut s, 9, set_field_req(bits, "Door", "nope", json!(1))),
            ask(&mut s, 10, set_field_req(gone_bits, "Door", "speed", json!(1))),
            ask(&mut s, 11, set_field_req(bits, "Nope", "speed", json!(1))),
            ask(&mut s, 12, set_field_req(entity_to_bits(other), "Door", "speed", json!(1))),
            ask(&mut s, 13, set_field_req(bits, "Transform", "position", json!([1.0, 2.0]))),
            ask(&mut s, 14, set_field_req(bits, "Transform", "position", json!([1.0]))),
            ask(&mut s, 15, set_field_req(bits, "EditorId", "id", json!(6))),
        ]
    });
    assert!(matches!(replies[1], Reply::FieldSet { old: ref o, .. } if *o == json!(1.0)), "{:?}", replies[1]);
    assert!(matches!(replies[2], Reply::FieldSet { old: ref o, .. } if *o == json!(false)), "{:?}", replies[2]);
    assert!(matches!(replies[4], Reply::FieldSet { old: ref o, .. } if *o == json!(2.5)), "{:?}", replies[4]);
    assert_eq!(err_code(&replies[5]), "unsupported_type");
    assert_eq!(err_code(&replies[6]), "bad_value");
    assert_eq!(err_code(&replies[7]), "bad_value");
    assert_eq!(err_code(&replies[8]), "unknown_field");
    assert_eq!(err_code(&replies[9]), "unknown_entity");
    assert_eq!(err_code(&replies[10]), "unknown_component");
    assert_eq!(err_code(&replies[11]), "missing_component");
    assert!(matches!(replies[12], Reply::FieldSet { .. }), "{:?}", replies[12]);
    assert_eq!(err_code(&replies[13]), "bad_value");
    assert_eq!(err_code(&replies[14]), "duplicate_editor_id");
    assert_eq!(door_of(&e, door), (true, 4.0));
    assert_eq!(e.world().get::<&Transform>(door).unwrap().position.to_array()[..2], [1.0, 2.0]);
}

#[test]
fn spawn_and_despawn_over_the_socket() {
    let (mut e, port) = engine();
    let port = port.load(std::sync::atomic::Ordering::Relaxed);
    let plain = e.world_mut().spawn((EditorId { id: 9 },));
    let plain_bits = entity_to_bits(plain);
    let replies = drive(&mut e, move || {
        let mut s = connect(port);
        hello(&mut s);
        let comps = json!({"Door": {"open": true, "speed": 3.0}}).as_object().unwrap().clone();
        vec![
            ask(&mut s, 1, Request::Spawn { components: comps.clone(), editor_id: None }),
            ask(&mut s, 2, Request::Spawn { components: comps.clone(), editor_id: Some("9".into()) }),
            ask(&mut s, 3, Request::Spawn { components: comps.clone(), editor_id: Some("20".into()) }),
            ask(&mut s, 4, Request::Spawn { components: json!({"Door": {"bogus": 1}}).as_object().unwrap().clone(), editor_id: None }),
            ask(&mut s, 5, Request::Despawn { entity: None, editor_id: Some("20".into()) }),
            ask(&mut s, 6, Request::Despawn { entity: Some(plain_bits), editor_id: None }),
            ask(&mut s, 7, Request::Despawn { entity: Some(plain_bits), editor_id: None }),
        ]
    });
    let auto = match &replies[0] {
        Reply::Spawned { entity, editor_id } => {
            assert_eq!(editor_id, "10", "max existing (9) + 1");
            *entity
        }
        o => panic!("{o:?}"),
    };
    assert_eq!(err_code(&replies[1]), "duplicate_editor_id");
    assert!(matches!(&replies[2], Reply::Spawned { editor_id, .. } if editor_id == "20"));
    assert_eq!(err_code(&replies[3]), "unknown_field");
    match &replies[4] {
        Reply::Despawned { owned, editor_id, components } => {
            assert!(*owned);
            assert_eq!(editor_id.as_deref(), Some("20"));
            let door = components.iter().find(|c| c.name == "Door").unwrap();
            assert_eq!(door.fields["speed"], json!(3.0));
        }
        o => panic!("{o:?}"),
    }
    assert!(matches!(&replies[5], Reply::Despawned { owned: false, .. }), "{:?}", replies[5]);
    assert_eq!(err_code(&replies[6]), "unknown_entity");
    // The spawned entity is scene-owned and shows up in the captured scene.
    let auto_e = efeengine_core::engine::types::entity_from_bits(auto).unwrap();
    assert!(e.is_scene_owned(auto_e));
    assert_eq!(door_of(&e, auto_e), (true, 3.0));
    let ron = e.capture_scene("t").to_ron();
    assert!(ron.contains("Door") && ron.contains("EditorId"), "{ron}");
    assert_eq!(e.capture_scene("t").entities.len(), 1, "only the owned entity: {ron}");
}
