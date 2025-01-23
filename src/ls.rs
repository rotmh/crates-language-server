use std::{collections::HashMap, str::FromStr, sync::Arc};

use crate::{
    crates::{self, DOCS_RS_URL},
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
        DidOpenTextDocumentParams, ExecuteCommandOptions, GotoDefinitionParams,
        GotoDefinitionResponse, InitializeParams, InitializeResult, InitializedParams, OneOf,
        Position, ServerCapabilities, ShowDocumentParams, TextDocumentContentChangeEvent,
        TextDocumentSyncCapability, TextDocumentSyncKind, WorkspaceFoldersServerCapabilities,
        WorkspaceServerCapabilities,
    },
};
use url::Url;

fn completions(index: crates::Index) -> Vec<CompletionItem> {
    let Ok(latest) = semver::Version::parse(&index.latest().vers) else {
        return Vec::new();
    };

    let mut comps = vec![
        CompletionItem::new_simple(
            format!("{}.{}.{}", latest.major, latest.minor, latest.patch),
            "patch".to_owned(),
        ),
        CompletionItem::new_simple(
            format!("{}.{}", latest.major, latest.minor),
            "minor".to_owned(),
        ),
        CompletionItem::new_simple(format!("{}", latest.major), "major".to_owned()),
    ];

    if !(latest.pre.is_empty() && latest.build.is_empty()) {
        let full = CompletionItem::new_simple(latest.to_string(), "latest".to_owned());
        comps.insert(0, full);
    }

    comps
}

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
                // We want to keep a synced version of the documents
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    // sync the document by sending changes using the
                    // `didChagne` notification.
                    TextDocumentSyncKind::INCREMENTAL,
                )),

                // We provide completions events
                completion_provider: Some(CompletionOptions {
                    // trigger completion event when the user hits `"`
                    trigger_characters: Some(vec!['\"'.to_string()]),
                    resolve_provider: Some(false),
                    ..Default::default()
                }),

                // We provide inlay hints
                //
                // TODO: uncomment when implemented
                // inlay_hint_provider: Some(OneOf::Left(true)),

                // We provide code action events
                code_action_provider: Some(CodeActionProviderCapability::Simple(true)),

                // We provide goto definition events
                definition_provider: Some(OneOf::Left(true)),

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
        let uri = params.text_document_position.text_document.uri;
        let pos = params.text_document_position.position;
        let name = self
            .doc(&uri)
            .await
            .and_then(|doc| parse::pos_in_dependency_version(&doc, pos));
        let comps = if let Some(name) = name {
            crates::fetch(&name)
                .await
                .ok()
                .map(completions)
                .map(CompletionResponse::Array)
        } else {
            None
        };

        Ok(comps)
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> jsonrpc::Result<Option<GotoDefinitionResponse>> {
        let uri = params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;
        let name = self
            .doc(&uri)
            .await
            .and_then(|doc| parse::pos_in_dependency_name(&doc, pos));

        if let Some(name) = name
            && crates::is_availabe(&name).await
        {
            let crate_docs_url = format!("{DOCS_RS_URL}/crate/{name}");
            let uri = Url::parse(&crate_docs_url).expect("url string should be valid");
            self.client
                .show_document(ShowDocumentParams {
                    uri,
                    external: Some(true),
                    take_focus: None,
                    selection: None,
                })
                .await
                .ok();
        }

        Ok(None)
    }

    async fn shutdown(&self) -> jsonrpc::Result<()> {
        Ok(())
    }
}
