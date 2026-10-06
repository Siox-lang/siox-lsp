//! Synchronous LSP lifecycle and document ownership; the compiler owns analysis.

use crate::{
    analysis::Analysis,
    protocol,
    text::{self, Result},
};
use serde_json::{json, Value};
use siox::compiler::Compiler;
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, BufRead, Write},
    path::PathBuf,
};

struct Document {
    version: i64,
    analysis: Analysis,
}

pub struct Server {
    compiler: Compiler,
    documents: BTreeMap<String, Document>,
    published: BTreeSet<String>,
    initialized: bool,
    shutdown: bool,
    hierarchical_symbols: bool,
}

impl Server {
    pub fn new(std_root: PathBuf) -> Self {
        Self {
            compiler: Compiler::new(std_root),
            documents: BTreeMap::new(),
            published: BTreeSet::new(),
            initialized: false,
            shutdown: false,
            hierarchical_symbols: false,
        }
    }

    pub fn run(&mut self, reader: &mut impl BufRead, writer: &mut impl Write) -> io::Result<u8> {
        while let Some(body) = protocol::read(reader)? {
            let message: Value = match serde_json::from_slice(&body) {
                Ok(message) => message,
                Err(error) => {
                    protocol::write(
                        writer,
                        &error_response(Value::Null, -32700, &error.to_string()),
                    )?;
                    continue;
                }
            };
            let id = message.get("id").cloned();
            // The server issues no requests; ignore client responses.
            if message.get("method").is_none()
                && (message.get("result").is_some() || message.get("error").is_some())
            {
                continue;
            }
            if message["jsonrpc"] != "2.0"
                || !message["method"].is_string()
                || id
                    .as_ref()
                    .is_some_and(|id| !id.is_string() && !id.is_i64())
            {
                protocol::write(
                    writer,
                    &error_response(Value::Null, -32600, "invalid JSON-RPC request"),
                )?;
                continue;
            }
            let method = message["method"].as_str().unwrap();
            let params = &message["params"];
            if method == "exit" && id.is_none() {
                return Ok(if self.shutdown { 0 } else { 1 });
            }
            if !self.initialized && method != "initialize" {
                if let Some(id) = id {
                    protocol::write(
                        writer,
                        &error_response(id, -32002, "server not initialized"),
                    )?;
                }
                continue;
            }
            if self.shutdown {
                if let Some(id) = id {
                    protocol::write(writer, &error_response(id, -32600, "server has shut down"))?;
                }
                continue;
            }
            if let Some(id) = id {
                let result = match method {
                    "initialize" if !self.initialized => {
                        if !params.is_object() {
                            Err((-32602, "initialize params must be an object".into()))
                        } else {
                            self.initialized = true;
                            self.hierarchical_symbols = params["capabilities"]["textDocument"]
                                ["documentSymbol"]["hierarchicalDocumentSymbolSupport"]
                                .as_bool()
                                .unwrap_or(false);
                            Ok(json!({"capabilities":{
                                "positionEncoding":"utf-16",
                                "textDocumentSync":{"openClose":true,"change":2,"save":{"includeText":false}},
                                "hoverProvider":true,"definitionProvider":true,"typeDefinitionProvider":true,
                                "referencesProvider":true,"documentHighlightProvider":true,
                                "documentSymbolProvider":true,"completionProvider":{"resolveProvider":false},
                                "documentFormattingProvider":true
                            },"serverInfo":{"name":"siox-lsp","version":env!("CARGO_PKG_VERSION")}}))
                        }
                    }
                    "initialize" => Err((-32600, "already initialized".into())),
                    "shutdown" => {
                        self.shutdown = true;
                        Ok(Value::Null)
                    }
                    "textDocument/hover"
                    | "textDocument/definition"
                    | "textDocument/typeDefinition"
                    | "textDocument/references"
                    | "textDocument/documentHighlight"
                    | "textDocument/documentSymbol"
                    | "textDocument/completion"
                    | "textDocument/formatting" => {
                        self.feature(method, params).map_err(|e| (-32602, e))
                    }
                    _ => Err((-32601, format!("unsupported method: {method}"))),
                };
                let response = match result {
                    Ok(value) => json!({"jsonrpc":"2.0","id":id,"result":value}),
                    Err((code, message)) => error_response(id, code, &message),
                };
                protocol::write(writer, &response)?;
            } else {
                match self.notification(method, params) {
                    Ok(true) => self.publish(writer)?,
                    Ok(false) => {}
                    Err(error) => protocol::write(
                        writer,
                        &json!({"jsonrpc":"2.0","method":"window/logMessage","params":{"type":2,"message":error}}),
                    )?,
                }
            }
        }
        Ok(if self.shutdown { 0 } else { 1 })
    }

