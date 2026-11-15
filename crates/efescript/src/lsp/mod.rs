//! Language server for `.efa` (E4.4): `efescript lsp [--api FILE.efapi]` over stdio.
//!
//! [`Server`] is the whole protocol state machine: [`Server::handle`] takes one incoming
//! message and returns the messages to send, so it is testable without stdio.
//! [`run_stdio`] only frames messages around it.

mod analysis;
mod text;

use crate::asm::{self, ast::Module, AsmOptions};
use efevm::api::ApiIndex;
use efevm::Op;
use lsp_server::{Message, Notification, Request, RequestId, Response};
use lsp_types as lt;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::time::SystemTime;

const INVALID_REQUEST: i32 = -32600;
const METHOD_NOT_FOUND: i32 = -32601;
const INVALID_PARAMS: i32 = -32602;
const INTERNAL_ERROR: i32 = -32603;
const NOT_INITIALIZED: i32 = -32002;

type Reply = Result<Value, (i32, String)>;

struct Doc {
    text: String,
    version: Option<i32>,
    /// The last AST that parsed (kept while the text has syntax errors).
    module: Option<Module>,
}

#[derive(Default)]
struct ApiState {
    path: Option<PathBuf>,
    stamp: Option<(SystemTime, u64)>,
    index: Option<ApiIndex>,
}

pub struct Server {
    initialized: bool,
    shutdown: bool,
    exited: bool,
    explicit_api: bool,
    watch_dynamic: bool,
    root: Option<PathBuf>,
    docs: HashMap<String, Doc>,
    api: ApiState,
}

fn log(kind: u32, message: impl Into<String>) -> Message {
    Message::Notification(Notification::new(
        "window/logMessage".to_string(),
        json!({"type": kind, "message": message.into()}),
    ))
}

fn invalid(msg: impl Into<String>) -> (i32, String) {
    (INVALID_PARAMS, msg.into())
}

macro_rules! params {
    ($t:ty, $v:expr) => {
        serde_json::from_value::<$t>($v).map_err(|e| invalid(format!("malformed params: {e}")))?
    };
}

