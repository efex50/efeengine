use std::fs;
use std::path::{Path, PathBuf};

use efeengine_project::*;

const CANON: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<Project Sdk="efeengine/1">
  <PropertyGroup>
    <Name>coin_dash</Name>
    <Package>coin_dash</Package>
    <Example></Example>
    <StartScene>scenes/start.ron</StartScene>
    <EngineVersion>1</EngineVersion>
  </PropertyGroup>

  <ItemGroup>
    <Scene Include="scenes/start.ron" />
    <Scene Include="scenes/level_1.ron" />
    <Mod Include="mods/inventory.efmod" />
  </ItemGroup>
</Project>
"#;

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("efproj_test_{}_{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn err(text: &str) -> String {
    parse(text).expect_err("should fail").to_string()
}

fn with(body: &str) -> String {
    CANON.replace("<Example></Example>", body)
}

#[test]
fn canonical_round_trips_byte_for_byte() {
    let p = parse(CANON).unwrap();
    assert_eq!(p.name, "coin_dash");
    assert_eq!(p.example, None);
    assert_eq!(p.scenes, ["scenes/start.ron", "scenes/level_1.ron"]);
    assert_eq!(p.mods, ["mods/inventory.efmod"]);
    assert_eq!(write(&p), CANON);
    assert_eq!(parse(&write(&p)).unwrap(), p);
}

#[test]
fn example_and_empty_lists_round_trip() {
    let p = Project {
        name: "a".into(),
        package: "a&b".into(),
        example: Some("demo".into()),
        start_scene: "s.ron".into(),
        engine_version: 1,
        scenes: vec![],
        mods: vec![],
    };
    let t = write(&p);
    assert!(t.ends_with("</Project>\n"));
    assert_eq!(parse(&t).unwrap(), p);
}

#[test]
fn wrong_or_missing_sdk() {
    assert!(err(&CANON.replace("efeengine/1", "efeengine/2")).contains("efeengine/2"));
    assert!(err(&CANON.replace(" Sdk=\"efeengine/1\"", "")).contains("Sdk"));
}

#[test]
fn missing_name() {
    let t = CANON.replace("    <Name>coin_dash</Name>\n", "");
    assert!(err(&t).contains("Name"));
}

#[test]
fn unknown_element_has_line() {
    let t = CANON.replace("<Example></Example>", "<Exmaple></Exmaple>");
    let e = err(&t);
    assert!(e.starts_with("line 6:"), "{e}");
    assert!(e.contains("Exmaple"));
}

#[test]
fn unknown_attribute() {
    let t = CANON.replace("<Mod Include=\"mods/inventory.efmod\" />", "<Mod Include=\"m.efmod\" Foo=\"1\" />");
    let e = err(&t);
    assert!(e.contains("Foo") && e.contains("line 14"), "{e}");
    assert!(err(&CANON.replace("<PropertyGroup>", "<PropertyGroup x=\"1\">")).contains("`x`"));
}

#[test]
fn bad_include_paths() {
    assert!(err(&CANON.replace("mods/inventory.efmod", "/etc/passwd")).contains("absolute"));
    assert!(err(&CANON.replace("mods/inventory.efmod", "C:\\x.efmod")).contains("absolute"));
    assert!(err(&CANON.replace("mods/inventory.efmod", "../x.efmod")).contains(".."));
    assert!(err(&CANON.replace("scenes/level_1.ron", "a/../../b.ron")).contains(".."));
}

#[test]
fn engine_version_must_be_one() {
    assert!(err(&CANON.replace("<EngineVersion>1<", "<EngineVersion>2<")).contains("EngineVersion 2"));
    assert!(err(&CANON.replace("<EngineVersion>1<", "<EngineVersion>x<")).contains("not a number"));
    assert!(err(&CANON.replace("    <EngineVersion>1</EngineVersion>\n", "")).contains("EngineVersion"));
}

#[test]
fn case_sensitive_and_duplicates() {
    assert!(err(&CANON.replace("Package>", "package>")).contains("unknown"));
    assert!(err(&with("<Example></Example><Example></Example>")).contains("twice"));
    assert!(err("<Project Sdk=\"efeengine/1\"><Foo/></Project>").contains("Foo"));
}

fn make_valid(tag: &str) -> PathBuf {
    let d = tmp(tag);
    fs::create_dir_all(d.join("scenes")).unwrap();
    fs::create_dir_all(d.join("mods")).unwrap();
    fs::write(d.join("Cargo.toml"), "[package]\nname = \"coin_dash\"\nversion = \"0.1.0\"\n").unwrap();
    fs::write(d.join("scenes/start.ron"), "").unwrap();
    fs::write(d.join("scenes/level_1.ron"), "").unwrap();
    fs::write(d.join("mods/inventory.efmod"), "").unwrap();
    fs::write(d.join("coin_dash.efproj"), CANON).unwrap();
    d
}

fn fields(d: &Path) -> Vec<String> {
    check(d).into_iter().map(|i| i.field).collect()
}