    fn feature(&self, method: &str, params: &Value) -> Result<Value> {
        let uri = text::string(&params["textDocument"], "uri")?;
        let doc = self.documents.get(uri).ok_or("document is not open")?;
        let analysis = &doc.analysis;
        if method == "textDocument/documentSymbol" {
            let symbols = analysis.symbols();
            if self.hierarchical_symbols {
                return Ok(symbols);
            }
            let mut flat = Vec::new();
            flatten_symbols(&symbols, uri, None, &mut flat);
            return Ok(json!(flat));
        }
        if method == "textDocument/formatting" {
            return Ok(analysis.formatting(&self.compiler));
        }
        let byte = text::offset(analysis.source(), &params["position"])?;
        Ok(match method {
            "textDocument/hover" => analysis.hover(byte),
            "textDocument/definition" => analysis.definition(byte),
            "textDocument/typeDefinition" => analysis.type_definition(byte),
            "textDocument/completion" => analysis.completion(byte),
            "textDocument/references" => {
                let include = params["context"]["includeDeclaration"]
                    .as_bool()
                    .ok_or("missing includeDeclaration")?;
                analysis.references(byte, include, false)
            }
            "textDocument/documentHighlight" => analysis.references(byte, true, true),
            _ => unreachable!(),
        })
    }

    fn notification(&mut self, method: &str, params: &Value) -> Result<bool> {
        match method {
            "textDocument/didOpen" => {
                let input = &params["textDocument"];
                let uri = text::string(input, "uri")?;
                if self.documents.contains_key(uri) {
                    return Err("document already open".into());
                }
                let path = text::path(uri)?;
                let source = text::string(input, "text")?;
                let version = input["version"]
                    .as_i64()
                    .ok_or("invalid document version")?;
                let analysis = Analysis::new(&self.compiler, &path, source);
                self.documents
                    .insert(uri.into(), Document { version, analysis });
            }
            "textDocument/didChange" => {
                let input = &params["textDocument"];
                let uri = text::string(input, "uri")?;
                let doc = self
                    .documents
                    .get_mut(uri)
                    .ok_or("changed document is not open")?;
                let version = input["version"]
                    .as_i64()
                    .ok_or("invalid document version")?;
                if version <= doc.version {
                    return Ok(false);
                }
                let source = text::changed(doc.analysis.source(), &params["contentChanges"])?;
                doc.analysis = Analysis::new(&self.compiler, &text::path(uri)?, &source);
                doc.version = version;
            }
            "textDocument/didSave" => {
                let uri = text::string(&params["textDocument"], "uri")?;
                let doc = self
                    .documents
                    .get_mut(uri)
                    .ok_or("saved document is not open")?;
                doc.analysis =
                    Analysis::new(&self.compiler, &text::path(uri)?, doc.analysis.source());
            }
            "textDocument/didClose" => {
                let uri = text::string(&params["textDocument"], "uri")?;
                self.documents.remove(uri);
            }
            // Sequential requests finish before these can arrive. Do not claim
            // asynchronous cancellation or workspace indexing in this version.
            _ => return Ok(false),
        }
        Ok(true)
    }

    fn publish(&mut self, writer: &mut impl Write) -> io::Result<()> {
        let mut groups: BTreeMap<String, Vec<Value>> = BTreeMap::new();
        for (uri, document) in &self.documents {
            for (file, diagnostics) in document.analysis.diagnostics(uri) {
                // Another compilation reads dependencies from disk. Its spans
                // cannot describe a different unsaved buffer owned by the client.
                if &file != uri && self.documents.contains_key(&file) {
                    continue;
                }
                let combined = groups.entry(file).or_default();
                for diagnostic in diagnostics {
                    if !combined.contains(&diagnostic) {
                        combined.push(diagnostic);
                    }
                }
            }
        }
        // Clear diagnostics from closed documents and no-longer-loaded files;
        // keep diagnostics still owned by another open compilation.
        for uri in &self.published {
            groups.entry(uri.clone()).or_default();
        }
        self.published = groups
            .iter()
            .filter(|(uri, diagnostics)| {
                !diagnostics.is_empty() || self.documents.contains_key(*uri)
            })
            .map(|(uri, _)| uri.clone())
            .collect();
        for (uri, diagnostics) in groups {
            let mut params = json!({"uri":uri,"diagnostics":diagnostics});
            if let Some(doc) = self.documents.get(&uri) {
                params["version"] = json!(doc.version);
            }
            protocol::write(
                writer,
                &json!({"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":params}),
            )?;
        }
        Ok(())
    }
}

fn error_response(id: Value, code: i32, message: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
}

fn flatten_symbols(symbols: &Value, uri: &str, container: Option<&str>, result: &mut Vec<Value>) {
    for symbol in symbols.as_array().into_iter().flatten() {
        let mut flat = json!({"name":symbol["name"],"kind":symbol["kind"],
            "location":{"uri":uri,"range":symbol["selectionRange"]}});
        if let Some(container) = container {
            flat["containerName"] = json!(container);
        }
        result.push(flat);
        flatten_symbols(&symbol["children"], uri, symbol["name"].as_str(), result);
    }
}