/// `file:///a/b%20c` to `/a/b c`.
fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    let b = rest.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() && rest.is_char_boundary(i + 1) && rest.is_char_boundary(i + 3) {
            if let Ok(v) = u8::from_str_radix(&rest[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    Some(PathBuf::from(String::from_utf8_lossy(&out).into_owned()))
}

fn file_name_of(uri: &str) -> String {
    uri.rsplit('/').next().filter(|s| !s.is_empty()).unwrap_or(uri).to_string()
}

impl Server {
    /// `api`: the `--api` file, when given on the command line.
    pub fn new(api: Option<PathBuf>) -> Server {
        Server {
            initialized: false,
            shutdown: false,
            exited: false,
            explicit_api: api.is_some(),
            watch_dynamic: false,
            root: None,
            docs: HashMap::new(),
            api: ApiState { path: api, ..ApiState::default() },
        }
    }

    /// True after the `exit` notification.
    pub fn exited(&self) -> bool {
        self.exited
    }

    /// Process exit code: 0 when `exit` followed `shutdown`, else 1.
    pub fn exit_code(&self) -> i32 {
        if self.shutdown {
            0
        } else {
            1
        }
    }

    /// Whether an API index is loaded.
    pub fn has_index(&self) -> bool {
        self.api.index.is_some()
    }

    /// Handles one incoming message; returns the messages to send (responses, notifications,
    /// requests to the client). Never panics: an internal panic becomes an error response.
    pub fn handle(&mut self, msg: Message) -> Vec<Message> {
        let id = match &msg {
            Message::Request(r) => Some(r.id.clone()),
            _ => None,
        };
        match catch_unwind(AssertUnwindSafe(|| self.handle_inner(msg))) {
            Ok(v) => v,
            Err(_) => match id {
                Some(id) => vec![Message::Response(Response::new_err(id, INTERNAL_ERROR, "internal error".into()))],
                None => vec![log(1, "internal error while handling a notification")],
            },
        }
    }

    fn handle_inner(&mut self, msg: Message) -> Vec<Message> {
        let mut out = Vec::new();
        if self.exited {
            return out;
        }
        match msg {
            Message::Request(req) => {
                if self.initialized {
                    self.refresh(&mut out);
                }
                self.request(req, &mut out);
            }
            Message::Notification(n) => {
                if self.initialized && n.method != "exit" && n.method != "workspace/didChangeWatchedFiles" {
                    self.refresh(&mut out);
                }
                self.notification(n, &mut out);
            }
            Message::Response(_) => {}
        }
        out
    }

    // ---- the API index ----

    /// Reloads the index when its file changed (or `force`). True when a new index was loaded.
    fn reload(&mut self, force: bool, out: &mut Vec<Message>) -> bool {
        let Some(path) = self.api.path.clone() else { return false };
        let stamp = std::fs::metadata(&path).ok().and_then(|m| Some((m.modified().ok()?, m.len())));
        if !force && stamp == self.api.stamp {
            return false;
        }
        self.api.stamp = stamp;
        let loaded = std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|t| ApiIndex::from_json(&t).map_err(|e| e.to_string()));
        match loaded {
            Ok(ix) => {
                out.push(log(3, format!("efescript: loaded API index {}", path.display())));
                self.api.index = Some(ix);
                true
            }
            Err(e) => {
                out.push(log(1, format!("efescript: cannot load API index {}: {e}", path.display())));
                false
            }
        }
    }

    fn refresh(&mut self, out: &mut Vec<Message>) {
        if self.reload(false, out) {
            let all = self.publish_all();
            out.extend(all);
        }
    }

    /// Looks for a single `*.efapi` in the workspace root (not recursive).
    fn discover(&mut self, out: &mut Vec<Message>) {
        let Some(root) = self.root.clone() else { return };
        let mut found: Vec<PathBuf> = match std::fs::read_dir(&root) {
            Ok(rd) => rd
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|x| x == "efapi") && p.is_file())
                .collect(),
            Err(_) => Vec::new(),
        };
        found.sort();
        match found.len() {
            1 => self.api.path = found.pop(),
            0 => out.push(log(3, "efescript: no .efapi file in the workspace root; running without an API index")),
            n => out.push(log(
                2,
                format!("efescript: {n} .efapi files in the workspace root; running without an API index (use --api)"),
            )),
        }
    }

    // ---- requests ----

    fn request(&mut self, req: Request, out: &mut Vec<Message>) {
        let Request { id, method, params } = req;
        let reply: Reply = if method == "initialize" {
            self.initialize(&params, out)
        } else if !self.initialized {
            Err((NOT_INITIALIZED, "server not initialized".into()))
        } else if self.shutdown && method != "shutdown" {
            Err((INVALID_REQUEST, "server is shutting down".into()))
        } else {
            match method.as_str() {
                "shutdown" => {
                    self.shutdown = true;
                    Ok(Value::Null)
                }
                "textDocument/hover" => self.hover(params),
                "textDocument/definition" => self.definition(params),
                "textDocument/completion" => self.completion(params),
                _ => Err((METHOD_NOT_FOUND, format!("unsupported request `{method}`"))),
            }
        };
        out.push(Message::Response(match reply {
            Ok(v) => Response::new_ok(id, v),
            Err((code, msg)) => Response::new_err(id, code, msg),
        }));
    }

    fn initialize(&mut self, p: &Value, out: &mut Vec<Message>) -> Reply {
        if self.initialized {
            return Err((INVALID_REQUEST, "already initialized".into()));
        }
        let root_uri = p
            .pointer("/workspaceFolders/0/uri")
            .or_else(|| p.get("rootUri"))
            .and_then(Value::as_str)
            .and_then(uri_to_path);
        self.root = root_uri.or_else(|| p.get("rootPath").and_then(Value::as_str).map(PathBuf::from));
        self.watch_dynamic =
            p.pointer("/capabilities/workspace/didChangeWatchedFiles/dynamicRegistration").and_then(Value::as_bool) == Some(true);
        self.initialized = true;
        if !self.explicit_api {
            self.discover(out);
        }
        self.reload(true, out);
        Ok(json!({
            "capabilities": {
                "textDocumentSync": {"openClose": true, "change": 1, "save": {"includeText": false}},
                "definitionProvider": true,
                "hoverProvider": true,
                "completionProvider": {"triggerCharacters": ["."]}
            },
            "serverInfo": {"name": "efescript-lsp", "version": env!("CARGO_PKG_VERSION")}
        }))
    }

    fn doc(&self, uri: &str) -> Result<&Doc, (i32, String)> {
        self.docs.get(uri).ok_or_else(|| invalid(format!("unknown document {uri}")))
    }

    fn hover(&self, v: Value) -> Reply {
        let p = params!(lt::HoverParams, v);
        let tp = p.text_document_position_params;
        let doc = self.doc(tp.text_document.uri.as_str())?;
        let lines = text::lines(&doc.text);
        let Some(line) = lines.get(tp.position.line as usize) else { return Ok(Value::Null) };
        let Some((s, e, word)) = text::word_at(line, text::char_col(line, tp.position.character)) else {
            return Ok(Value::Null);
        };
        let Some(md) = analysis::hover_text(self.api.index.as_ref(), &word) else { return Ok(Value::Null) };
        let l = tp.position.line;
        Ok(json!({
            "contents": {"kind": "markdown", "value": md},
            "range": text::range_json(l, text::utf16_col(line, s), l, text::utf16_col(line, e))
        }))
    }

    fn definition(&self, v: Value) -> Reply {
        let p = params!(lt::GotoDefinitionParams, v);
        let tp = p.text_document_position_params;
        let uri = tp.text_document.uri.as_str();
        let doc = self.doc(uri)?;
        let lines = text::lines(&doc.text);
        let Some(line) = lines.get(tp.position.line as usize) else { return Ok(Value::Null) };
        let Some((_, _, word)) = text::word_at(line, text::char_col(line, tp.position.character)) else {
            return Ok(Value::Null);
        };
        let Some(m) = &doc.module else { return Ok(Value::Null) };
        match analysis::definition(m, tp.position.line + 1, &word) {
            Some(span) => Ok(json!({"uri": uri, "range": text::span_range(&lines, span)})),
            None => Ok(Value::Null),
        }
    }

    fn completion(&self, v: Value) -> Reply {
        let p = params!(lt::CompletionParams, v);
        let tp = p.text_document_position;
        let doc = self.doc(tp.text_document.uri.as_str())?;
        let lines = text::lines(&doc.text);
        let l = tp.position.line;
        let line = lines.get(l as usize).copied().unwrap_or("");
        let cc = text::char_col(line, tp.position.character);
        let Some((start, _prefix)) = text::prefix_at(line, cc) else { return Ok(json!([])) };
        let range = text::range_json(l, text::utf16_col(line, start), l, text::utf16_col(line, cc));
        let mut items: Vec<Value> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        let mut add = |label: String, kind: u32, detail: String, doc: String| {
            if !seen.insert(label.clone()) {
                return;
            }
            let mut it = json!({
                "label": label, "kind": kind, "detail": detail, "filterText": label,
                "textEdit": {"range": range.clone(), "newText": label}
            });
            if !doc.is_empty() {
                it["documentation"] = json!({"kind": "markdown", "value": doc});
            }
            items.push(it);
        };
        if let Some(ix) = &self.api.index {
            for f in &ix.functions {
                add(f.qualified(), 3, f.signature_text(), f.doc.clone());
            }
            for (label, detail, d) in analysis::struct_constants(ix) {
                add(label, 21, detail, d);
            }
        }
        if let Some(m) = &doc.module {
            for (label, kind, detail) in analysis::local_symbols(m, l + 1) {
                add(label, kind, detail, String::new());
            }
        }
        for op in Op::ALL {
            add(op.name().to_string(), 14, op.shape().operands().to_string(), String::new());
        }
        Ok(Value::Array(items))
    }

    // ---- notifications ----

    fn notification(&mut self, n: Notification, out: &mut Vec<Message>) {
        let Notification { method, params: p } = n;
        match method.as_str() {
            "exit" => self.exited = true,
            "initialized" => {
                if self.watch_dynamic {
                    out.push(Message::Request(Request::new(
                        RequestId::from("efescript-watch".to_string()),
                        "client/registerCapability".to_string(),
                        json!({"registrations": [{
                            "id": "efapi-watch",
                            "method": "workspace/didChangeWatchedFiles",
                            "registerOptions": {"watchers": [{"globPattern": "**/*.efapi"}]}
                        }]}),
                    )));
                }
            }
            _ if !self.initialized => {}
            "textDocument/didOpen" => match serde_json::from_value::<lt::DidOpenTextDocumentParams>(p) {
                Ok(p) => {
                    let d = p.text_document;
                    let uri = d.uri.as_str().to_string();
                    self.docs.insert(uri.clone(), Doc { text: d.text, version: Some(d.version), module: None });
                    self.update_doc(&uri, out);
                }
                Err(e) => out.push(log(1, format!("malformed didOpen: {e}"))),
            },
            "textDocument/didChange" => match serde_json::from_value::<lt::DidChangeTextDocumentParams>(p) {
                Ok(p) => {
                    let uri = p.text_document.uri.as_str().to_string();
                    if let Some(doc) = self.docs.get_mut(&uri) {
                        doc.version = Some(p.text_document.version);
                        for c in p.content_changes {
                            match c.range {
                                None => doc.text = c.text,
                                Some(r) => text::apply_edit(
                                    &mut doc.text,
                                    (r.start.line, r.start.character),
                                    (r.end.line, r.end.character),
                                    &c.text,
                                ),
                            }
                        }
                        self.update_doc(&uri, out);
                    }
                }
                Err(e) => out.push(log(1, format!("malformed didChange: {e}"))),
            },
            "textDocument/didSave" => {
                if let Ok(p) = serde_json::from_value::<lt::DidSaveTextDocumentParams>(p) {
                    let uri = p.text_document.uri.as_str().to_string();
                    if let Some(t) = p.text {
                        if let Some(d) = self.docs.get_mut(&uri) {
                            d.text = t;
                        }
                    }
                    if self.docs.contains_key(&uri) {
                        self.update_doc(&uri, out);
                    }
                }
            }
            "textDocument/didClose" => {
                if let Ok(p) = serde_json::from_value::<lt::DidCloseTextDocumentParams>(p) {
                    let uri = p.text_document.uri.as_str().to_string();
                    if self.docs.remove(&uri).is_some() {
                        out.push(publish(&uri, None, Vec::new()));
                    }
                }
            }
            "workspace/didChangeWatchedFiles" => {
                if !self.explicit_api && self.api.path.is_none() {
                    self.discover(out);
                }
                if self.reload(true, out) {
                    let all = self.publish_all();
                    out.extend(all);
                }
            }
            _ => {}
        }
    }

    // ---- diagnostics ----

    fn update_doc(&mut self, uri: &str, out: &mut Vec<Message>) {
        if let Some(doc) = self.docs.get_mut(uri) {
            if let Ok(m) = asm::parse(&doc.text, "") {
                doc.module = Some(m);
            }
        }
        if let Some(m) = self.publish_one(uri) {
            out.push(m);
        }
    }

    fn publish_one(&self, uri: &str) -> Option<Message> {
        let doc = self.docs.get(uri)?;
        let name = file_name_of(uri);
        let opts = AsmOptions { api: self.api.index.as_ref() };
        let lines = text::lines(&doc.text);
        let diags: Vec<Value> = match asm::assemble_with(&doc.text, &name, &opts) {
            Ok(_) => Vec::new(),
            Err(ds) => ds
                .0
                .iter()
                .map(|d| {
                    let mut msg = d.message.clone();
                    if let Some(n) = &d.note {
                        msg.push_str("\nnote: ");
                        msg.push_str(n);
                    }
                    json!({
                        "range": text::span_range(&lines, d.span()),
                        "severity": 1,
                        "source": "efescript",
                        "message": msg
                    })
                })
                .collect(),
        };
        Some(publish(uri, doc.version, diags))
    }

    fn publish_all(&self) -> Vec<Message> {
        let mut uris: Vec<&String> = self.docs.keys().collect();
        uris.sort();
        uris.into_iter().filter_map(|u| self.publish_one(u)).collect()
    }
}

