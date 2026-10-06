//! Editor views of compiler products. No parser or name/type checker lives here.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{json, Value};
use siox::compiler::{Artifact, Compilation, CompileRequest, Compiler, Emit, SourceInput};
use siox::diag::{DiagnosticSink, Severity, Span};
use siox::resolve::{DefId, DefKind};
use siox::syntax::{ast::*, lexer::Lexer, pretty, token::TokenKind};
use siox::types::Ty;

use crate::text;

pub struct Analysis {
    pub compilation: Compilation,
    occurrences: Vec<(Span, DefId, bool)>,
}

impl Analysis {
    pub fn new(compiler: &Compiler, path: &Path, source: &str) -> Self {
        // ponytail: recompile one entry synchronously; use compiler queries if
        // measured edit latency or retained compilation memory requires caching.
        let compilation = compiler.compile(CompileRequest::new(
            SourceInput::memory(path, source),
            Emit::Metadata,
        ));
        let mut occurrences = Vec::new();
        if let Some(resolved) = &compilation.resolved {
            for module in &compilation.modules {
                let file = module.span.file;
                let Some(source) = compilation.sources.get(file) else {
                    continue;
                };
                let tokens = Lexer::new(file, &source.text).tokenize(&mut DiagnosticSink::new());
                for (i, token) in tokens.iter().enumerate() {
                    if !matches!(token.kind, TokenKind::Ident | TokenKind::SelfKw) {
                        continue;
                    }
                    if let Some(id) = resolved.declared(token.span) {
                        occurrences.push((token.span, id, true));
                        continue;
                    }
                    // The resolver records complete `a::b::Name` spans. Check
                    // the prefix ending at this token, never guess by spelling.
                    let mut start = i;
                    while start >= 2
                        && tokens[start - 1].kind == TokenKind::ColonColon
                        && matches!(
                            tokens[start - 2].kind,
                            TokenKind::Ident | TokenKind::SelfKw | TokenKind::Super
                        )
                    {
                        start -= 2;
                    }
                    if let Some(id) = resolved
                        .resolved(token.span)
                        .or_else(|| resolved.resolved(tokens[start].span.to(token.span)))
                    {
                        occurrences.push((token.span, id, false));
                    }
                }
            }
        }
        Self {
            compilation,
            occurrences,
        }
    }

    pub fn source(&self) -> &str {
        self.compilation
            .entry_file
            .and_then(|f| self.compilation.sources.get(f))
            .map_or("", |f| f.text.as_str())
    }

    pub fn span_range(&self, span: Span) -> Option<Value> {
        Some(text::range(
            &self.compilation.sources.get(span.file)?.text,
            span.start,
            span.end,
        ))
    }

    pub fn location(&self, span: Span) -> Option<Value> {
        let source = self.compilation.sources.get(span.file)?;
        Some(
            json!({"uri": text::uri(Path::new(&source.name)).ok()?, "range": self.span_range(span)?}),
        )
    }

    fn at(&self, byte: usize) -> Option<(Span, DefId)> {
        let file = self.compilation.entry_file?;
        self.occurrences
            .iter()
            .find(|(span, _, _)| {
                span.file == file && span.start as usize <= byte && byte < span.end as usize
            })
            .map(|&(span, id, _)| (span, id))
    }

    fn ty_at(&self, byte: usize) -> Option<(Span, &Ty)> {
        let file = self.compilation.entry_file?;
        self.compilation
            .typed
            .as_ref()?
            .expr_types()
            .iter()
            .filter(|(span, ty)| {
                span.file == file
                    && span.start as usize <= byte
                    && byte < span.end as usize
                    && **ty != Ty::Error
            })
            .min_by_key(|(span, _)| span.end - span.start)
            .map(|(span, ty)| (*span, ty))
    }

    fn type_name(&self, ty: &Ty) -> String {
        match ty {
            Ty::Integer => "integer".into(),
            Ty::Real => "real".into(),
            Ty::Char => "Char".into(),
            Ty::Named(id) => self
                .compilation
                .resolved
                .as_ref()
                .and_then(|r| r.qualified_name(*id))
                .unwrap_or_default(),
            Ty::Array { elem, len, family } => format!(
                "{}[{len}]",
                family.clone().unwrap_or_else(|| self.type_name(elem))
            ),
            Ty::Void => "void".into(),
            Ty::Error => "<unknown>".into(),
        }
    }

