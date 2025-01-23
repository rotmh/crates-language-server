use std::{collections::HashMap, str::FromStr, sync::Arc};

use crate::{
    crates,
    parse::{self, DEPENDENCIES_KEY, Dependencies},
};
use ropey::Rope;
use tokio::sync::RwLock;
use toml_edit::{ImDocument, Item, Table, Value};
use tower_lsp::{
    Client, LanguageServer, jsonrpc,
    lsp_types::{
        CodeActionProviderCapability, CompletionItem, CompletionOptions, CompletionParams,
        CompletionResponse, Diagnostic, DiagnosticSeverity, DidChangeTextDocumentParams,
        DidOpenTextDocumentParams, ExecuteCommandOptions, InitializeParams, InitializeResult,
        InitializedParams, OneOf, Position, ServerCapabilities, TextDocumentContentChangeEvent,
        TextDocumentSyncCapability, TextDocumentSyncKind, WorkspaceFoldersServerCapabilities,
        WorkspaceServerCapabilities,
    },
};
use url::Url;

#[derive(Debug)]
pub struct Backend {
    client: Client,
    documents: Arc<RwLock<HashMap<Url, Rope>>>,
}

impl Backend {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            documents: Default::default(),
        }
    }

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

    async fn doc(&self, uri: &Url) -> Option<String> {
        let docs = self.documents.read().await;
        docs.get(uri).map(|rope| rope.to_string())
    }

    async fn parse_dependencies(&self, uri: &Url) -> Result<Dependencies, ()> {
        self.doc(uri)
            .await
            .ok_or(())
            .and_then(|doc| Dependencies::from_str(&doc).map_err(|_| ()))
    }

    async fn on_change(&self, uri: Url) {
        let Ok(dependencies) = self.parse_dependencies(&uri).await else {
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

    async fn initialized(&self, _: InitializedParams) {}

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        self.documents.write().await.insert(
            params.text_document.uri.clone(),
            Rope::from_str(&params.text_document.text),
        );
        self.on_change(params.text_document.uri).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        self.apply_changes(&params.text_document.uri, params.content_changes)
            .await;
        self.on_change(params.text_document.uri).await;
    }

    async fn completion(
        &self,
        params: CompletionParams,
    ) -> jsonrpc::Result<Option<CompletionResponse>> {
        let uri = &params.text_document_position.text_document.uri;
        let pos = params.text_document_position.position;
        let name = self
            .doc(uri)
            .await
            .and_then(|doc| parse::pos_in_dependency_version(&doc, pos));
        let completions = if let Some(name) = name {
            crates::fetch(&name).await.ok().map(|index| {
                let label = index.latest().vers.clone();
                let detail = format!("{name}\n\n`@latest`");
                let completion = CompletionItem::new_simple(label, detail);
                CompletionResponse::Array(vec![completion])
            })
        } else {
            None
        };

        Ok(completions)
    }

    async fn shutdown(&self) -> jsonrpc::Result<()> {
        Ok(())
    }
}
