use efescript::prelude::*;

const SRC: &str = include_str!("../examples/sound.efe");

#[test]
fn sound_ok() {
    let cap = CaptureHost::shared();
    let mut rt = ProgramRuntime::with_host(Box::new(cap.clone()));
    rt.load_from_vec(parse_str(SRC).unwrap());
    let res = rt.run_with_limit(1_000_000);
    assert_eq!(res, RunResult::Finished(0));
    assert_eq!(cap.borrow().sounds, vec!["examples/ominous_worse.ogg".to_string()]);
    assert_eq!(cap.borrow().out_string(), "sound ok\n");
}

/// A host whose sound device always fails.
struct FailingHost {
    out: Vec<u8>,
}

impl Host for FailingHost {
    fn write_out(&mut self, bytes: &[u8]) {
        self.out.extend_from_slice(bytes);
    }
    fn play_sound(&mut self, _path: &str) -> Result<(), String> {
        Err("no audio device".to_string())
    }
}

#[test]
fn sound_failed() {
    let host = std::rc::Rc::new(std::cell::RefCell::new(FailingHost { out: vec![] }));
    let mut rt = ProgramRuntime::with_host(Box::new(host.clone()));
    rt.load_from_vec(parse_str(SRC).unwrap());
    let res = rt.run_with_limit(1_000_000);
    match res {
        RunResult::Finished(c) => assert_ne!(c, 0),
        other => panic!("{other:?}"),
    }
    assert_eq!(String::from_utf8_lossy(&host.borrow().out), "sound failed\n");
}
