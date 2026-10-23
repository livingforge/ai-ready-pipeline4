-- Disposable local search index; canonical documents remain under documents/.
CREATE TABLE IF NOT EXISTS documents (
    id TEXT PRIMARY KEY,
    fingerprint TEXT NOT NULL,
    state TEXT NOT NULL,
    refs TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS metadata (
    signature TEXT NOT NULL,
    failed TEXT NOT NULL,
    refreshed_unix INTEGER NOT NULL,
    corpus_revision TEXT NOT NULL,
    indexed_documents INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS passages (
    id INTEGER PRIMARY KEY,
    document TEXT NOT NULL,
    -- The passage as JSON, deflate-compressed; only returned hits are expanded.
    payload BLOB NOT NULL,
    title TEXT NOT NULL,
    body TEXT NOT NULL,
    repetition_group INTEGER NOT NULL,
    heading_group INTEGER NOT NULL,
    identifiers TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS passages_document ON passages(document);
-- Each format of indexed text once, as JSON: the effective font of a cell or
-- shape text and the emphasis its style records.
CREATE TABLE IF NOT EXISTS formats (
    id INTEGER PRIMARY KEY,
    format TEXT NOT NULL UNIQUE
);
-- The formats each passage's text is in, with the cells in each; a cell that
-- continues from an earlier fragment is counted there.
CREATE TABLE IF NOT EXISTS passage_formats (
    passage INTEGER NOT NULL,
    format INTEGER NOT NULL,
    cells INTEGER NOT NULL,
    PRIMARY KEY (passage, format)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS passage_formats_format ON passage_formats(format, passage);
CREATE VIRTUAL TABLE IF NOT EXISTS terms USING fts5(title, body, context, grams,
    content = '', contentless_delete = 1,
    tokenize = 'unicode61 remove_diacritics 0');