    pub fn hover(&self, byte: usize) -> Value {
        let declaration = self.at(byte).and_then(|(span, id)| {
            let resolved = self.compilation.resolved.as_ref()?;
            let info = resolved.def(id)?;
            Some((
                span,
                format!("{:?} {}", info.kind, resolved.qualified_name(id)?),
            ))
        });
        let typed = self.ty_at(byte);
        let Some(span) = declaration
            .as_ref()
            .map(|(s, _)| *s)
            .or_else(|| typed.map(|(s, _)| s))
        else {
            return Value::Null;
        };
        let mut value = declaration.map_or_else(String::new, |(_, name)| name);
        if let Some((_, ty)) = typed {
            if !value.is_empty() {
                value.push('\n');
            }
            value.push_str(&format!("type: {}", self.type_name(ty)));
        }
        json!({"contents":{"kind":"plaintext","value":value}, "range":self.span_range(span)})
    }

    pub fn definition(&self, byte: usize) -> Value {
        self.at(byte)
            .and_then(|(_, id)| self.compilation.resolved.as_ref()?.def(id)?.span)
            .and_then(|span| self.location(span))
            .unwrap_or(Value::Null)
    }

    pub fn type_definition(&self, byte: usize) -> Value {
        let Some((_, Ty::Named(id))) = self.ty_at(byte) else {
            return Value::Null;
        };
        self.compilation
            .resolved
            .as_ref()
            .and_then(|r| r.def(*id))
            .and_then(|d| d.span)
            .and_then(|span| self.location(span))
            .unwrap_or(Value::Null)
    }

    pub fn references(&self, byte: usize, include_declaration: bool, highlights: bool) -> Value {
        let Some((_, id)) = self.at(byte) else {
            return json!([]);
        };
        let found: Vec<Value> = self
            .occurrences
            .iter()
            .filter(|(span, target, declaration)| {
                *target == id
                    && (include_declaration || !declaration)
                    && (!highlights || Some(span.file) == self.compilation.entry_file)
            })
            .filter_map(|(span, _, _)| {
                if highlights {
                    Some(json!({"range":self.span_range(*span)?, "kind":1}))
                } else {
                    self.location(*span)
                }
            })
            .collect();
        json!(found)
    }

    /// Preserve notes, help and secondary locations rather than scraping CLI text.
    pub fn diagnostics(&self, entry_uri: &str) -> BTreeMap<String, Vec<Value>> {
        let mut groups = BTreeMap::from([(entry_uri.to_string(), Vec::new())]);
        for diagnostic in self.compilation.diagnostics() {
            let location = diagnostic.primary.and_then(|s| self.location(s));
            let uri = location
                .as_ref()
                .and_then(|l| l["uri"].as_str())
                .unwrap_or(entry_uri);
            let uri = if diagnostic
                .primary
                .is_some_and(|s| Some(s.file) == self.compilation.entry_file)
            {
                entry_uri
            } else {
                uri
            };
            let range = location
                .as_ref()
                .map(|l| l["range"].clone())
                .unwrap_or_else(|| text::range(self.source(), 0, 0));
            let mut message = diagnostic.message.clone();
            for note in &diagnostic.notes {
                message.push_str(&format!("\nnote: {note}"));
            }
            if let Some(help) = &diagnostic.help {
                message.push_str(&format!("\nhelp: {help}"));
            }
            let mut rendered = json!({"range":range,"message":message,"source":"sioxc","severity":match diagnostic.severity {
                Severity::Error => 1, Severity::Warning => 2, Severity::Note => 3, Severity::Help => 4,
            }});
            if let Some(code) = diagnostic.code {
                rendered["code"] = json!(code);
            }
            let related: Vec<Value> = diagnostic
                .labels
                .iter()
                .filter_map(|label| {
                    Some(json!({"location":self.location(label.span)?, "message":label.message}))
                })
                .collect();
            if !related.is_empty() {
                rendered["relatedInformation"] = json!(related);
            }
            groups.entry(uri.to_string()).or_default().push(rendered);
        }
        if let Some(failure) = &self.compilation.failure {
            groups.entry(entry_uri.to_string()).or_default().push(json!({
                "range":text::range(self.source(),0,0),"severity":1,"source":"sioxc","message":failure.message
            }));
        }
        groups
    }

