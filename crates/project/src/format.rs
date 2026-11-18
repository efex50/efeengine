use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use crate::{ENGINE_VERSION, Project, ProjectError, SDK, path_problem};

fn line_at(text: &str, pos: usize) -> usize {
    let end = pos.min(text.len());
    text.as_bytes()[..end].iter().filter(|&&b| b == b'\n').count() + 1
}

struct Ctx<'a> {
    text: &'a str,
    pos: usize,
}

impl Ctx<'_> {
    fn err<T>(&self, msg: impl Into<String>) -> Result<T, ProjectError> {
        Err(ProjectError::Parse { line: line_at(self.text, self.pos), msg: msg.into() })
    }
}

fn name_of(e: &BytesStart) -> String {
    String::from_utf8_lossy(e.name().as_ref()).into_owned()
}

fn end_name(e: &quick_xml::events::BytesEnd) -> String {
    String::from_utf8_lossy(e.name().as_ref()).into_owned()
}

fn attrs(c: &Ctx, e: &BytesStart) -> Result<Vec<(String, String)>, ProjectError> {
    let mut out = Vec::new();
    for a in e.attributes() {
        let a = match a {
            Ok(a) => a,
            Err(x) => return c.err(format!("bad attribute: {x}")),
        };
        let k = String::from_utf8_lossy(a.key.as_ref()).into_owned();
        let v = match a.normalized_value(quick_xml::XmlVersion::Implicit1_0) {
            Ok(v) => v.into_owned(),
            Err(x) => return c.err(format!("bad attribute value: {x}")),
        };
        out.push((k, v));
    }
    Ok(out)
}

/// Parse a project file. Unknown elements and attributes, a wrong `Sdk`, missing required
/// properties, unsafe `Include` paths and an `EngineVersion` other than 1 are errors.
pub fn parse(text: &str) -> Result<Project, ProjectError> {
    let mut reader = Reader::from_str(text);
    let mut c = Ctx { text, pos: 0 };
    let mut stack: Vec<String> = Vec::new();
    let mut cur_text = String::new();

    let mut name = None;
    let mut package = None;
    let mut example: Option<Option<String>> = None;
    let mut start_scene = None;
    let mut engine_version = None;
    let mut scenes = Vec::new();
    let mut mods = Vec::new();
    let mut root_seen = false;
    let mut root_closed = false;

    loop {
        let ev = reader.read_event();
        c.pos = reader.buffer_position() as usize;
        let ev = match ev {
            Ok(e) => e,
            Err(x) => return c.err(format!("not valid XML: {x}")),
        };
        let (start, empty) = match &ev {
            Event::Start(e) => (Some(e), false),
            Event::Empty(e) => (Some(e), true),
            _ => (None, false),
        };
        if let Some(e) = start {
            let n = name_of(e);
            let at = attrs(&c, e)?;
            let depth = stack.len();
            let parent = stack.last().map(String::as_str);
            if root_closed {
                return c.err("content after the root element");
            }
            match (depth, parent, n.as_str()) {
                (0, _, "Project") => {
                    if root_seen {
                        return c.err("second root element");
                    }
                    root_seen = true;
                    let mut sdk = None;
                    for (k, v) in &at {
                        if k == "Sdk" {
                            sdk = Some(v.clone());
                        } else {
                            return c.err(format!("unknown attribute `{k}` on <Project>"));
                        }
                    }
                    match sdk {
                        None => return c.err("<Project> needs Sdk=\"efeengine/1\""),
                        Some(s) if s != SDK => {
                            return c.err(format!("Sdk `{s}` is not supported (this reader needs `{SDK}`)"));
                        }
                        _ => {}
                    }
                }
                (0, _, other) => return c.err(format!("unknown root element <{other}>, expected <Project>")),
                (1, Some("Project"), "PropertyGroup" | "ItemGroup") => {
                    if let Some((k, _)) = at.first() {
                        return c.err(format!("unknown attribute `{k}` on <{n}>"));
                    }
                }
                (2, Some("PropertyGroup"), "Name" | "Package" | "Example" | "StartScene" | "EngineVersion") => {
                    if let Some((k, _)) = at.first() {
                        return c.err(format!("unknown attribute `{k}` on <{n}>"));
                    }
                    cur_text.clear();
                }
                (2, Some("ItemGroup"), "Scene" | "Mod") => {
                    let mut inc = None;
                    for (k, v) in &at {
                        if k == "Include" {
                            inc = Some(v.clone());
                        } else {
                            return c.err(format!("unknown attribute `{k}` on <{n}>"));
                        }
                    }
                    let Some(inc) = inc else {
                        return c.err(format!("<{n}> needs an Include attribute"));
                    };
                    if let Some(p) = path_problem(&inc) {
                        return c.err(format!("<{n} Include=\"{inc}\">: {p}"));
                    }
                    if n == "Scene" { scenes.push(inc) } else { mods.push(inc) }
                }
                _ => return c.err(format!("unknown element <{n}> here")),
            }
            if empty {
                if depth == 2 && parent == Some("PropertyGroup") {
                    finish_property(&c, &n, "", &mut name, &mut package, &mut example, &mut start_scene, &mut engine_version)?;
                }
                if depth == 0 {
                    root_closed = true;
                }
            } else {
                stack.push(n);
            }
            continue;
        }
        match ev {
            Event::End(e) => {
                let n = end_name(&e);
                let top = stack.pop();
                if top.as_deref() != Some(n.as_str()) {
                    return c.err(format!("unexpected closing tag </{n}>"));
                }
                if stack.len() == 2 && stack[1] == "PropertyGroup" {
                    let v = cur_text.trim().to_string();
                    finish_property(&c, &n, &v, &mut name, &mut package, &mut example, &mut start_scene, &mut engine_version)?;
                    cur_text.clear();
                }
                if stack.is_empty() {
                    root_closed = true;
                }
            }
            Event::Text(t) => {
                let s = match t.decode() {
                    Ok(s) => s.into_owned(),
                    Err(x) => return c.err(format!("bad text: {x}")),
                };
                if stack.len() == 3 && stack[1] == "PropertyGroup" {
                    cur_text.push_str(&s);
                } else if !s.trim().is_empty() {
                    return c.err(format!("unexpected text `{}`", s.trim()));
                }
            }
            Event::GeneralRef(r) => {
                let s = match r.resolve_char_ref() {
                    Ok(Some(ch)) => ch.to_string(),
                    _ => match r.decode().as_deref() {
                        Ok("amp") => "&".into(),
                        Ok("lt") => "<".into(),
                        Ok("gt") => ">".into(),
                        Ok("quot") => "\"".into(),
                        Ok("apos") => "'".into(),
                        _ => return c.err("unknown entity"),
                    },
                };
                if stack.len() == 3 && stack[1] == "PropertyGroup" {
                    cur_text.push_str(&s);
                } else {
                    return c.err("unexpected text");
                }
            }
            Event::CData(_) => return c.err("CDATA is not allowed"),
            Event::Eof => break,
            _ => {}
        }
    }
    if !stack.is_empty() {
        return c.err("unexpected end of file (unclosed element)");
    }
    if !root_seen {
        return c.err("no <Project> element");
    }
    let need = |v: Option<String>, what: &str| match v {
        Some(v) => Ok(v),
        None => c.err(format!("missing required property <{what}>")),
    };
    let name = need(name, "Name")?;
    let package = need(package, "Package")?;
    let start_scene = need(start_scene, "StartScene")?;
    let engine_version = match engine_version {
        Some(v) => v,
        None => return c.err("missing required property <EngineVersion>"),
    };
    Ok(Project { name, package, example: example.flatten(), start_scene, engine_version, scenes, mods })
}

