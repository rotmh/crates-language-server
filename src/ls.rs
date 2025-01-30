use std::{collections::HashMap, sync::Arc};

use crate::{
    crates::{self, DOCS_RS_URL},
    format,
    parse::{DEPENDENCIES_KEYS, Dependency},
};
use ropey::Rope;
use taplo::dom;
use tokio::sync::RwLock;
use tower_lsp::{
    Client, LanguageServer, jsonrpc,
    lsp_types::{
        CodeActionOrCommand, CodeActionParams, CodeActionProviderCapability, CodeActionResponse,
        Command, CompletionItem, CompletionOptions, CompletionParams, CompletionResponse,
        Diagnostic, DiagnosticSeverity, DidChangeTextDocumentParams, DidOpenTextDocumentParams,
        ExecuteCommandOptions, ExecuteCommandParams, GotoDefinitionParams, GotoDefinitionResponse,
        Hover, HoverContents, HoverParams, HoverProviderCapability, InitializeParams,
        InitializeResult, MarkupContent, MarkupKind, MessageType, OneOf, Range, ServerCapabilities,
        ShowDocumentParams, TextDocumentContentChangeEvent, TextDocumentSyncCapability,
        TextDocumentSyncKind, TextEdit, WorkDoneProgressOptions, WorkspaceEdit,
    },
};
use url::Url;

pub mod code_action {
    pub const LATEST_VERSION: &str = "latest_version";
}

#[derive(Debug)]
pub struct Backend {
    client: Client,
    documents: Arc<RwLock<HashMap<Url, Rope>>>,
    manifests: Arc<RwLock<HashMap<Url, Vec<Dependency>>>>,
    registry: crates::RegistryCache,
}

