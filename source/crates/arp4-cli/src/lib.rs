pub mod data;
pub mod documents;
pub mod excel;
pub mod skills;

pub fn schemas() -> serde_json::Value {
    serde_json::from_str(include_str!("../../../contracts/document-schemas.json"))
        .expect("embedded document schemas must be valid JSON")
}

pub fn capabilities() -> serde_json::Value {
    serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "implementation": "rust",
        "release_ready": false,
        "python_required_for_implemented_commands": false,
        "capabilities": {
            "skills_install": true,
            "documents_schema": true,
            "documents_workflow": true,
            "excel_import": true,
            "excel_writeback": true,
            "excel_structural_writeback": false,
            "noncell_extraction": false,
            "ocr": false
        },
        "limitations": ["Excel scalar cells only; structural/formula writeback, OCR, non-cell extraction, other formats, edit-plan and spec are not implemented. Empty-Windows acceptance remains unverified."]
    })
}
