#![recursion_limit = "512"]

pub mod agent_format;
pub mod data;
pub mod document_body;
pub mod document_source;
pub mod document_structure;
pub mod documents;
pub mod excel;
pub mod fonts;
pub mod native_text;
pub mod ocr;
mod office_com;
mod office_worker;
pub mod project;
pub mod registry;
pub mod rights_management;
pub mod semantic;
pub mod semantic_contract;
pub mod semantic_operations;
pub mod skills;
pub mod source_impact;
pub mod specification_ids;
pub mod specifications;
pub mod workflow;

pub fn schemas() -> serde_json::Value {
    let mut schemas: serde_json::Value =
        serde_json::from_str(include_str!("../../../contracts/document-schemas.json"))
            .expect("embedded document schemas must be valid JSON");
    let search: serde_json::Value = serde_json::from_str(include_str!(
        "../../../contracts/document-search-schemas.json"
    ))
    .expect("embedded search schemas must be valid JSON");
    schemas
        .as_object_mut()
        .unwrap()
        .extend(search.as_object().unwrap().clone());
    schemas
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
            "document_structure_batch_edits": true,
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
            "document_structure_apply_carry": true,
            "excel_screenshot_rendering": true,
            "excel_screenshot_engine": "excel-com (Windows with desktop Microsoft Excel)",
            "documents_workflow": true,
            "documents_typed_value_edits": "Office/PDF existing cells and text; Excel scalar values, existing formulas, table labels and shape text; exact base and old value; before structural operations",
            "document_fonts": "effective latin and East Asian font names, size in points and RRGGBB color per cell, paragraph and shape text, resolved through styles and themes; mixed when runs differ; PDF strings read-only (font name without subset prefix, size on the page, device colors)",
            "documents_font_edits": "Excel cells (one font name for latin and East Asian text) and shape text, Word paragraphs and table cells (sizes in half points), PowerPoint paragraphs and table cells (RRGGBB colors) through documents values; exact base and old font properties",
            "documents_apply_export_confirmation": "human layout review bound to exact candidate and report hashes",
            "excel_rich_text_writeback": "unambiguous edits within one run; cross-run edits refused",
            "excel_value_writeback_preservation_check": true,
            "document_ids": "source path below the sources folder",
            "documents_folder_import": true,
            "documents_import_unprotect": "encrypted Office originals are saved over unencrypted before import: password encryption (Agile, Standard AES) with passwords from --password-stdin; IRM and sensitivity labels by desktop Office on Windows with the signed-in account's rights",
            "documents_batch_steps": ["record", "adopt", "review"],
            "documents_remove": true,
            "documents_search": true,
            "documents_search_batch": true,
            "documents_search_refresh": "explicit documents search-refresh command",
            "documents_search_freshness": "saved index snapshot; external changes remain unchecked until search-refresh",
            "documents_search_engine": "SQLite FTS5 BM25; TiniestSegmenter Japanese segmentation; Unicode normalization; character bigrams; explicit synonyms",
            "documents_search_scope": "adopted extraction with structure context; proposals and unexported content edits are not indexed",
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
            "excel_chart_xml_extraction": true,
            "excel_opaque_part_inventory": true,
            "print_settings_extraction": true,
            "office_unchanged_part_verification": true,
            "excel_table_structure_inference": true,
            "excel_writeback": true,
            "excel_structural_writeback": true,
            "excel_element_body_version": "4",
            "excel_body_model": "reading-order value arrays; identities, topology, formula kinds and physical bindings in layout.yml",
            "excel_external_reimport": "unchanged grid with at most one changed cell; ambiguous structural or multi-cell correspondence is refused",
            "excel_column_move_scope": "plain worksheets and ordinary contiguous A1 formulas; positional objects, defined names and mixed structural operations are refused",
            "excel_row_column_commands": ["rows insert", "rows delete", "columns insert", "columns delete", "columns move"],
            "excel_image_writeback": true,
            "word_import": true,
            "word_embedded_image_extraction": true,
            "word_field_code_extraction": true,
            "word_writeback": true,
            "pptx_import": true,
            "pptx_writeback": true,
            "pptx_slide_commands": ["slides insert", "slides add", "slides delete", "slides move", "slides hide", "slides show"],
            "pptx_shape_commands": ["shapes update", "shapes add", "shapes delete", "shapes add-picture", "shapes replace-picture", "shapes add-connector"],
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
            "pptx_drawing_extraction": "shapes, pictures (saved as assets), connectors, groups and graphic frames with places on the slide in points, fill, line and placeholder",
            "ocr_engine": "Windows.Media.Ocr (imported Excel, Word and PowerPoint images on Windows)"
        },
        "limitations": ["Excel supports scalar/structural writeback, renaming table columns and totals labels from their cells (structured references follow), replacing existing formulas (file notation with _xlfn. prefixes; spilling functions, LET/LAMBDA, # and @ are refused) and PNG insertion. Windows OCR runs automatically on imported Excel, Word and PowerPoint image assets; unsupported images retain an unavailable reason. Word (DOCX/DOCM/DOTX/DOTM) and PPTX lay paragraphs and table cells out as rows and edit the runs a change touches; Word field codes are extracted read-only and field results are read-only because Word recalculates them; plain text content controls bound to document data write the data and the controls sharing it. Import removes the encryption of Office originals and saves them unencrypted (passwords from --password-stdin; IRM and sensitivity labels only by desktop Office on Windows with the right to remove them); binary (.xls/.xlsb/.doc/.ppt) and Strict Open XML files are rejected with resave guidance. Excel chart, dialog and macro sheets are not extracted and are preserved unchanged; print settings and chart XML are extracted read-only, while ActiveX binary parts are inventoried by hash; row/column edits move notes, threaded comments, form controls and embedded objects, and are rejected for workbooks with a VBA project, macro or dialog sheets or ActiveX controls, and where Excel itself refuses them (table header/totals rows, cutting through pivot tables or array formulas); PDF supports page text-show strings using original font encodings. Exports retain the source format; cross-format conversion, PDF page insertion/deletion (Word and PowerPoint paragraphs and table rows take row operations; PowerPoint slides are copied, with their notes but not their comments, added from a slide layout with empty placeholders, deleted, moved in the show order (custom shows keep their order) and hidden or shown with slide operations; deletion is refused while another slide links to a deleted slide or a custom show would be left empty), OCR for PDF images, PDF Form XObject text, annotations, slide masters are not supported, Excel shapes are edited in their text and fonts only, and PowerPoint shapes holding the slide's text are not deleted (PowerPoint shapes, PNG pictures and connectors between rectangles, rounded rectangles and ellipses are changed, added and deleted with shape operations) (slide notes are extracted and written back as notes-N). Text edits require visual layout review; Word and PowerPoint edits may add, remove or replace line breaks and tabs within a paragraph but cannot split or join paragraphs, and PDF rejects line breaks/tabs; PDF does not reflow text and rejects unavailable font characters. Structural edits move references like Excel does: formulas on every sheet, defined names, conditional formats, validations, tables, pivot sources, chart series, sparklines, merges, column widths and drawing anchors; Excel performs recalculation. Content rows and columns an operation adds are keyed <operation ID>-<n>; values a grown merge would hide are rejected. Specifications use agent-authored models and Markdown rendering; semantic completeness requires review. Empty-Windows acceptance remains unverified."]
    })
}
