mod crates;

use std::collections::BTreeMap;

use cargo_util_schemas::manifest::{InheritableDependency, PackageName};
use serde::Deserialize;
use tower_lsp::{
    Client, LanguageServer, LspService, Server, jsonrpc,
    lsp_types::{
        self, Diagnostic, DiagnosticOptions, DiagnosticServerCapabilities, DiagnosticSeverity,
        DidOpenTextDocumentParams, InitializeParams, InitializeResult, InitializedParams,
        InlayHintOptions, InlayHintServerCapabilities, OneOf, Position, ServerCapabilities,
        TextDocumentItem, TextDocumentSyncCapability, TextDocumentSyncKind,
        TextDocumentSyncOptions, WorkDoneProgressOptions,
    },
};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct CargoManifest {
    pub dependencies: Option<BTreeMap<PackageName, InheritableDependency>>,
}

#[derive(Debug)]
struct Backend {
    client: Client,
}

impl Backend {
    async fn on_change(&self, params: TextDocumentItem) {
        let Ok(CargoManifest {
            dependencies: Some(dependencies),
        }) = toml::from_str::<CargoManifest>(&params.text)
        else {
            return;
        };

        let mut diags = Vec::new();

        for (name, _) in dependencies {
            if let Ok(index) = crates::fetch(name.as_str()).await {
                let lines = params.text.lines().count();
                let idx = params.text.find(name.as_str()).unwrap();
                let range = lsp_types::Range {
                    start: Position {
                        line: (idx / lines) as u32,
                        character: (idx % lines) as u32,
                    },
                    end: Position {
                        line: (idx / lines) as u32,
                        character: (idx % lines) as u32 + 1,
                    },
                };
                let message = index.latest().vers.clone();
                eprintln!("Added {message}");
                diags.push(Diagnostic {
                    range,
                    severity: Some(DiagnosticSeverity::HINT),
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

        self.client
            .publish_diagnostics(params.uri, diags, Some(params.version))
            .await;
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, _: InitializeParams) -> jsonrpc::Result<InitializeResult> {
        eprintln!("initialize");
        Ok(InitializeResult {
            server_info: None,
            capabilities: ServerCapabilities {
                // definition_provider: Some(OneOf::Left(true)),
                inlay_hint_provider: Some(OneOf::Right(InlayHintServerCapabilities::Options(
                    InlayHintOptions {
                        work_done_progress_options: Default::default(),
                        resolve_provider: None,
                    },
                ))),
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::INCREMENTAL,
                )),
                // completion_provider: Some(CompletionOptions {
                //     resolve_provider: Some(false),
                //     trigger_characters: Some(vec![".".to_string()]),
                //     work_done_progress_options: Default::default(),
                //     all_commit_characters: None,
                //     ..Default::default()
                // }),
                // execute_command_provider: Some(ExecuteCommandOptions {
                //     commands: vec!["dummy.do_something".to_string()],
                //     work_done_progress_options: Default::default(),
                // }),
                // workspace: Some(WorkspaceServerCapabilities {
                //     workspace_folders: Some(WorkspaceFoldersServerCapabilities {
                //         supported: Some(true),
                //         change_notifications: Some(OneOf::Left(true)),
                //     }),
                //     file_operations: None,
                // }),
                ..ServerCapabilities::default()
            },
            ..Default::default()
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        eprintln!("Initialized");
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        eprintln!("didOpen");
        // self.on_change(params.text_document).await;
    }

    async fn shutdown(&self) -> jsonrpc::Result<()> {
        eprintln!("shutDown");
        Ok(())
    }
}

#[tokio::main]
async fn main() {
    let (stdin, stdout) = (tokio::io::stdin(), tokio::io::stdout());

    let (service, socket) = LspService::new(|client| Backend { client });
    Server::new(stdin, stdout, socket).serve(service).await;
}
