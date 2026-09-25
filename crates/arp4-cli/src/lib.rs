#![recursion_limit = "256"]

pub mod agent_format;
pub mod data;
pub mod document_source;
pub mod document_structure;
pub mod documents;
pub mod excel;
pub mod native_text;
pub mod ocr;
pub mod project;
pub mod registry;
pub mod semantic;
pub mod semantic_contract;
pub mod semantic_operations;
pub mod skills;
pub mod source_impact;
pub mod specification_ids;
pub mod specifications;
pub mod workflow;

pub fn schemas() -> serde_json::Value {
    serde_json::from_str(include_str!("../../../contracts/document-schemas.json"))
        .expect("embedded document schemas must be valid JSON")
}

pub fn capabilities() -> serde_json::Value {
    serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "implementation": "rust",
        "release_ready": false,
        "capabilities": {
            "skills_install": true,
            "documents_schema": true,
            "document_structure": true,
            "document_structure_formats": document_source::structure_formats(),
            "document_structure_requirement_policy": document_structure::requirement_policy(),
            "document_structure_external_ocr_records": true,
            "document_structure_image_regions": true,
            "document_structure_compact_drawings": true,
            "document_structure_visual_graphs": true,
            "document_structure_table_descriptions": true,
            "document_structure_imported_image_paths": true,
            "document_structure_corrections_in_document": true,
            "document_structure_writeback": false,
            "excel_screenshot_rendering": true,
            "excel_screenshot_engine": "excel-com (Windows with desktop Microsoft Excel)",
            "documents_workflow": true,
            "specifications": true,
            "semantic_workflow": true,
            "agent_read_formats": ["toon", "json"],
            "agent_read_format_default": "toon",
            "semantic_workflow_handoff": true,
            "semantic_workflow_status_summary": true,
            "semantic_workflow_receipts": true,
            "semantic_workflow_confirmation_delivery": true,
            "semantic_workflow_record_usage": true,
            "semantic_workflow_concerns": true,
            "semantic_workflow_combined_repairs": true,
            "semantic_workflow_providers": ["claude-code", "command"],
            "specification_registry": true,
            "excel_import": true,
            "excel_drawing_extraction": true,
            "excel_embedded_image_extraction": true,
            "excel_table_structure_inference": true,
            "excel_writeback": true,
            "excel_structural_writeback": true,
            "excel_image_writeback": true,
            "word_import": true,
            "word_writeback": true,
            "pptx_import": true,
            "pptx_writeback": true,
            "pdf_import": true,
            "pdf_writeback": true,
            "text_import": true,
            "markdown_import": true,
            "markdown_structure": true,
            "source_change_tracking": true,
            "csv_import": true,
            "tsv_import": true,
            "native_text_writeback": false,
            "native_text_encodings": ["UTF-8", "UTF-8-BOM"],
            "document_input_formats": document_source::input_formats(),
            "document_output": "same format as source (Office/PDF only; native text uses direct editing and re-import)",
            "text_run_writeback": true,
            "noncell_extraction": true,
            "ocr": true,
            "ocr_engine": "Windows.Media.Ocr (imported Excel images on Windows)"
        },
        "limitations": ["Excel supports scalar/structural writeback and PNG insertion. Windows OCR runs automatically on imported Excel image assets; unsupported images retain an unavailable reason. Word (DOCX/DOCM/DOTX/DOTM) and PPTX support existing XML text runs; Word field results are read-only because Word recalculates them. Encrypted (password, IRM, sensitivity label), binary (.xls/.xlsb/.doc/.ppt) and Strict Open XML files are rejected with resave guidance. Excel chart, dialog and macro sheets are not extracted and are preserved unchanged; row/column edits are rejected for workbooks with a VBA project, macro or dialog sheets, or form controls/comments/embedded objects on the edited sheet, and where Excel itself refuses them (table header/totals rows, cutting through pivot tables or array formulas); PDF supports page text-show strings using original font encodings. Exports retain the source format; cross-format conversion, text-container insertion/deletion, OCR for DOCX/PPTX/PDF images, PDF Form XObject text, annotations, slide notes/masters and shape editing are not supported. Text edits require visual layout review and reject line breaks/tabs; PDF does not reflow text and rejects unavailable font characters. Structural edits move references like Excel does: formulas on every sheet, defined names, conditional formats, validations, tables, pivot sources, chart series, sparklines, merges, column widths and drawing anchors; Excel performs recalculation. Specifications use agent-authored models and Markdown rendering; semantic completeness requires review. Empty-Windows acceptance remains unverified."]
    })
}
