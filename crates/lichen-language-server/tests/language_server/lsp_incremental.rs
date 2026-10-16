//! The incremental path: the frontend runs once per document *text*, not
//! per request (`docs/notes/code-audit.md` P1-17).

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use lichen_language_server::lsp_types::Url;

/// Silence before a test concludes nothing more is coming.
/// Past the 150 ms debounce, so a burst reads as one publish.
const QUIET: Duration = Duration::from_millis(1200);

fn frame(json: &str) -> Vec<u8> {
    let body = json.as_bytes();
    let mut out = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    out.extend_from_slice(body);
    out
}

/// Read one LSP stdio frame, or `None` at end of stream.
fn read_frame(reader: &mut impl BufRead) -> Option<String> {
    let mut header = Vec::new();
    loop {
        let mut byte = [0u8; 1];
        match reader.read(&mut byte) {
            Ok(0) | Err(_) => return None,
            Ok(_) => {}
        }
        header.push(byte[0]);
        if header.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let header = String::from_utf8(header).ok()?;
    let len: usize = header.lines().find_map(|line| {
        line.trim()
            .strip_prefix("Content-Length:")
            .and_then(|value| value.trim().parse().ok())
    })?;
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body).ok()?;
    String::from_utf8(body).ok()
}

fn json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

struct Server {
    stdin: ChildStdin,
    messages: Receiver<String>,
    child: Child,
}