impl Backend {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            documents: Default::default(),
            manifests: Default::default(),
            registry: Default::default(),
        }
    }

    async fn apply_changes(&self, uri: &Url, changes: Vec<TextDocumentContentChangeEvent>) {
        if let Some(doc) = self.documents.write().await.get_mut(uri) {
            // According to the [LSP spec]:
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

    async fn update_manifest(&self, uri: Url) {
        if let Some(doc) = self.documents.read().await.get(&uri).map(Rope::to_string) {
            // NOTE: we must parse the document in a separate function as the
            // `Node` type does not implement the `Send` trait.
            let deps = self.parse_document(&doc);

            self.manifests.write().await.insert(uri, deps);
        }
    }

    fn parse_document(&self, doc: &str) -> Vec<Dependency> {
        fn parse_dependencies(table: &dom::node::Table, doc: &str) -> Vec<Dependency> {
            table
                .entries()
                .read()
                .iter()
                .flat_map(|(key, node)| Dependency::parse(doc, key, node))
                .collect::<Vec<_>>()
        }

        let dom = taplo::parser::parse(doc).into_dom();

        let deps = DEPENDENCIES_KEYS
            .iter()
            .filter_map(|&key| dom.as_table().and_then(|t| t.get(key)))
            .collect::<Vec<_>>();

        deps.iter()
            .filter_map(|deps| deps.as_table())
            .flat_map(|table| parse_dependencies(table, doc))
            .collect()
    }

    async fn generate_diagnostics(&self, dependency: &Dependency) -> Vec<Diagnostic> {
        if let Ok(latest) = self.registry.fetch(&dependency.name.value).await {
            let mut diags = Vec::new();

            // Latest version hint
            if let Some(current_version) = &dependency.version
                // We don't want to hint latest version, if the user already
                // uses the latest version in their manifest.
                && current_version.value.as_ref().is_none_or(|v| *v != latest.version)
            {
                diags.push(Diagnostic {
                    range: current_version.range,
                    severity: Some(DiagnosticSeverity::INFORMATION),
                    code: None,
                    code_description: None,
                    source: None,
                    message: latest.version.to_string(),
                    related_information: None,
                    tags: None,
                    data: None,
                });
            }

            // Non-existant features
            if let Some(available_features) = latest
                .features
                .as_ref()
                .map(HashMap::keys)
                .map(|f| f.collect::<Vec<_>>())
                && let Some(features) = &dependency.features
            {
                for feature in features.iter() {
                    if !available_features.contains(&&feature.value) {
                        diags.push(Diagnostic {
                            range: feature.range,
                            severity: Some(DiagnosticSeverity::ERROR),
                            code: None,
                            code_description: None,
                            source: None,
                            message: format!(
                                "No such feature available for crate `{}`",
                                &dependency.name.value
                            ),
                            related_information: None,
                            tags: None,
                            data: None,
                        });
                    }
                }
            }

            diags
        } else {
            vec![Diagnostic {
                range: dependency.name.range,
                severity: Some(DiagnosticSeverity::ERROR),
                code: None,
                code_description: None,
                source: None,
                message: "No such crate in crates.io".to_owned(),
                related_information: None,
                tags: None,
                data: None,
            }]
        }
    }

    async fn publish_diagnostics(&self, uri: Url) {
        let manifests = self.manifests.read().await;
        let Some(dependencies) = manifests.get(&uri) else {
            return;
        };

        let mut diags = Vec::new();

        for dependency in dependencies.iter() {
            diags.push(self.generate_diagnostics(dependency).await);
        }

        let diags = diags.into_iter().flatten().collect();

        self.client.publish_diagnostics(uri, diags, None).await;
    }

    async fn generate_completion<F>(&self, name: &str, f: F) -> Option<CompletionResponse>
    where
        F: Fn(crates::Latest) -> Vec<CompletionItem>,
    {
        self.registry
            .fetch(name)
            .await
            .ok()
            .map(f)
            .map(CompletionResponse::Array)
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

                // We provide hover events
                hover_provider: Some(HoverProviderCapability::Simple(true)),

                // We provide code action events
                code_action_provider: Some(CodeActionProviderCapability::Simple(true)),

                // We provide goto definition events
                definition_provider: Some(OneOf::Left(true)),

                execute_command_provider: Some(ExecuteCommandOptions {
                    commands: vec![code_action::LATEST_VERSION.to_owned()],
                    work_done_progress_options: WorkDoneProgressOptions {
                        work_done_progress: None,
                    },
                }),

                ..ServerCapabilities::default()
            },
        })
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri;
        let text = Rope::from_str(&params.text_document.text);
        self.documents.write().await.insert(uri.clone(), text);
        self.update_manifest(uri.clone()).await;
        self.publish_diagnostics(uri).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;
        self.apply_changes(&uri, params.content_changes).await;
        self.update_manifest(uri.clone()).await;
        self.publish_diagnostics(uri).await;
    }

    async fn completion(
        &self,
        params: CompletionParams,
    ) -> jsonrpc::Result<Option<CompletionResponse>> {
        let uri = params.text_document_position.text_document.uri;
        let pos = params.text_document_position.position;
        let manifests = self.manifests.read().await;
        let Some(dependencies) = manifests.get(&uri) else {
            return Ok(None);
        };

        for dependecy in dependencies.iter() {
            if dependecy
                .version
                .as_ref()
                .is_some_and(|v| v.contains_pos(pos))
            {
                let name = &dependecy.name.value;
                let comps = self
                    .generate_completion(name, format::version_completions)
                    .await;
                return Ok(comps);
            } else if dependecy
                .features
                .as_ref()
                .is_some_and(|f| f.iter().any(|f| f.contains_pos(pos)))
            {
                let name = &dependecy.name.value;
                let comps = self
                    .generate_completion(name, |latest| {
                        format::features_completions(dependecy, latest)
                    })
                    .await;
                return Ok(comps);
            }
        }

        Ok(None)
    }

    async fn hover(&self, params: HoverParams) -> jsonrpc::Result<Option<Hover>> {
        let uri = params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;
        let manifests = self.manifests.read().await;
        let Some(dependencies) = manifests.get(&uri) else {
            return Ok(None);
        };

        let hover = if let Some(name) = dependencies
            .iter()
            .find_map(|d| d.name.contains_pos(pos).then_some(&d.name))
            && let Ok(latest) = self.registry.fetch(&name.value).await
        {
            // Hovering over a dependency name

            Some(Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::PlainText,
                    value: format::format_name_hover(&name.value, latest),
                }),
                range: Some(name.range),
            })
        } else if let Some((name, feature)) = dependencies.iter().find_map(|d| {
            // Hovering over some feature

            let feature = d
                .features
                .as_ref()
                .and_then(|f| f.iter().find(|f| f.contains_pos(pos)));
            feature.map(|f| (&d.name.value, f))
        }) && let Ok(latest) = self.registry.fetch(name).await
            && let Some(features) = latest.features
            && let Some(feature_description) = features.get(&feature.value)
        {
            Some(Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::PlainText,
                    value: format::format_feature_hover(&feature.value, feature_description),
                }),
                range: Some(feature.range),
            })
        } else {
            None
        };

        Ok(hover)
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> jsonrpc::Result<Option<GotoDefinitionResponse>> {
        let uri = params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;
        let manifests = self.manifests.read().await;
        let Some(dependencies) = manifests.get(&uri) else {
            return Ok(None);
        };

        if let Some(name) = dependencies
            .iter()
            .find_map(|d| d.name.contains_pos(pos).then_some(&d.name.value))
            && self.registry.is_availabe(name).await
            && let Ok(uri) = Url::parse(&format!("{DOCS_RS_URL}/{name}"))
        {
            // The prefered method to tell the client to open a page in the
            // browser is returning here a `GotoDefinitionResponse::Scalar`
            // with a HTTP link. But because helix does not support this at
            // time of writing, we'll use this for now.
            let params = ShowDocumentParams {
                uri,
                external: Some(true),
                take_focus: None,
                selection: None,
            };
            let was_shown = self.client.show_document(params).await;

            if matches!(was_shown, Ok(true)) {
                let msg = format!("opened docs for `{name}` in your browser");
                self.client.show_message(MessageType::INFO, msg).await;
            }
        }

        Ok(None)
    }

    async fn code_action(
        &self,
        params: CodeActionParams,
    ) -> jsonrpc::Result<Option<CodeActionResponse>> {
        let uri = params.text_document.uri;
        let Range { start, end } = params.range;
        let manifests = self.manifests.read().await;
        let Some(dependencies) = manifests.get(&uri) else {
            return Ok(None);
        };

        let dependency = dependencies.iter().find(|d| {
            d.version
                .as_ref()
                .is_some_and(|v| v.contains_pos(start) || v.contains_pos(end))
        });

        if let Some(dependency) = dependency
            && let Ok(latest) = self.registry.fetch(&dependency.name.value).await
        {
            let current_version = dependency.version.as_ref().and_then(|v| v.value.as_ref());
            // We don't want to suggest the LATEST_VERSION code action if
            // the user already uses the latest version in their manifest.
            if current_version.is_none_or(|v| *v != latest.version) {
                let command = CodeActionOrCommand::Command(Command::new(
                    "Latest version".to_owned(),
                    code_action::LATEST_VERSION.to_owned(),
                    Some(vec![
                        serde_json::Value::String(dependency.name.value.to_owned()),
                        serde_json::Value::String(uri.into()),
                    ]),
                ));
                return Ok(Some(vec![command]));
            }
        }

        Ok(None)
    }

    async fn execute_command(
        &self,
        params: ExecuteCommandParams,
    ) -> jsonrpc::Result<Option<serde_json::Value>> {
        if params.command == code_action::LATEST_VERSION
            && let Some(serde_json::Value::String(name)) = params.arguments.first()
            && let Some(serde_json::Value::String(uri)) = params.arguments.get(1)
            && let Ok(uri) = Url::parse(uri)
            && let Some(range) = self.manifests.read().await.get(&uri).and_then(|deps| {
                deps.iter()
                    .find(|d| &d.name.value == name)
                    .and_then(|d| d.version.as_ref().map(|v| v.range))
            })
            && let Ok(latest) = self.registry.fetch(name).await
        {
            let change = TextEdit::new(range, format!("\"{}\"", latest.version));
            let changes = WorkspaceEdit::new(std::iter::once((uri, vec![change])).collect());
            let _ = self.client.apply_edit(changes).await;
            Ok(None)
        } else {
            Err(jsonrpc::Error::invalid_request())
        }
    }

    async fn shutdown(&self) -> jsonrpc::Result<()> {
        Ok(())
    }
}
