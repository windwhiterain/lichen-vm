//! End-to-end smoke test: spawn the real `lichen-language-server` binary and
//! drive it over stdio as an LSP client would.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use lichen_language_server::lsp_types::Url;

/// Encode one JSON-RPC message as an LSP stdio frame.
fn frame(json: &str) -> Vec<u8> {
    let body = json.as_bytes();
    let mut out = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    out.extend_from_slice(body);
    out
}

/// Read one LSP stdio frame back off the server's stdout.
fn read_frame(reader: &mut impl BufRead) -> String {
    let mut header = Vec::new();
    loop {
        let mut b = [0u8; 1];
        let n = reader.read(&mut b).expect("read header byte");
        if n == 0 {
            panic!("eof while reading frame header");
        }
        header.push(b[0]);
        if header.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let header = String::from_utf8(header).expect("header is utf-8");
    let len: usize = header
        .lines()
        .find_map(|l| {
            l.trim()
                .strip_prefix("Content-Length:")
                .and_then(|v| v.trim().parse().ok())
        })
        .expect("Content-Length header present");
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body).expect("read frame body");
    String::from_utf8(body).expect("body is utf-8")
}

fn send(stdin: &mut impl Write, json: &str) {
    stdin.write_all(&frame(json)).expect("write frame");
    stdin.flush().expect("flush");
}

/// Read the next **response**, skipping the server's notifications.
///
/// # Invariant
/// The server interleaves its own notifications (`publishDiagnostics`, the
/// `lichen/analysis` telemetry a compile emits), so a frame that is not a
/// response must be skipped rather than taken as the answer.
fn read_response(reader: &mut impl BufRead) -> String {
    for _ in 0..64 {
        let msg = read_frame(reader);
        if msg.contains("\"id\":") {
            return msg;
        }
    }
    panic!("never observed a response frame");
}

fn wait_for(reader: &mut impl BufRead, needle: &str) -> String {
    for _ in 0..64 {
        let msg = read_frame(reader);
        if msg.contains(needle) {
            return msg;
        }
    }
    panic!("never observed {needle:?} in server output");
}

#[test]
fn handshake_and_features() {
    let mut child: Child = Command::new(env!("CARGO_BIN_EXE_lichen-language-server"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn lichen-language-server");
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());

    // initialize
    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"processId":null,"rootUri":null,"capabilities":{}}}"#,
    );
    let init = read_response(&mut stdout);
    assert!(init.contains("\"id\":1"), "initialize resp = {init}");
    assert!(
        init.contains("\"capabilities\""),
        "initialize resp = {init}"
    );
    assert!(
        init.contains("semanticTokensProvider"),
        "initialize resp = {init}"
    );
    assert!(
        init.contains("completionProvider"),
        "initialize resp = {init}"
    );

    // initialized (notification), then open a document that has an unresolved name.
    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#,
    );
    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///test.lichen","languageId":"lichen","version":1,"text":"a = 1\nb = a + unknown\nb\n"}}}"#,
    );
    let diag = wait_for(&mut stdout, "publishDiagnostics");
    assert!(diag.contains("unresolved name"), "diagnostics = {diag}");

    // hover on the `a` use (line 1, char 4): the binding hover shows
    // the bound expr's `value : type` (`a = 1` → `1 : Int`).
    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":2,"method":"textDocument/hover","params":{"textDocument":{"uri":"file:///test.lichen"},"position":{"line":1,"character":4}}}"#,
    );
    let hover = read_response(&mut stdout);
    assert!(hover.contains("\"id\":2"), "hover resp = {hover}");
    assert!(hover.contains("1 : Int"), "hover resp = {hover}");

    // go-to-definition on the final `b` (line 2, character 0).
    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":3,"method":"textDocument/definition","params":{"textDocument":{"uri":"file:///test.lichen"},"position":{"line":2,"character":0}}}"#,
    );
    let definition = read_response(&mut stdout);
    assert!(
        definition.contains("\"id\":3"),
        "definition resp = {definition}"
    );
    assert!(
        definition.contains("file:///test.lichen"),
        "definition resp = {definition}"
    );

    // completion on the `a` use (line 1, char 4): the in-scope name `a` is
    // offered, filtered to the typed prefix.
    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":4,"method":"textDocument/completion","params":{"textDocument":{"uri":"file:///test.lichen"},"position":{"line":1,"character":4}}}"#,
    );
    let completion = read_response(&mut stdout);
    assert!(
        completion.contains("\"id\":4"),
        "completion resp = {completion}"
    );
    assert!(
        completion.contains("\"label\":\"a\""),
        "completion resp = {completion}"
    );

    // semanticTokens/full — Lichen's own parser drives the highlight payload.
    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":5,"method":"textDocument/semanticTokens/full","params":{"textDocument":{"uri":"file:///test.lichen"}}}"#,
    );
    let tokens = read_response(&mut stdout);
    assert!(
        tokens.contains("\"id\":5"),
        "semanticTokens resp = {tokens}"
    );
    assert!(
        tokens.contains("\"data\""),
        "semanticTokens resp = {tokens}"
    );

    // shutdown, then exit.
    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":6,"method":"shutdown","params":null}"#,
    );
    let shutdown = read_response(&mut stdout);
    assert!(shutdown.contains("\"id\":6"), "shutdown resp = {shutdown}");
    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","method":"exit","params":null}"#,
    );
    drop(stdin);
    child.wait().expect("server exits cleanly");
}

