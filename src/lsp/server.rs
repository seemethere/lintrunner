use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tower_lsp::jsonrpc;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer};
use dashmap::DashMap;

use crate::lint_config::{LintRunnerConfig, get_linters_from_configs};
use crate::lint_message::{LintMessage, LintSeverity};
use crate::path::AbsPath;

pub struct LintrunnerLspServer {
    client: Client,
    config: Arc<RwLock<Option<LintRunnerConfig>>>,
    diagnostics: Arc<DashMap<Url, Vec<Diagnostic>>>,
    workspace_root: Arc<RwLock<Option<PathBuf>>>,
}

impl LintrunnerLspServer {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            config: Arc::new(RwLock::new(None)),
            diagnostics: Arc::new(DashMap::new()),
            workspace_root: Arc::new(RwLock::new(None)),
        }
    }

    async fn lint_file(&self, uri: &Url) -> Result<Vec<Diagnostic>> {
        let config_guard = self.config.read().await;
        let config = match config_guard.as_ref() {
            Some(config) => config,
            None => return Ok(vec![]),
        };

        let workspace_root = self.workspace_root.read().await;
        let workspace_root = match workspace_root.as_ref() {
            Some(root) => root,
            None => return Ok(vec![]),
        };

        // Convert URI to file path
        let file_path = match uri.to_file_path() {
            Ok(path) => path,
            Err(_) => return Ok(vec![]),
        };

        let abs_file_path = AbsPath::try_from(file_path)?;

        // Get applicable linters for this file
        let primary_config_path = AbsPath::try_from(workspace_root.join(".lintrunner.toml"))?;
        let linters = get_linters_from_configs(
            &config.linters,
            None, // no skipped linters
            None, // no taken linters
            &primary_config_path,
        )?;

        let mut all_diagnostics = Vec::new();

        // Run each linter on the single file
        for linter in linters {
            let lint_messages = linter.run(&[abs_file_path.clone()]);
            
            for lint_message in lint_messages {
                if let Some(diagnostic) = self.lint_message_to_diagnostic(&lint_message) {
                    all_diagnostics.push(diagnostic);
                }
            }
        }

        Ok(all_diagnostics)
    }

    fn lint_message_to_diagnostic(&self, lint_message: &LintMessage) -> Option<Diagnostic> {
        let range = if let (Some(line), Some(col)) = (lint_message.line, lint_message.char) {
            Range {
                start: Position {
                    line: (line - 1) as u32, // LSP is 0-indexed
                    character: (col - 1) as u32,
                },
                end: Position {
                    line: (line - 1) as u32,
                    character: (col - 1) as u32 + lint_message.name.len() as u32,
                },
            }
        } else {
            // Default to first line if no position info
            Range {
                start: Position { line: 0, character: 0 },
                end: Position { line: 0, character: 0 },
            }
        };

        let severity = match lint_message.severity {
            LintSeverity::Error => DiagnosticSeverity::ERROR,
            LintSeverity::Warning => DiagnosticSeverity::WARNING,
            LintSeverity::Advice => DiagnosticSeverity::INFORMATION,
            LintSeverity::Disabled => return None, // Skip disabled lints
        };

        Some(Diagnostic {
            range,
            severity: Some(severity),
            code: Some(NumberOrString::String(lint_message.code.clone())),
            source: Some("lintrunner".to_string()),
            message: lint_message.description.clone().unwrap_or_else(|| lint_message.name.clone()),
            related_information: None,
            tags: None,
            code_description: None,
            data: None,
        })
    }

    async fn publish_diagnostics(&self, uri: Url) {
        match self.lint_file(&uri).await {
            Ok(diagnostics) => {
                self.diagnostics.insert(uri.clone(), diagnostics.clone());
                self.client
                    .publish_diagnostics(uri, diagnostics, None)
                    .await;
            }
            Err(err) => {
                log::error!("Failed to lint file {}: {}", uri, err);
            }
        }
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for LintrunnerLspServer {
    async fn initialize(&self, params: InitializeParams) -> jsonrpc::Result<InitializeResult> {
        // Store workspace root
        if let Some(workspace_folders) = params.workspace_folders {
            if let Some(folder) = workspace_folders.first() {
                if let Ok(path) = folder.uri.to_file_path() {
                    *self.workspace_root.write().await = Some(path.clone());
                    
                    // Load lintrunner config
                    let config_path = path.join(".lintrunner.toml");
                    if config_path.exists() {
                        match LintRunnerConfig::new(&vec![config_path.to_string_lossy().to_string()]) {
                            Ok(config) => {
                                *self.config.write().await = Some(config);
                                self.client
                                    .log_message(MessageType::INFO, "Lintrunner config loaded")
                                    .await;
                            }
                            Err(err) => {
                                self.client
                                    .log_message(
                                        MessageType::ERROR,
                                        format!("Failed to load lintrunner config: {}", err),
                                    )
                                    .await;
                            }
                        }
                    }
                }
            }
        }

        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                code_action_provider: Some(CodeActionProviderCapability::Simple(true)),
                ..Default::default()
            },
            server_info: Some(ServerInfo {
                name: "lintrunner-lsp".to_string(),
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
            }),
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "Lintrunner LSP server initialized!")
            .await;
    }

    async fn shutdown(&self) -> jsonrpc::Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        self.publish_diagnostics(params.text_document.uri).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        self.publish_diagnostics(params.text_document.uri).await;
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        self.publish_diagnostics(params.text_document.uri).await;
    }

    async fn code_action(&self, params: CodeActionParams) -> jsonrpc::Result<Option<CodeActionResponse>> {
        let uri = &params.text_document.uri;
        
        // Get diagnostics for this file
        let diagnostics = match self.diagnostics.get(uri) {
            Some(diagnostics) => diagnostics.clone(),
            None => return Ok(None),
        };

        let mut code_actions = Vec::new();

        // For each diagnostic in the requested range, check if we can create a code action
        for diagnostic in diagnostics {
            if params.range.start <= diagnostic.range.start && diagnostic.range.end <= params.range.end {
                // Check if this diagnostic has a fix available
                // We'd need to store the original LintMessage to access the replacement
                // For now, we'll create a placeholder code action
                let action = CodeAction {
                    title: format!("Fix: {}", diagnostic.message),
                    kind: Some(CodeActionKind::QUICKFIX),
                    diagnostics: Some(vec![diagnostic.clone()]),
                    edit: None, // TODO: Implement actual workspace edit
                    command: None,
                    is_preferred: Some(true),
                    disabled: None,
                    data: None,
                };
                code_actions.push(CodeActionOrCommand::CodeAction(action));
            }
        }

        if code_actions.is_empty() {
            Ok(None)
        } else {
            Ok(Some(code_actions))
        }
    }
}