    pub fn symbols(&self) -> Value {
        let Some(module) = self.compilation.entry() else {
            return json!([]);
        };
        json!(module
            .items
            .iter()
            .filter_map(|item| self.item_symbol(item))
            .collect::<Vec<_>>())
    }

    fn symbol(&self, name: &Ident, span: Span, kind: u32, children: Vec<Value>) -> Value {
        json!({"name":name.text,"kind":kind,"range":self.span_range(span.to(name.span)),
            "selectionRange":self.span_range(name.span),"children":children})
    }

    fn item_symbol(&self, item: &Item) -> Option<Value> {
        Some(match item {
            Item::Struct(d) => self.symbol(
                &d.name,
                d.span,
                23,
                d.fields
                    .iter()
                    .map(|f| self.symbol(&f.name, f.span, 8, vec![]))
                    .collect(),
            ),
            Item::View(d) => self.symbol(
                &d.name,
                d.span,
                11,
                d.fields
                    .iter()
                    .map(|f| self.symbol(&f.name, f.span, 8, vec![]))
                    .collect(),
            ),
            Item::Entity(d) => self.symbol(
                &d.name,
                d.span,
                5,
                d.ports
                    .iter()
                    .map(|p| self.symbol(&p.name, p.span, 8, vec![]))
                    .collect(),
            ),
            Item::Enum(d) => self.symbol(
                &d.name,
                d.span,
                10,
                d.variants
                    .iter()
                    .map(|v| self.symbol(&v.name, v.span, 22, vec![]))
                    .collect(),
            ),
            Item::Trait(d) => self.symbol(
                &d.name,
                d.span,
                11,
                d.items
                    .iter()
                    .map(|f| self.symbol(&f.name, f.span, 6, vec![]))
                    .collect(),
            ),
            Item::Fn(d) => self.symbol(&d.name, d.span, 12, vec![]),
            Item::Const(d) => self.symbol(&d.name, d.span, 14, vec![]),
            Item::AttrDecl(d) => self.symbol(&d.name, d.span, 7, vec![]),
            Item::Using(Using {
                kind: UsingKind::Alias { name, .. },
                span,
                ..
            }) => self.symbol(name, *span, 23, vec![]),
            Item::Impl(d) => {
                let name = Ident {
                    text: format!("impl {}", pretty::type_str(&d.target)),
                    span: Span {
                        end: d.span.start + 4,
                        ..d.span
                    },
                };
                let children = d
                    .items
                    .iter()
                    .filter_map(|item| {
                        Some(match item {
                            ImplItem::Fn(f) => self.symbol(&f.name, f.span, 6, vec![]),
                            ImplItem::Const(c) => self.symbol(&c.name, c.span, 14, vec![]),
                            ImplItem::Let(l) => self.symbol(&l.name, l.span, 13, vec![]),
                            ImplItem::Process(p) => self.symbol(
                                &p.label.clone().unwrap_or(Ident {
                                    text: "process".into(),
                                    span: Span {
                                        end: p.span.start + 7,
                                        ..p.span
                                    },
                                }),
                                p.span,
                                6,
                                vec![],
                            ),
                            _ => return None,
                        })
                    })
                    .collect();
                self.symbol(&name, d.span, 19, children)
            }
            _ => return None,
        })
    }