#[test]
fn field_completion_after_a_dot() {
    // Typing `point.s` must offer the struct field `sub`, through the real
    // `textDocument/completion` handler.
    let mut child: Child = Command::new(env!("CARGO_BIN_EXE_lichen-language-server"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn lichen-language-server");
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());

    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"processId":null,"rootUri":null,"capabilities":{}}}"#,
    );
    let _init = read_response(&mut stdout);
    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#,
    );
    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///field.lichen","languageId":"lichen","version":1,"text":"point = { x = 1, y = 2, sub = 3 }\npoint.s\n"}}}"#,
    );
    let _diag = wait_for(&mut stdout, "publishDiagnostics");

    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":2,"method":"textDocument/completion","params":{"textDocument":{"uri":"file:///field.lichen"},"position":{"line":1,"character":7}}}"#,
    );
    let completion = read_response(&mut stdout);
    assert!(
        completion.contains("\"id\":2"),
        "completion resp = {completion}"
    );
    assert!(
        completion.contains("\"label\":\"sub\""),
        "completion resp = {completion}"
    );

    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":3,"method":"shutdown","params":null}"#,
    );
    let _shutdown = read_response(&mut stdout);
    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":4,"method":"exit","params":null}"#,
    );
    drop(stdin);
    child.wait().expect("server exits cleanly");
}

/// A fresh temporary directory so a test's import files are isolated on disk.
fn temp_dir(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir =
        std::env::temp_dir().join(format!("lichen-lsp-{name}-{}-{nonce}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(dir: &Path, name: &str, contents: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, contents).unwrap();
    path
}

#[test]
fn relative_imports_resolve_via_the_document_uri() {
    // A relative `@import` must resolve against the document's own directory
    // (from its `file://` URI), not the process CWD.
    let dir = temp_dir("import");
    write(&dir, "math.lichen", "x => x + 1\n");
    let main_path = write(
        &dir,
        "main.lichen",
        "---\n  math = import \"math.lichen\"\n---\nmath.succ 41\n",
    );
    let uri = Url::from_file_path(&main_path).unwrap();

    let mut child: Child = Command::new(env!("CARGO_BIN_EXE_lichen-language-server"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn lichen-language-server");
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());

    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"processId":null,"rootUri":null,"capabilities":{}}}"#,
    );
    let _init = read_response(&mut stdout);
    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#,
    );

    let text = fs::read_to_string(&main_path).unwrap();
    let did_open = format!(
        r#"{{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{{"textDocument":{{"uri":"{}","languageId":"lichen","version":1,"text":{}}}}}}}"#,
        uri.as_str(),
        serde_json::to_string(&text).unwrap(),
    );
    send(&mut stdin, &did_open);

    let diag = wait_for(&mut stdout, "publishDiagnostics");
    assert!(
        !diag.contains("cannot load package"),
        "relative import should resolve; got diagnostics = {diag}"
    );

    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":2,"method":"shutdown","params":null}"#,
    );
    let _shutdown = read_response(&mut stdout);
    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","method":"exit","params":null}"#,
    );
    drop(stdin);
    child.wait().expect("server exits cleanly");
}

#[test]
fn the_repo_import_example_loads() {
    // `import/_.lichen` has a transitive relative import: every import
    // must resolve from the file's directory, not the CWD.
    let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/import");
    let main_path = examples.join("_.lichen");
    let uri = Url::from_file_path(&main_path).unwrap();

    let mut child: Child = Command::new(env!("CARGO_BIN_EXE_lichen-language-server"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn lichen-language-server");
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());

    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"processId":null,"rootUri":null,"capabilities":{}}}"#,
    );
    let _init = read_response(&mut stdout);
    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#,
    );

    let text = fs::read_to_string(&main_path).unwrap();
    let did_open = format!(
        r#"{{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{{"textDocument":{{"uri":"{}","languageId":"lichen","version":1,"text":{}}}}}}}"#,
        uri.as_str(),
        serde_json::to_string(&text).unwrap(),
    );
    send(&mut stdin, &did_open);

    let diag = wait_for(&mut stdout, "publishDiagnostics");
    assert!(
        !diag.contains("cannot load package"),
        "the repo import example's relative imports should resolve; got {diag}"
    );

    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":2,"method":"shutdown","params":null}"#,
    );
    let _shutdown = read_response(&mut stdout);
    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","method":"exit","params":null}"#,
    );
    drop(stdin);
    child.wait().expect("server exits cleanly");
}
