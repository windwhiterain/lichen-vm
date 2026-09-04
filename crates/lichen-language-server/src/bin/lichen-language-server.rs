//! The `lichen-language-server`: an LSP server for Lichen over stdio.
//!
//! It is a thin JSON-RPC/LSP loop over the tooling library. All of the actual
//! editor behavior — span↔position conversion, name resolution, the shared
//! frontend artefacts — lives in `lichen-language-server`'s `lib.rs`, so the
//! same behavior is available to other editors (via the Zed extension importing
//! the same library) without duplicating it here.
//!
//! Run it directly, or install it as the LSP binary that a Zed extension launches:
//!
//! ```text
//! cargo run -p lichen-language-server --bin lichen-language-server
//! ```

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};

use serde_json::{json, Value};

use lichen_language_server::analysis::Doc;
use lichen_language_server::lsp::{
    self, ContentChange, Envelope, TextDocumentIdentifier, TextDocumentItem,
    TextDocumentPositionParams,
};

/// The server state: one `Doc` per open document URI.
struct Server {
    docs: HashMap<String, Doc>,
}

impl Server {
    fn new() -> Self {
        Server {
            docs: HashMap::new(),
        }
    }

    fn publish(&self, out: &mut impl Write, uri: &str) {
        let (version, diagnostics) = match self.docs.get(uri) {
            Some(doc) => (
                None,
                doc.lsp_diagnostics(),
            ),
            None => (None, Vec::new()),
        };
        let msg = lsp::publish_diagnostics(uri, version, diagnostics);
        write_message(out, &msg);
    }

    fn open(&mut self, out: &mut impl Write, item: TextDocumentItem) {
        let doc = Doc::new(item.text);
        self.docs.insert(item.uri.clone(), doc);
        self.publish(out, &item.uri);
    }

    fn change(&mut self, out: &mut impl Write, uri: &str, changes: Vec<ContentChange>) {
        // Full sync: the last change replaces the whole document.
        let Some(last) = changes.into_iter().last() else { return };
        let doc = Doc::new(last.text);
        self.docs.insert(uri.to_string(), doc);
        self.publish(out, uri);
    }

    fn close(&mut self, out: &mut impl Write, uri: &str) {
        self.docs.remove(uri);
        self.publish(out, uri);
    }
}

fn main() {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut reader = BufReader::new(stdin.lock());
    let mut out = stdout.lock();

    let mut server = Server::new();

    while let Some(msg) = read_message(&mut reader) {
        let Ok(envelope) = serde_json::from_value::<Envelope>(msg.clone()) else {
            continue;
        };
        let is_request = envelope.id.is_some();
        let method = envelope.method.clone();

        match method.as_str() {
            "initialize" => {
                let result = json!({
                    "capabilities": {
                        "textDocumentSync": 1, // Full
                        "hoverProvider": true,
                        "definitionProvider": true,
                    }
                });
                respond(&mut out, envelope.id.as_ref(), Some(result));
            }
            "initialized" | "textDocument/didOpen" | "textDocument/didChange"
            | "textDocument/didClose" => {
                // Handled below by method; `initialized` is a no-op notification.
            }
            "shutdown" => {
                respond(&mut out, envelope.id.as_ref(), Some(Value::Null));
            }
            "exit" => break,
            "textDocument/hover" => {
                let params = serde_json::from_value::<TextDocumentPositionParams>(envelope.params.clone());
                let result = params.and_then(|p| {
                    let doc = server.docs.get(&p.textDocument.uri)?;
                    doc.hover_at(p.position).map(|(contents, range)| {
                        json!({ "contents": contents, "range": range })
                    })
                });
                respond(&mut out, envelope.id.as_ref(), result);
            }
            "textDocument/definition" => {
                let params = serde_json::from_value::<TextDocumentPositionParams>(envelope.params.clone());
                let result = params.and_then(|p| {
                    let doc = server.docs.get(&p.textDocument.uri)?;
                    let uri = p.textDocument.uri;
                    doc.definition_at(p.position)
                        .map(|range| json!({ "uri": uri, "range": range }))
                });
                respond(&mut out, envelope.id.as_ref(), result);
            }
            _ => {
                // Unhandled request: reply with `-32601` (Method not found).
                if is_request {
                    respond_error(&mut out, envelope.id.as_ref(), -32601, "Method not found");
                }
            }
        }

        // Text-document notifications (unhandled above by the match arms that no-op).
        match method.as_str() {
            "textDocument/didOpen" => {
                if let Ok(item) =
                    serde_json::from_value::<TextDocumentItem>(envelope.params["textDocument"].clone())
                {
                    server.open(&mut out, item);
                }
            }
            "textDocument/didChange" => {
                let uri = envelope.params["textDocument"]["uri"].as_str().map(str::to_string);
                let changes = serde_json::from_value::<Vec<ContentChange>>(
                    envelope.params["contentChanges"].clone(),
                )
                .unwrap_or_default();
                if let Some(uri) = uri {
                    server.change(&mut out, &uri, changes);
                }
            }
            "textDocument/didClose" => {
                if let Some(uri) = envelope.params["textDocument"]["uri"].as_str() {
                    server.close(&mut out, uri);
                }
            }
            _ => {}
        }
    }
}

fn respond(out: &mut impl Write, id: Option<&Value>, result: Option<Value>) {
    let Some(id) = id else { return };
    let msg = json!({ "jsonrpc": "2.0", "id": id, "result": result });
    write_message(out, &msg);
}

fn respond_error(out: &mut impl Write, id: Option<&Value>, code: i64, message: &str) {
    let Some(id) = id else { return };
    let msg = json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message }
    });
    write_message(out, &msg);
}

/// Read one LSP frame: a `Content-Length` header, an empty line, then the body.
fn read_message(reader: &mut impl BufRead) -> Option<Value> {
    let mut line = String::new();
    let mut content_length: Option<usize> = None;
    loop {
        line.clear();
        let n = reader.read_line(&mut line).ok()?;
        if n == 0 {
            return None; // EOF
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break; // end of headers
        }
        if let Some(v) = trimmed.strip_prefix("Content-Length:") {
            content_length = v.trim().parse().ok();
        }
    }
    let len = content_length?;
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf).ok()?;
    serde_json::from_slice(&buf).ok()
}

/// Write one LSP frame.
fn write_message(out: &mut impl Write, msg: &Value) {
    let body = serde_json::to_string(msg).expect("serialize the message");
    write!(out, "Content-Length: {}\r\n\r\n{}", body.len(), body).expect("write the frame");
    let _ = out.flush();
}
