#![feature(let_chains)]

mod crates;
mod parse;

use std::{collections::HashMap, str::FromStr, sync::Arc};

use parse::Dependencies;
use ropey::Rope;
use tokio::sync::RwLock;
use toml_edit::ImDocument;
use tower_lsp::{
    Client, LanguageServer, LspService, Server, jsonrpc,
    lsp_types::{
        CodeActionProviderCapability, CompletionItem, CompletionOptions,
        CompletionOptionsCompletionItem, CompletionParams, CompletionResponse, Diagnostic,
        DiagnosticSeverity, DidChangeTextDocumentParams, DidOpenTextDocumentParams,
        ExecuteCommandOptions, InitializeParams, InitializeResult, InitializedParams, OneOf,
        ServerCapabilities, TextDocumentContentChangeEvent, TextDocumentSyncCapability,
        TextDocumentSyncKind, WorkspaceFoldersServerCapabilities, WorkspaceServerCapabilities,
    },
};
use url::Url;

#[derive(Debug)]
struct Backend {
    client: Client,
    documents: Arc<RwLock<HashMap<Url, Rope>>>,
}

impl Backend {
    async fn apply_changes(&self, uri: &Url, changes: Vec<TextDocumentContentChangeEvent>) {
        if let Some(doc) = self.documents.write().await.get_mut(uri) {
            // according to the [LSP spec]:
            //
            //   To mirror the content of a document using change events [...]
            //   apply the `TextDocumentContentChangeEvent`s in a single
            //   notification in the order you receive them.
            //
            // [LSP spec]: https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_didChange
            for change in changes {
                if let Some(range) = change.range {
                    let start = doc.line_to_byte(range.start.line as usize)
                        + range.start.character as usize;
                    let end =
                        doc.line_to_byte(range.end.line as usize) + range.end.character as usize;
                    doc.remove(start..end);
                    doc.insert(start, &change.text);
                } else {
                    // this doesn't suppose to happen
                    eprintln!("got full document in INCREMENTAL mode")
                }
            }
        }
    }

    async fn on_change(&self, uri: Url) {
        let docs = self.documents.read().await;
        let Some(doc) = docs.get(&uri) else {
            return;
        };
        let Ok(dependencies) = Dependencies::from_str(&doc.to_string()) else {
            return;
        };

        let mut diags = Vec::new();

        for (name, (range, _)) in dependencies.crates {
            if let Ok(index) = crates::fetch(name.as_str()).await {
                let message = index.latest().vers.clone();
                diags.push(Diagnostic {
                    range,
                    severity: Some(DiagnosticSeverity::INFORMATION),
                    code: None,
                    code_description: None,
                    source: None,
                    message,
                    related_information: None,
                    tags: None,
                    data: None,
                });
            }
        }

        self.client.publish_diagnostics(uri, diags, None).await;
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, _: InitializeParams) -> jsonrpc::Result<InitializeResult> {
        eprintln!("initialize");
        Ok(InitializeResult {
            server_info: None,
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    // sync the document by sending changes using the
                    // `didChagne` notification.
                    TextDocumentSyncKind::INCREMENTAL,
                )),
                completion_provider: Some(CompletionOptions {
                    resolve_provider: Some(false),
                    trigger_characters: Some(vec!['\"'.to_string()]),
                    work_done_progress_options: Default::default(),
                    all_commit_characters: None,
                    ..Default::default()
                }),
                inlay_hint_provider: Some(OneOf::Left(true)),
                code_action_provider: Some(CodeActionProviderCapability::Simple(true)),
                execute_command_provider: Some(ExecuteCommandOptions {
                    commands: vec!["dummy.do_something".to_string()],
                    work_done_progress_options: Default::default(),
                }),
                workspace: Some(WorkspaceServerCapabilities {
                    workspace_folders: Some(WorkspaceFoldersServerCapabilities {
                        supported: Some(true),
                        change_notifications: Some(OneOf::Left(true)),
                    }),
                    file_operations: None,
                }),

                ..ServerCapabilities::default()
            },
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        eprintln!("Initialized");
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        eprintln!("didOpen");
        self.documents.write().await.insert(
            params.text_document.uri.clone(),
            Rope::from_str(&params.text_document.text),
        );
        self.on_change(params.text_document.uri).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        eprintln!("didChange");
        self.apply_changes(&params.text_document.uri, params.content_changes)
            .await;
        self.on_change(params.text_document.uri).await;
    }

    async fn completion(
        &self,
        params: CompletionParams,
    ) -> jsonrpc::Result<Option<CompletionResponse>> {
        let docs = self.documents.read().await;

        let inside = docs
            .get(&params.text_document_position.text_document.uri)
            .map(|rope| {
                let span = ImDocument::from_str(&rope.to_string())
                    .ok()
                    // TODO: use the constand from the `parse` module.
                    .and_then(|doc| doc.get("dependencies").and_then(|deps| deps.span()));

                let pos = params.text_document_position.position;
                let idx = rope.line_to_char(pos.line as usize) + pos.character as usize;

                span.is_some_and(|r| r.contains(&idx))
            })
            .unwrap_or(false);

        if inside {
            Ok(Some(CompletionResponse::Array(vec![
                CompletionItem::new_simple("indside".to_owned(), "yay".to_owned()),
            ])))
        } else {
            Ok(None)
        }
    }

    async fn shutdown(&self) -> jsonrpc::Result<()> {
        eprintln!("shutDown");
        Ok(())
    }
}

#[tokio::main]
async fn main() {
    let (stdin, stdout) = (tokio::io::stdin(), tokio::io::stdout());

    let (service, socket) = LspService::new(|client| Backend {
        client,
        documents: Default::default(),
    });
    Server::new(stdin, stdout, socket).serve(service).await;
}