#[allow(clippy::too_many_arguments)]
fn finish_property(
    c: &Ctx,
    n: &str,
    v: &str,
    name: &mut Option<String>,
    package: &mut Option<String>,
    example: &mut Option<Option<String>>,
    start_scene: &mut Option<String>,
    engine_version: &mut Option<u32>,
) -> Result<(), ProjectError> {
    let dup = |set: bool| if set { c.err(format!("<{n}> appears twice")) } else { Ok(()) };
    let required = |v: &str| if v.is_empty() { c.err(format!("<{n}> must not be empty")) } else { Ok(()) };
    match n {
        "Name" => {
            dup(name.is_some())?;
            required(v)?;
            *name = Some(v.to_string());
        }
        "Package" => {
            dup(package.is_some())?;
            required(v)?;
            *package = Some(v.to_string());
        }
        "Example" => {
            dup(example.is_some())?;
            *example = Some(if v.is_empty() { None } else { Some(v.to_string()) });
        }
        "StartScene" => {
            dup(start_scene.is_some())?;
            required(v)?;
            if let Some(p) = path_problem(v) {
                return c.err(format!("<StartScene>: {p}"));
            }
            *start_scene = Some(v.to_string());
        }
        "EngineVersion" => {
            dup(engine_version.is_some())?;
            let Ok(num) = v.parse::<u32>() else {
                return c.err(format!("<EngineVersion> `{v}` is not a number"));
            };
            if num != ENGINE_VERSION {
                return c.err(format!("EngineVersion {num} is not supported (this engine is version {ENGINE_VERSION})"));
            }
            *engine_version = Some(num);
        }
        _ => {}
    }
    Ok(())
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Write the canonical text: two-space indentation, fixed order, trailing newline.
pub fn write(p: &Project) -> String {
    let mut s = String::new();
    s.push_str("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
    s.push_str(&format!("<Project Sdk=\"{SDK}\">\n"));
    s.push_str("  <PropertyGroup>\n");
    s.push_str(&format!("    <Name>{}</Name>\n", esc(&p.name)));
    s.push_str(&format!("    <Package>{}</Package>\n", esc(&p.package)));
    s.push_str(&format!("    <Example>{}</Example>\n", esc(p.example.as_deref().unwrap_or(""))));
    s.push_str(&format!("    <StartScene>{}</StartScene>\n", esc(&p.start_scene)));
    s.push_str(&format!("    <EngineVersion>{}</EngineVersion>\n", p.engine_version));
    s.push_str("  </PropertyGroup>\n");
    if !p.scenes.is_empty() || !p.mods.is_empty() {
        s.push_str("\n  <ItemGroup>\n");
        for x in &p.scenes {
            s.push_str(&format!("    <Scene Include=\"{}\" />\n", esc(x)));
        }
        for x in &p.mods {
            s.push_str(&format!("    <Mod Include=\"{}\" />\n", esc(x)));
        }
        s.push_str("  </ItemGroup>\n");
    }
    s.push_str("</Project>\n");
    s
}
