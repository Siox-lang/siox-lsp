//! Spawn the real binary, exercise JSON-RPC framing and compiler-backed results.

use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Read, Write},
    path::PathBuf,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver},
    time::Duration,
};
use url::Url;

fn std_root() -> PathBuf {
    std::env::var_os("SIOX_STD")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../siox/std"))
}

struct Client {
    child: Child,
    input: ChildStdin,
    output: Receiver<Value>,
    next: i64,
}

impl Client {
    fn new() -> Self {
        Self::with_std(std_root())
    }

    fn with_std(std: PathBuf) -> Self {
        assert!(
            std.join("prelude.siox").is_file() || std.join("std/prelude.siox").is_file(),
            "set SIOX_STD to the matching compiler std directory"
        );
        let mut child = Command::new(env!("CARGO_BIN_EXE_siox-lsp"))
            .args(["--stdio", "--std"])
            .arg(std)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (send, output) = mpsc::channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut length = None;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap() == 0 {
                        return;
                    }
                    if line == "\r\n" {
                        break;
                    }
                    if let Some(value) = line.strip_prefix("Content-Length:") {
                        length = Some(value.trim().parse::<usize>().unwrap());
                    }
                }
                let mut bytes = vec![0; length.expect("protocol-only stdout")];
                reader.read_exact(&mut bytes).unwrap();
                if send.send(serde_json::from_slice(&bytes).unwrap()).is_err() {
                    return;
                }
            }
        });
        Self {
            child,
            input,
            output,
            next: 0,
        }
    }

    fn raw(&mut self, data: &[u8]) {
        write!(self.input, "Content-Length: {}\r\n\r\n", data.len()).unwrap();
        self.input.write_all(data).unwrap();
        self.input.flush().unwrap();
    }
    fn send(&mut self, message: Value) {
        self.raw(&serde_json::to_vec(&message).unwrap());
    }
    fn receive(&self) -> Value {
        self.output
            .recv_timeout(Duration::from_secs(30))
            .expect("server did not respond")
    }
    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let id = self.next;
        self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}));
        loop {
            let message = self.receive();
            if message["id"] == id {
                return message;
            }
        }
    }
    fn initialize(&mut self) -> Value {
        let response = self.request(
            "initialize",
            json!({"processId":null,"rootUri":null,"capabilities":{"textDocument":{"documentSymbol":{"hierarchicalDocumentSymbolSupport":true}}}}),
        );
        assert!(response.get("error").is_none(), "{response}");
        self.notify("initialized", json!({}));
        response["result"]["capabilities"].clone()
    }
    fn notify(&mut self, method: &str, params: Value) {
        self.send(json!({"jsonrpc":"2.0","method":method,"params":params}));
    }
    fn diagnostics(&self, uri: &str) -> Value {
        loop {
            let message = self.receive();
            if message["method"] == "textDocument/publishDiagnostics"
                && message["params"]["uri"] == uri
            {
                return message["params"].clone();
            }
        }
    }
    fn open(&mut self, uri: &str, source: &str) -> Value {
        self.notify(
            "textDocument/didOpen",
            json!({"textDocument":{"uri":uri,"languageId":"siox","version":1,"text":source}}),
        );
        self.diagnostics(uri)
    }
    fn finish(&mut self) {
        assert_eq!(self.request("shutdown", Value::Null)["result"], Value::Null);
        self.notify("exit", Value::Null);
        assert!(self.child.wait().unwrap().success());
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn uri(name: &str) -> String {
    Url::from_file_path(std::env::temp_dir().join(name))
        .unwrap()
        .to_string()
}
fn at(source: &str, byte: usize) -> Value {
    let prefix = &source[..byte];
    json!({"line":prefix.bytes().filter(|&b| b==b'\n').count(),"character":prefix.rsplit('\n').next().unwrap().encode_utf16().count()})
}
fn position_params(uri: &str, source: &str, byte: usize) -> Value {
    json!({"textDocument":{"uri":uri},"position":at(source,byte)})
}
fn errors(diagnostics: &Value) -> Vec<&Value> {
    diagnostics["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["severity"] == 1)
        .collect()
}

#[test]
fn lifecycle_and_malformed_requests_recover() {
    let mut client = Client::new();
    assert_eq!(
        client.request("textDocument/hover", json!({}))["error"]["code"],
        -32002
    );
    client.raw(b"not json");
    assert_eq!(client.receive()["error"]["code"], -32700);
    for invalid in [
        json!([]),
        json!({"jsonrpc":"1.0","id":9,"method":"initialize"}),
        json!({"jsonrpc":"2.0","id":true,"method":"initialize"}),
    ] {
        client.send(invalid);
        let response = client.receive();
        assert_eq!(response["error"]["code"], -32600);
        assert_eq!(response["id"], Value::Null);
    }
    let capabilities = client.initialize();
    assert_eq!(capabilities["positionEncoding"], "utf-16");
    assert_eq!(capabilities["textDocumentSync"]["change"], 2);
    assert_eq!(
        client.request("initialize", json!({}))["error"]["code"],
        -32600
    );
    assert_eq!(
        client.request("custom/unknown", json!({}))["error"]["code"],
        -32601
    );
    client.send(json!({"jsonrpc":"2.0","id":"string-id","method":"custom/unknown"}));
    let string_id = client.receive();
    assert_eq!(string_id["id"], "string-id");
    assert_eq!(string_id["error"]["code"], -32601);
    assert_eq!(
        client.request("textDocument/hover", json!({}))["error"]["code"],
        -32602
    );
    assert_eq!(
        client.request("shutdown", Value::Null)["result"],
        Value::Null
    );
    assert_eq!(
        client.request("custom/unknown", json!({}))["error"]["code"],
        -32600
    );
    client.notify("exit", Value::Null);
    assert!(client.child.wait().unwrap().success());
    let mut early_exit = Client::new();
    early_exit.notify("exit", Value::Null);
    assert_eq!(early_exit.child.wait().unwrap().code(), Some(1));
}

#[test]
fn unsaved_diagnostics_incremental_utf16_versions_and_close() {
    let mut client = Client::new();
    client.initialize();
    let uri = uri("siox lsp#😀.siox");
    // A string with a surrogate pair precedes the error on the same line.
    let source = "module editor; const TEXT: string = \"😀\"; fn bad(x: Missing) {}\n";
    let initial = client.open(&uri, source);
    let missing = errors(&initial)
        .into_iter()
        .find(|d| d["message"].as_str().unwrap().contains("Missing"))
        .expect("compiler unknown-type diagnostic");
    let start = source.find("Missing").unwrap();
    assert_eq!(missing["range"]["start"], at(source, start));
    assert!(missing["code"].as_str().unwrap().starts_with("E-"));
    client.notify(
        "textDocument/didChange",
        json!({"textDocument":{"uri":uri,"version":2},"contentChanges":[
            {"range":{"start":at(source,start),"end":at(source,start+7)},"text":"integer"}
        ]}),
    );
    let corrected = client.diagnostics(&uri);
    assert_eq!(corrected["version"], 2);
    assert!(errors(&corrected).is_empty(), "{corrected}");
    let after_emoji = source.find('😀').unwrap() + '😀'.len_utf8();
    assert_eq!(
        client.request(
            "textDocument/completion",
            position_params(&uri, source, after_emoji)
        )["result"],
        json!([])
    );
    // A stale full-text change cannot restore the old error.
    client.notify(
        "textDocument/didChange",
        json!({"textDocument":{"uri":uri,"version":1},"contentChanges":[{"text":source}]}),
    );
    let symbol = client.request(
        "textDocument/documentSymbol",
        json!({"textDocument":{"uri":uri}}),
    );
    assert!(symbol["result"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["name"] == "bad"));
    // Invalid edits leave the last valid snapshot intact and report a log.
    client.notify("textDocument/didChange",json!({"textDocument":{"uri":uri,"version":3},"contentChanges":[
        {"text":source},
        {"range":{"start":{"line":99,"character":0},"end":{"line":99,"character":0}},"text":"oops"}
    ]}));
    assert_eq!(client.receive()["method"], "window/logMessage");
    client.notify("textDocument/didSave", json!({"textDocument":{"uri":uri}}));
    let unchanged = client.diagnostics(&uri);
    assert_eq!(unchanged["version"], 2);
    assert!(errors(&unchanged).is_empty(), "{unchanged}");
    client.notify("textDocument/didClose", json!({"textDocument":{"uri":uri}}));
    assert_eq!(client.diagnostics(&uri)["diagnostics"], json!([]));
    client.finish();
}

#[test]
fn compiler_navigation_hover_references_symbols_and_completion() {
    let mut client = Client::new();
    client.initialize();
    let uri = uri("siox-lsp-navigation.siox");
    let source = "module editor;\nstruct Packet { pub value: integer }\nfn pass(x: Packet) -> Packet { return x; }\nfn first() -> integer { let local: integer = 1; return local; }\nfn second() -> integer { let local: integer = 2; return local; }\nfn echo(x: Bit) -> Bit { return x; }\n";
    let diagnostics = client.open(&uri, source);
    assert!(errors(&diagnostics).is_empty(), "{diagnostics}");
    let byte = source.find("x: Packet").unwrap() + 3;
    let definition = client.request(
        "textDocument/definition",
        position_params(&uri, source, byte),
    );
    assert_eq!(definition["result"]["uri"], uri);
    assert_eq!(
        definition["result"]["range"]["start"],
        at(source, source.find("Packet").unwrap())
    );
    let hover = client.request("textDocument/hover", position_params(&uri, source, byte));
    assert!(hover["result"]["contents"]["value"]
        .as_str()
        .unwrap()
        .contains("Packet"));
    let x = source.find("return x;").unwrap() + 7;
    let ty = client.request(
        "textDocument/typeDefinition",
        position_params(&uri, source, x),
    );
    assert_eq!(ty["result"]["range"], definition["result"]["range"]);
    let local = source.find("return local").unwrap() + 7;
    let mut params = position_params(&uri, source, local);
    params["context"] = json!({"includeDeclaration":true});
    let references = client.request("textDocument/references", params.clone());
    assert_eq!(
        references["result"].as_array().unwrap().len(),
        2,
        "{references}"
    );
    params["context"]["includeDeclaration"] = json!(false);
    assert_eq!(
        client.request("textDocument/references", params)["result"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        client.request(
            "textDocument/documentHighlight",
            position_params(&uri, source, local)
        )["result"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let bit = source.find("x: Bit").unwrap() + 3;
    let library = client.request(
        "textDocument/definition",
        position_params(&uri, source, bit),
    );
    assert!(
        library["result"]["uri"].as_str().unwrap().contains("/std/"),
        "{library}"
    );
    let symbols = client.request(
        "textDocument/documentSymbol",
        json!({"textDocument":{"uri":uri}}),
    );
    assert!(symbols["result"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["name"] == "Packet" && s["children"][0]["name"] == "value"));
    let completions = client.request("textDocument/completion", position_params(&uri, source, 0));
    assert!(completions["result"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["label"] == "process"));
    assert!(completions["result"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["label"] == "Packet"));
    client.finish();
}

#[test]
fn formatting_preserves_comments_and_unexpanded_source() {
    let mut client = Client::new();
    client.initialize();
    let uri = uri("siox-lsp-format.siox");
    let source = "module editor;const A: integer=1;";
    assert!(errors(&client.open(&uri, source)).is_empty());
    let params = json!({"textDocument":{"uri":uri},"options":{"tabSize":4,"insertSpaces":true}});
    let formatted = client.request("textDocument/formatting", params.clone());
    assert_eq!(formatted["result"].as_array().unwrap().len(), 1);
    let text = formatted["result"][0]["newText"]
        .as_str()
        .unwrap()
        .to_string();
    client.notify(
        "textDocument/didChange",
        json!({"textDocument":{"uri":uri,"version":2},"contentChanges":[{"text":text}]}),
    );
    assert!(errors(&client.diagnostics(&uri)).is_empty());
    assert_eq!(
        client.request("textDocument/formatting", params.clone())["result"],
        json!([])
    );
    client.notify("textDocument/didChange",json!({"textDocument":{"uri":uri,"version":3},"contentChanges":[{"text":"module editor; // keep me\nconst A: integer=1;"}]}));
    client.diagnostics(&uri);
    assert_eq!(
        client.request("textDocument/formatting", params.clone())["result"],
        json!([])
    );
    let macro_source = "module editor; macro one() { 1 } const A: integer = one!();";
    client.notify(
        "textDocument/didChange",
        json!({"textDocument":{"uri":uri,"version":4},"contentChanges":[{"text":macro_source}]}),
    );
    assert!(errors(&client.diagnostics(&uri)).is_empty());
    let result = client.request("textDocument/formatting", params);
    let edits = result["result"].as_array().unwrap();
    assert_eq!(edits.len(), 1);
    assert!(edits[0]["newText"].as_str().unwrap().contains("one!()"));
    client.finish();
}

#[test]
fn flat_symbol_fallback_qualified_paths_and_partial_parse() {
    let mut client = Client::new();
    let initialized = client.request(
        "initialize",
        json!({"processId":null,"rootUri":null,"capabilities":{}}),
    );
    assert!(initialized.get("error").is_none());
    let uri = uri("siox-lsp-qualified.siox");
    let source = "module editor; struct S { value: integer } fn echo(x: std::logic::Bit) -> std::logic::Bit { return x; }";
    assert!(errors(&client.open(&uri, source)).is_empty());
    let bit = source.find("::Bit").unwrap() + 2;
    let navigation = client.request(
        "textDocument/definition",
        position_params(&uri, source, bit),
    );
    assert!(
        navigation["result"]["uri"]
            .as_str()
            .unwrap()
            .ends_with("/std/logic.siox"),
        "{navigation}"
    );
    let symbols = client.request(
        "textDocument/documentSymbol",
        json!({"textDocument":{"uri":uri}}),
    );
    let field = symbols["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "value")
        .unwrap();
    assert_eq!(field["containerName"], "S");
    assert_eq!(field["location"]["uri"], uri);
    client.notify("textDocument/didChange",json!({"textDocument":{"uri":uri,"version":2},"contentChanges":[{"text":"module editor; struct S { value: integer } fn broken("}]}));
    assert!(!errors(&client.diagnostics(&uri)).is_empty());
    let partial = client.request(
        "textDocument/documentSymbol",
        json!({"textDocument":{"uri":uri}}),
    );
    assert!(partial["result"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["name"] == "S"));
    assert_eq!(
        client.request(
            "textDocument/formatting",
            json!({"textDocument":{"uri":uri},"options":{"tabSize":4,"insertSpaces":true}})
        )["result"],
        json!([])
    );
    client.finish();
}

#[test]
fn closing_one_document_keeps_other_diagnostics() {
    let mut client = Client::new();
    client.initialize();
    let first = uri("siox-lsp-first.siox");
    let second = uri("siox-lsp-second.siox");
    client.open(&first, "module one; fn bad(x: MissingOne) {}");
    let diagnostics = client.open(&second, "module two; fn bad(x: MissingTwo) {}");
    assert!(!errors(&diagnostics).is_empty());
    client.notify(
        "textDocument/didClose",
        json!({"textDocument":{"uri":first}}),
    );
    assert_eq!(client.diagnostics(&first)["diagnostics"], json!([]));
    assert!(!errors(&client.diagnostics(&second)).is_empty());
    client.finish();
}

#[test]
fn related_diagnostics_and_sequential_crlf_edits() {
    let mut client = Client::new();
    client.initialize();
    let uri = uri("siox-lsp-related.siox");
    let duplicate = "module editor;\r\nconst DUP: integer = 1;\r\nconst DUP: integer = 2;\r\n";
    let diagnostics = client.open(&uri, duplicate);
    let related = errors(&diagnostics)
        .into_iter()
        .find(|d| d["relatedInformation"].is_array())
        .expect("duplicate declaration has a compiler secondary location");
    assert_eq!(related["relatedInformation"][0]["location"]["uri"], uri);
    assert_eq!(
        related["relatedInformation"][0]["location"]["range"]["start"],
        at(duplicate, duplicate.find("DUP").unwrap())
    );

    let source = "module editor;\r\nfn echo(x: integer) -> integer { return x; }\r\n";
    let first = source.find("x: integer").unwrap();
    let after = source.replacen("x: integer", "value: integer", 1);
    let second = after.find("return x").unwrap() + 7;
    client.notify("textDocument/didChange",json!({"textDocument":{"uri":uri,"version":2},"contentChanges":[
        {"text":source},
        {"range":{"start":at(source,first),"end":at(source,first+1)},"text":"value"},
        {"range":{"start":at(&after,second),"end":at(&after,second+1)},"text":"value"},
        {"range":{"start":{"line":1,"character":999},"end":{"line":1,"character":999}},"text":" "}
    ]}));
    let repaired = client.diagnostics(&uri);
    assert_eq!(repaired["version"], 2);
    assert!(errors(&repaired).is_empty(), "{repaired}");
    let result = client.request(
        "textDocument/definition",
        position_params(&uri, &after, second),
    );
    assert_eq!(result["result"]["range"]["start"], at(source, first));
    // A malformed open cannot create a phantom buffer or terminate the server.
    client.notify(
        "textDocument/didOpen",
        json!({"textDocument":{"uri":"https://example.com/input.siox","version":1,"text":source}}),
    );
    assert_eq!(client.receive()["method"], "window/logMessage");
    client.finish();
}

#[test]
fn hardware_views_traits_ports_and_named_process_outline() {
    let mut client = Client::new();
    client.initialize();
    let uri = uri("siox-lsp-hardware.siox");
    let source = "module editor;\nuse std::bits::unsigned;\nstruct Bus { pub data: unsigned[8] }\nview Source for Bus { data out }\nenum State { Idle, Busy }\ntrait Reset { fn reset(self); }\ntype Word = unsigned[8];\nentity Device { input: Bit in, output: Bit out }\nimpl Device { relay: process { output = input; } }\n";
    let diagnostics = client.open(&uri, source);
    assert!(errors(&diagnostics).is_empty(), "{diagnostics}");
    let result = client.request(
        "textDocument/documentSymbol",
        json!({"textDocument":{"uri":uri}}),
    );
    let symbols = result["result"].as_array().unwrap();
    for (name, child) in [
        ("Bus", "data"),
        ("Source", "data"),
        ("State", "Busy"),
        ("Reset", "reset"),
        ("Device", "input"),
        ("impl Device", "relay"),
    ] {
        let symbol = symbols.iter().find(|s| s["name"] == name).expect(name);
        assert!(
            symbol["children"]
                .as_array()
                .unwrap()
                .iter()
                .any(|s| s["name"] == child),
            "{symbol}"
        );
    }
    assert!(symbols.iter().any(|s| s["name"] == "Word"));
    let input = source.rfind("input;").unwrap();
    let definition = client.request(
        "textDocument/definition",
        position_params(&uri, source, input),
    );
    // Value-side port checking has not published a resolver declaration ID;
    // the adapter must not invent an identity by matching the port's spelling.
    assert_eq!(definition["result"], Value::Null);
    let port_type = client.request(
        "textDocument/typeDefinition",
        position_params(&uri, source, input),
    );
    assert!(
        port_type["result"]["uri"]
            .as_str()
            .unwrap()
            .ends_with("/std/logic.siox"),
        "{port_type}"
    );
    client.finish();
}

#[test]
fn warning_codes_help_and_repair() {
    let mut client = Client::new();
    client.initialize();
    let uri = uri("siox-lsp-warning.siox");
    let source = "module editor; use std::bits::signed;";
    let diagnostics = client.open(&uri, source);
    assert!(errors(&diagnostics).is_empty(), "{diagnostics}");
    let warning = diagnostics["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["code"] == siox::diag::codes::UNUSED_IMPORT)
        .expect("structured unused-import warning");
    assert_eq!(warning["severity"], 2);
    assert_eq!(warning["source"], "sioxc");
    assert!(warning["message"]
        .as_str()
        .unwrap()
        .contains("help: remove it"));
    client.notify("textDocument/didChange",json!({"textDocument":{"uri":uri,"version":2},"contentChanges":[{"text":"module editor;"}]}));
    assert_eq!(client.diagnostics(&uri)["diagnostics"], json!([]));
    client.finish();
}

#[test]
fn cli_arguments_and_compiler_checkout_library_root() {
    for arguments in [vec!["--unknown"], vec!["--std"]] {
        let output = Command::new(env!("CARGO_BIN_EXE_siox-lsp"))
            .args(arguments)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(
            output.stdout.is_empty(),
            "errors must not pollute LSP stdout"
        );
        assert!(!output.stderr.is_empty());
    }
    let output = Command::new(env!("CARGO_BIN_EXE_siox-lsp"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8(output.stdout)
        .unwrap()
        .contains("--stdio"));

    let mut client = Client::with_std(std_root().parent().unwrap().to_path_buf());
    client.initialize();
    let uri = uri("siox-lsp-checkout-root.siox");
    let source = "module editor; const ONE: Bit = '1';";
    assert!(errors(&client.open(&uri, source)).is_empty());
    let definition = client.request(
        "textDocument/definition",
        position_params(&uri, source, source.find("Bit").unwrap()),
    );
    assert!(
        definition["result"]["uri"]
            .as_str()
            .unwrap()
            .ends_with("/std/logic.siox"),
        "{definition}"
    );
    client.finish();
}
