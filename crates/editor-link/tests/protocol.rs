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
        Request::SetMode { mode: Mode::Edit },
        Request::SaveScene { path: "a.ron".into() },
        Request::LoadScene { path: "a.ron".into() },
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
        Reply::Saved { path: "a.ron".into() },
        Reply::Loaded { path: "a.ron".into(), entities: 2 },
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
    let r = go(&mut e, vec![Request::LoadScene { path: "a.ron".into() }, Request::SetMode { mode: Mode::Edit }]);
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
    let r = go(&mut e, vec![Request::Scene, Request::LoadScene { path: "a.ron".into() }, Request::SetMode { mode: Mode::Edit }]);
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
    let r = go(&mut e, vec![Request::SetMode { mode: Mode::Play }]);
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