impl Server {
    /// Spawn the real binary and complete the handshake.
    fn start() -> Server {
        let mut child = Command::new(env!("CARGO_BIN_EXE_lichen-language-server"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn lichen-language-server");
        let stdin = child.stdin.take().expect("stdin");
        let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));
        let (tx, messages) = channel();
        thread::spawn(move || {
            while let Some(msg) = read_frame(&mut stdout) {
                if tx.send(msg).is_err() {
                    break;
                }
            }
        });
        let mut server = Server {
            stdin,
            messages,
            child,
        };
        server.send(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"processId":null,"rootUri":null,"capabilities":{}}}"#,
        );
        server.response(1);
        server.send(r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#);
        server
    }

    fn send(&mut self, json: &str) {
        self.stdin.write_all(&frame(json)).expect("write frame");
        self.stdin.flush().expect("flush");
    }

    /// The response carrying `id`, reading past any notifications before it.
    fn response(&mut self, id: u32) -> String {
        let needle = format!("\"id\":{id}");
        loop {
            let msg = self
                .messages
                .recv_timeout(Duration::from_secs(60))
                .unwrap_or_else(|_| panic!("no response to request {id}"));
            if msg.contains(&needle) {
                return msg;
            }
        }
    }

    fn request(&mut self, id: u32, json: &str) -> String {
        self.send(json);
        self.response(id)
    }

    /// The next `publishDiagnostics`, reading past anything else.
    fn next_publish(&mut self) -> String {
        loop {
            let msg = self
                .messages
                .recv_timeout(Duration::from_secs(60))
                .unwrap_or_else(|_| panic!("no diagnostics were published"));
            if msg.contains("publishDiagnostics") {
                return msg;
            }
        }
    }

    /// Every `publishDiagnostics` until the server is quiet for
    /// `quiet`, so "no more" is asserted rather than hoped.
    fn publishes(&mut self, quiet: Duration) -> Vec<String> {
        let mut out = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.messages.recv_timeout(quiet.min(left)) {
                Ok(msg) => {
                    if msg.contains("publishDiagnostics") {
                        out.push(msg);
                    }
                }
                Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => break,
            }
        }
        out
    }

    fn shutdown(mut self) {
        self.send(r#"{"jsonrpc":"2.0","id":900,"method":"shutdown","params":null}"#);
        self.response(900);
        self.send(r#"{"jsonrpc":"2.0","method":"exit","params":null}"#);
        drop(self.stdin);
        self.child.wait().expect("the server exits cleanly");
    }
}

fn did_open(uri: &str, text: &str) -> String {
    format!(
        r#"{{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{{"textDocument":{{"uri":"{uri}","languageId":"lichen","version":1,"text":{}}}}}}}"#,
        json_string(text)
    )
}

fn did_change(uri: &str, version: u32, text: &str) -> String {
    format!(
        r#"{{"jsonrpc":"2.0","method":"textDocument/didChange","params":{{"textDocument":{{"uri":"{uri}","version":{version}}},"contentChanges":[{{"text":{}}}]}}}}"#,
        json_string(text)
    )
}

fn did_close(uri: &str) -> String {
    format!(
        r#"{{"jsonrpc":"2.0","method":"textDocument/didClose","params":{{"textDocument":{{"uri":"{uri}"}}}}}}"#
    )
}

fn completion(uri: &str, id: u32, line: u32, character: u32) -> String {
    format!(
        r#"{{"jsonrpc":"2.0","id":{id},"method":"textDocument/completion","params":{{"textDocument":{{"uri":"{uri}"}},"position":{{"line":{line},"character":{character}}},"context":{{"triggerKind":1}}}}}}"#
    )
}

fn goto_definition(uri: &str, id: u32, line: u32, character: u32) -> String {
    format!(
        r#"{{"jsonrpc":"2.0","id":{id},"method":"textDocument/definition","params":{{"textDocument":{{"uri":"{uri}"}},"position":{{"line":{line},"character":{character}}}}}}}"#
    )
}

/// A burst of keystrokes must collapse to one frontend run, and the published
/// diagnostics must be the *last* edit's.
///
/// # Invariant
/// Only the last of the six edits has an error, so "which edit was analyzed" is
/// observable rather than assumed.
#[test]
fn a_burst_of_edits_runs_the_frontend_once() {
    let mut server = Server::start();
    let uri = "file:///burst.lichen";
    server.send(&did_open(uri, "a = 1\na\n"));
    let _opened = server.next_publish();

    for version in 2..=6 {
        let text = format!("a = 1\nb{version} = a + 1\nb{version}\n");
        server.send(&did_change(uri, version, &text));
    }
    server.send(&did_change(uri, 7, "a = 1\nb = unknown_name\nb\n"));

    let publishes = server.publishes(QUIET);
    assert_eq!(
        publishes.len(),
        1,
        "six edits inside the debounce window must run the frontend once, got {} publish(es): {publishes:?}",
        publishes.len()
    );
    assert!(
        publishes[0].contains("unresolved name"),
        "the published set must be the last edit's, not an earlier one's: {}",
        publishes[0]
    );
    server.shutdown();
}

/// A dependency is half of the cache key: the index is keyed by the document's
/// text *and* every imported file's bytes.
///
/// # Invariant
/// The cursor sits right after the dot, so the module's exported fields are
/// offered unfiltered and a newly exported one is visible.
#[test]
fn an_edit_to_an_imported_file_refreshes_the_answer() {
    let dir = temp_dir("dep");
    write(&dir, "math.lichen", "succ = x => x + 1\n");
    let main = write(
        &dir,
        "main.lichen",
        "---\n  math = import \"math.lichen\"\n---\nmath.succ 41\n",
    );
    let uri = Url::from_file_path(&main).unwrap().to_string();

    let mut server = Server::start();
    let text = fs::read_to_string(&main).unwrap();
    server.send(&did_open(&uri, &text));
    let diagnostics = server.next_publish();
    assert!(
        !diagnostics.contains("cannot load package"),
        "the relative import should resolve, got {diagnostics}"
    );

    let before = server.request(2, &completion(&uri, 2, 3, 5));
    assert!(
        before.contains("\"label\":\"succ\""),
        "the module's own field should be offered, got {before}"
    );
    assert!(
        !before.contains("\"label\":\"extra\""),
        "`extra` is not exported yet, got {before}"
    );

    // Only the import changes; the document the editor holds is untouched.
    write(&dir, "math.lichen", "succ = x => x + 1\nextra = 7\n");

    let after = server.request(3, &completion(&uri, 3, 3, 5));
    assert!(
        after.contains("\"label\":\"succ\""),
        "the existing field is still offered, got {after}"
    );
    assert!(
        after.contains("\"label\":\"extra\""),
        "the cached analysis was keyed without the import's bytes, so the newly \
         exported field is missing: {after}"
    );
    server.shutdown();
}

/// A jump to a **prelude** name lands in the built-in's own file.
///
/// # Invariant
/// A failure inside the built-in is published against it, never the document
/// (`docs/notes/core-prelude.md` §4), and only the persistent store
/// materializes `core.lichen` — so the jumped-to path must be one
/// `Url::from_file_path` can open.
#[test]
fn a_prelude_name_jumps_into_the_materialized_builtin_file() {
    let dir = temp_dir("prelude");
    let main = write(&dir, "main.lichen", "add [\"a\", \"b\"]\n");
    let uri = Url::from_file_path(&main).unwrap().to_string();

    let mut server = Server::start();
    let text = fs::read_to_string(&main).unwrap();
    server.send(&did_open(&uri, &text));

    // The document's own set is published against the document: the built-in's
    // failure must not appear on it.
    let document = server.next_publish();
    assert!(
        document.contains(&uri),
        "the first publish is the document's, got {document}"
    );
    assert!(
        !document.contains("assertion failed"),
        "the built-in's failure must not be published against the document: {document}"
    );

    // The built-in's failure names the file the store materialized, not the document.
    let builtin = server.next_publish();
    assert!(
        builtin.contains("core.lichen"),
        "the built-in's failure is published against its own file, got {builtin}"
    );
    assert!(
        builtin.contains("assertion failed"),
        "the failure inside the built-in should be reported there, got {builtin}"
    );

    // Go to definition on the prelude's `add`: a Location in the built-in's
    // file, not a position in the document.
    let definition = server.request(2, &goto_definition(&uri, 2, 0, 1));
    assert!(
        definition.contains("core.lichen"),
        "the jump should name the built-in's file, got {definition}"
    );
    assert!(
        !definition.contains(&format!("\"uri\":\"{uri}\"")),
        "the jump must not answer with the document's own URI, got {definition}"
    );
    server.shutdown();
}

/// Closing a document supersedes its analyses.
/// The close clears diagnostics; the interrupted edit publishes none.
#[test]
fn a_closed_document_publishes_nothing_further() {
    let mut server = Server::start();
    let uri = "file:///closing.lichen";
    server.send(&did_open(uri, "a = 1\na\n"));
    let _opened = server.next_publish();

    server.send(&did_change(uri, 2, "a = 1\nb = unknown_name\nb\n"));
    server.send(&did_close(uri));

    let publishes = server.publishes(QUIET);
    assert!(
        !publishes.is_empty(),
        "the close should clear the document's diagnostics"
    );
    for publish in &publishes {
        assert!(
            publish.contains("\"diagnostics\":[]"),
            "only the close's empty set may be published, got {publish}"
        );
    }
    server.shutdown();
}

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
