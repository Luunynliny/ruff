use lsp_types::notification::PublishDiagnostics;
use lsp_types::{
    CodeDescription, Diagnostic, DiagnosticRelatedInformation, DiagnosticSeverity, DiagnosticTag,
    NumberOrString, PublishDiagnosticsParams, Range, Url,
};
use rustc_hash::FxHashMap;

use ruff_db::diagnostic::{Annotation, Severity, SubDiagnostic};
use ruff_db::files::FileRange;
use ruff_db::source::{line_index, source_text};
use ty_project::{Db, ProjectDatabase};

use crate::document::{DocumentKey, FileRangeExt, ToRangeExt};
use crate::session::DocumentSnapshot;
use crate::session::client::Client;
use crate::{PositionEncoding, Session};

/// Represents the diagnostics for a text document or a notebook document.
pub(super) enum Diagnostics {
    TextDocument(Vec<Diagnostic>),

    /// A map of cell URLs to the diagnostics for that cell.
    NotebookDocument(FxHashMap<Url, Vec<Diagnostic>>),
}

impl Diagnostics {
    /// Returns the diagnostics for a text document.
    ///
    /// # Panics
    ///
    /// Panics if the diagnostics are for a notebook document.
    pub(super) fn expect_text_document(self) -> Vec<Diagnostic> {
        match self {
            Diagnostics::TextDocument(diagnostics) => diagnostics,
            Diagnostics::NotebookDocument(_) => {
                panic!("Expected a text document diagnostics, but got notebook diagnostics")
            }
        }
    }
}

/// Clears the diagnostics for the document identified by `key`.
///
/// This is done by notifying the client with an empty list of diagnostics for the document.
/// For notebook cells, this clears diagnostics for the specific cell.
/// For other document types, this clears diagnostics for the main document.
pub(super) fn clear_diagnostics(key: &DocumentKey, client: &Client) {
    let Some(uri) = key.to_url() else {
        // If we can't convert to URL, we can't clear diagnostics
        return;
    };

    client.send_notification::<PublishDiagnostics>(PublishDiagnosticsParams {
        uri,
        diagnostics: vec![],
        version: None,
    });
}

/// Publishes the diagnostics for the given document snapshot using the [publish diagnostics
/// notification].
///
/// This function is a no-op if the client supports pull diagnostics.
///
/// [publish diagnostics notification]: https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_publishDiagnostics
pub(super) fn publish_diagnostics(session: &Session, key: &DocumentKey, client: &Client) {
    if session.client_capabilities().supports_pull_diagnostics() {
        return;
    }

    let Some(url) = key.to_url() else {
        return;
    };

    let snapshot = session.take_document_snapshot(url.clone());

    let document = match snapshot.document() {
        Ok(document) => document,
        Err(err) => {
            tracing::debug!("Failed to resolve document for URL `{}`: {}", url, err);
            return;
        }
    };

    let db = session.project_db(key.path());

    let Some(diagnostics) = compute_diagnostics(db, &snapshot) else {
        return;
    };

    // Sends a notification to the client with the diagnostics for the document.
    let publish_diagnostics_notification = |uri: Url, diagnostics: Vec<Diagnostic>| {
        client.send_notification::<PublishDiagnostics>(PublishDiagnosticsParams {
            uri,
            diagnostics,
            version: Some(document.version()),
        });
    };

    match diagnostics {
        Diagnostics::TextDocument(diagnostics) => {
            publish_diagnostics_notification(url, diagnostics);
        }
        Diagnostics::NotebookDocument(cell_diagnostics) => {
            for (cell_url, diagnostics) in cell_diagnostics {
                publish_diagnostics_notification(cell_url, diagnostics);
            }
        }
    }
}

pub(super) fn compute_diagnostics(
    db: &ProjectDatabase,
    snapshot: &DocumentSnapshot,
) -> Option<Diagnostics> {
    let document = match snapshot.document() {
        Ok(document) => document,
        Err(err) => {
            tracing::info!("Failed to resolve document for snapshot: {}", err);
            return None;
        }
    };

    let Some(file) = document.file(db) else {
        tracing::info!("No file found for snapshot for `{}`", document.file_path());
        return None;
    };

    let diagnostics = db.check_file(file);

    if let Some(notebook) = document.as_notebook() {
        let mut cell_diagnostics: FxHashMap<Url, Vec<Diagnostic>> = FxHashMap::default();

        // Populates all relevant URLs with an empty diagnostic list. This ensures that documents
        // without diagnostics still get updated.
        for cell_url in notebook.cell_urls() {
            cell_diagnostics.entry(cell_url.clone()).or_default();
        }

        for (cell_index, diagnostic) in diagnostics.iter().map(|diagnostic| {
            (
                // TODO: Use the cell index instead using `SourceKind`
                usize::default(),
                to_lsp_diagnostic(db, diagnostic, snapshot.encoding()),
            )
        }) {
            let Some(cell_uri) = notebook.cell_uri_by_index(cell_index) else {
                tracing::warn!("Unable to find notebook cell at index {cell_index}");
                continue;
            };
            cell_diagnostics
                .entry(cell_uri.clone())
                .or_default()
                .push(diagnostic);
        }

        Some(Diagnostics::NotebookDocument(cell_diagnostics))
    } else {
        Some(Diagnostics::TextDocument(
            diagnostics
                .iter()
                .map(|diagnostic| to_lsp_diagnostic(db, diagnostic, snapshot.encoding()))
                .collect(),
        ))
    }
}