#[test]
fn check_valid_has_no_issues() {
    let d = make_valid("valid");
    assert_eq!(check(&d), vec![]);
}

#[test]
fn check_package_mismatch() {
    let d = make_valid("pkg");
    fs::write(d.join("Cargo.toml"), "[package]\nname = \"other\"\n").unwrap();
    let i = check(&d);
    assert_eq!(i.len(), 1);
    assert_eq!(i[0].field, "Package");
    assert!(i[0].message.contains("other"));
}

#[test]
fn check_missing_start_scene() {
    let d = make_valid("start");
    fs::write(d.join("coin_dash.efproj"), CANON.replace("<StartScene>scenes/start.ron", "<StartScene>scenes/nope.ron")).unwrap();
    assert_eq!(fields(&d), ["StartScene"]);
}

#[test]
fn check_missing_files_and_cargo_collects_all() {
    let d = make_valid("files");
    fs::remove_file(d.join("scenes/level_1.ron")).unwrap();
    assert_eq!(fields(&d), ["Scene"]);
    fs::remove_file(d.join("mods/inventory.efmod")).unwrap();
    assert_eq!(fields(&d), ["Scene", "Mod"]);
    fs::remove_file(d.join("Cargo.toml")).unwrap();
    assert_eq!(fields(&d), ["Cargo.toml", "Scene", "Mod"]);
}

#[test]
fn check_reports_bad_or_missing_project_file() {
    let d = make_valid("bad");
    fs::write(d.join("coin_dash.efproj"), "<Project Sdk=\"efeengine/9\"/>").unwrap();
    assert_eq!(fields(&d), [".efproj"]);
    assert_eq!(fields(&tmp("none")), [".efproj"]);
}

#[test]
fn create_writes_files_that_check() {
    let parent = tmp("create");
    let f = create(&parent, "my_game").unwrap();
    let d = parent.join("my_game");
    assert_eq!(f, d.join("my_game.efproj"));
    for p in ["my_game.efproj", "Cargo.toml", "src/main.rs", "scenes/start.ron"] {
        assert!(d.join(p).is_file(), "{p}");
    }
    let cargo = fs::read_to_string(d.join("Cargo.toml")).unwrap();
    assert!(cargo.contains(&format!("efeengine-core = {{ git = \"{}\"", efeengine_project::ENGINE_GIT_URL)), "{cargo}");
    assert!(cargo.contains("rev = \"") || cargo.contains("branch = \"main\""), "{cargo}");
    assert!(!cargo.contains("path ="), "no local path in the scaffold: {cargo}");
    assert_eq!(check(&d), vec![]);
    let p = parse(&fs::read_to_string(&f).unwrap()).unwrap();
    assert_eq!(p.start_scene, "scenes/start.ron");
    // Nothing else was written.
    let mut n = 0;
    fn count(d: &Path, n: &mut usize) {
        for e in fs::read_dir(d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() { count(&p, n) } else { *n += 1 }
        }
    }
    count(&d, &mut n);
    assert_eq!(n, 4);
    // The scene is valid RON in format 1 (loading needs the engine, which this crate avoids).
    let scene = fs::read_to_string(d.join("scenes/start.ron")).unwrap();
    assert!(scene.starts_with("Scene(") && scene.contains("format: 1,") && scene.contains("entities: []"));
}

#[test]
fn create_refuses_non_empty_but_accepts_empty() {
    let parent = tmp("nonempty");
    fs::create_dir_all(parent.join("g")).unwrap();
    create(&parent, "g").expect("empty folder is fine");
    let e = create(&parent, "g").unwrap_err();
    assert!(matches!(e, ProjectError::NotEmpty(_)), "{e}");
}

#[test]
fn create_rejects_bad_names() {
    let parent = tmp("names");
    for n in ["", "A", "1a", "a-b", "a b", "..", "a/b", "a\\b", "a.b", "../x", &"a".repeat(65)] {
        assert!(create(&parent, n).is_err(), "{n:?}");
    }
    assert!(create(&parent, &"a".repeat(64)).is_ok());
    assert_eq!(fs::read_dir(&parent).unwrap().count(), 1);
}

#[test]
fn find_walks_up() {
    let d = tmp("find");
    fs::create_dir_all(d.join("a/b")).unwrap();
    fs::write(d.join("x.efproj"), CANON).unwrap();
    assert_eq!(find(&d.join("a/b")), Some(d.join("x.efproj")));
    assert_eq!(find(&d.join("a/b/file.rs")), Some(d.join("x.efproj")));
    fs::write(d.join("a/inner.efproj"), CANON).unwrap();
    assert_eq!(find(&d.join("a/b")), Some(d.join("a/inner.efproj")));
}

#[test]
fn find_none_without_project() {
    let d = tmp("findnone");
    fs::create_dir_all(d.join("a")).unwrap();
    // The temp dir's ancestors must not hold a project for this to be meaningful.
    if find(&std::env::temp_dir()).is_none() {
        assert_eq!(find(&d.join("a")), None);
    }
}