    pub fn completion(&self, byte: usize) -> Value {
        if self.compilation.entry_tokens.iter().any(|token| {
            matches!(
                token.kind,
                TokenKind::Comment | TokenKind::StrLit | TokenKind::CharacterLit
            ) && token.span.start as usize <= byte
                && byte < token.span.end as usize
        }) {
            return json!([]);
        }
        let prefix = &self.source()[..byte];
        let start = prefix
            .char_indices()
            .rev()
            .find(|(_, c)| !(c.is_alphanumeric() || *c == '_'))
            .map_or(0, |(i, c)| i + c.len_utf8());
        let stem = &prefix[start..];
        let before = prefix[..start].trim_end();
        // No guessed member/module scope: a later compiler query can supply it.
        if before.ends_with('.') || before.ends_with("::") {
            return json!([]);
        }
        let mut items = BTreeMap::new();
        if let Some(resolved) = &self.compilation.resolved {
            for def in resolved.defs().iter().filter(|d| {
                d.kind == DefKind::Builtin
                    || (d
                        .span
                        .is_some_and(|s| Some(s.file) == self.compilation.entry_file)
                        && !matches!(
                            d.kind,
                            DefKind::Local | DefKind::Param | DefKind::EnumVariant
                        ))
            }) {
                items.insert(
                    def.name.clone(),
                    json!({"label":def.name,"kind":match def.kind {
                        DefKind::Fn => 3, DefKind::Const => 21, DefKind::Enum => 13,
                        DefKind::Trait|DefKind::View => 8, _ => 22,
                    }}),
                );
            }
        }
        if let Some(module) = self.compilation.entry() {
            for item in &module.items {
                if let Some(symbol) = self.item_symbol(item) {
                    if !matches!(item, Item::Impl(_)) {
                        let name = symbol["name"].as_str().unwrap().to_string();
                        items
                            .entry(name.clone())
                            .or_insert(json!({"label":name,"kind":22}));
                    }
                }
                if let Item::Using(Using {
                    kind: UsingKind::Import { names, .. },
                    ..
                }) = item
                {
                    for import in names.iter().filter(|i| !i.glob) {
                        let label = &import.binding().text;
                        items
                            .entry(label.clone())
                            .or_insert(json!({"label":label,"kind":22}));
                    }
                }
            }
        }
        // Source spellings come from the compiler's token catalogue.
        for keyword in [
            TokenKind::Entity,
            TokenKind::Struct,
            TokenKind::View,
            TokenKind::Enum,
            TokenKind::Impl,
            TokenKind::Trait,
            TokenKind::Fn,
            TokenKind::Let,
            TokenKind::Const,
            TokenKind::Process,
            TokenKind::Use,
            TokenKind::Type,
            TokenKind::If,
            TokenKind::Else,
            TokenKind::For,
            TokenKind::Match,
            TokenKind::Return,
            TokenKind::Pub,
            TokenKind::Attr,
            TokenKind::Extern,
            TokenKind::Module,
            TokenKind::In,
            TokenKind::Out,
            TokenKind::Inout,
        ] {
            let label = keyword.describe().trim_matches('`');
            items
                .entry(label.into())
                .or_insert(json!({"label":label,"kind":14}));
        }
        json!(items
            .into_iter()
            .filter(|(label, _)| label.starts_with(stem))
            .map(|(_, value)| value)
            .collect::<Vec<_>>())
    }

    pub fn formatting(&self, compiler: &Compiler) -> Value {
        // Formatting expanded/desugared modules would erase macros/imports.
        // Reuse the compiler parser/printer on the original in-memory source.
        if self
            .compilation
            .entry_tokens
            .iter()
            .any(|t| t.kind == TokenKind::Comment)
            || self.compilation.diagnostics.has_errors()
        {
            return json!([]);
        }
        let Some(source) = self
            .compilation
            .entry_file
            .and_then(|f| self.compilation.sources.get(f))
        else {
            return json!([]);
        };
        let parsed = compiler.compile(CompileRequest::new(
            SourceInput::memory(&source.name, self.source()),
            Emit::Source,
        ));
        let Some(Artifact::Text(formatted)) = parsed.artifact else {
            return json!([]);
        };
        if formatted == self.source() {
            return json!([]);
        }
        json!([{"range":{"start":text::position(self.source(),0),"end":text::position(self.source(),self.source().len())},"newText":formatted}])
    }
}