/// Converts the tool specific [`Diagnostic`][ruff_db::diagnostic::Diagnostic] to an LSP
/// [`Diagnostic`].
pub(super) fn to_lsp_diagnostic(
    db: &dyn Db,
    diagnostic: &ruff_db::diagnostic::Diagnostic,
    encoding: PositionEncoding,
) -> Diagnostic {
    let range = if let Some(span) = diagnostic.primary_span() {
        let file = span.expect_ty_file();
        let index = line_index(db, file);
        let source = source_text(db, file);

        span.range()
            .map(|range| range.to_lsp_range(&source, &index, encoding))
            .unwrap_or_default()
    } else {
        Range::default()
    };

    let severity = match diagnostic.severity() {
        Severity::Info => DiagnosticSeverity::INFORMATION,
        Severity::Warning => DiagnosticSeverity::WARNING,
        Severity::Error | Severity::Fatal => DiagnosticSeverity::ERROR,
    };

    let tags = diagnostic
        .primary_tags()
        .map(|tags| {
            tags.iter()
                .map(|tag| match tag {
                    ruff_db::diagnostic::DiagnosticTag::Unnecessary => DiagnosticTag::UNNECESSARY,
                    ruff_db::diagnostic::DiagnosticTag::Deprecated => DiagnosticTag::DEPRECATED,
                })
                .collect::<Vec<DiagnosticTag>>()
        })
        .filter(|mapped_tags| !mapped_tags.is_empty());

    let code_description = diagnostic
        .id()
        .is_lint()
        .then(|| {
            Some(CodeDescription {
                href: Url::parse(&format!("https://ty.dev/rules#{}", diagnostic.id())).ok()?,
            })
        })
        .flatten();

    let mut related_information = Vec::new();

    related_information.extend(
        diagnostic
            .secondary_annotations()
            .filter_map(|annotation| annotation_to_related_information(db, annotation, encoding)),
    );

    for sub_diagnostic in diagnostic.sub_diagnostics() {
        related_information.extend(sub_diagnostic_to_related_information(
            db,
            sub_diagnostic,
            encoding,
        ));

        related_information.extend(
            sub_diagnostic
                .annotations()
                .iter()
                .filter_map(|annotation| {
                    annotation_to_related_information(db, annotation, encoding)
                }),
        );
    }

    Diagnostic {
        range,
        severity: Some(severity),
        tags,
        code: Some(NumberOrString::String(diagnostic.id().to_string())),
        code_description,
        source: Some("ty".into()),
        message: diagnostic.concise_message().to_string(),
        related_information: Some(related_information),
        data: None,
    }
}

/// Converts an [`Annotation`] to a [`DiagnosticRelatedInformation`].
fn annotation_to_related_information(
    db: &dyn Db,
    annotation: &Annotation,
    encoding: PositionEncoding,
) -> Option<DiagnosticRelatedInformation> {
    let span = annotation.get_span();

    let annotation_message = annotation.get_message()?;
    let range = FileRange::try_from(span).ok()?;
    let location = range.to_location(db, encoding)?;

    Some(DiagnosticRelatedInformation {
        location,
        message: annotation_message.to_string(),
    })
}

/// Converts a [`SubDiagnostic`] to a [`DiagnosticRelatedInformation`].
fn sub_diagnostic_to_related_information(
    db: &dyn Db,
    diagnostic: &SubDiagnostic,
    encoding: PositionEncoding,
) -> Option<DiagnosticRelatedInformation> {
    let primary_annotation = diagnostic.primary_annotation()?;

    let span = primary_annotation.get_span();
    let range = FileRange::try_from(span).ok()?;
    let location = range.to_location(db, encoding)?;

    Some(DiagnosticRelatedInformation {
        location,
        message: diagnostic.concise_message().to_string(),
    })
}