fn publish(uri: &str, version: Option<i32>, diags: Vec<Value>) -> Message {
    let mut p = json!({"uri": uri, "diagnostics": diags});
    if let Some(v) = version {
        p["version"] = json!(v);
    }
    Message::Notification(Notification::new("textDocument/publishDiagnostics".to_string(), p))
}

/// Serves LSP on stdin/stdout and returns the process exit code (0 after `shutdown` + `exit`).
pub fn run_stdio_code(api: Option<PathBuf>) -> i32 {
    use std::io::Write;
    let mut server = Server::new(api);
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let stdout = std::io::stdout();
    while !server.exited() {
        let msg = match Message::read(&mut input) {
            Ok(Some(m)) => m,
            Ok(None) => break,
            Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
                eprintln!("efescript lsp: {e}");
                continue;
            }
            Err(_) => break,
        };
        for reply in server.handle(msg) {
            let mut o = stdout.lock();
            if reply.write(&mut o).is_err() || o.flush().is_err() {
                return 1;
            }
        }
    }
    server.exit_code()
}

/// [`run_stdio_code`] as an [`std::process::ExitCode`].
pub fn run_stdio(api: Option<PathBuf>) -> std::process::ExitCode {
    std::process::ExitCode::from(run_stdio_code(api) as u8)
}
