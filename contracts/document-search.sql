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
CREATE VIRTUAL TABLE IF NOT EXISTS terms USING fts5(title, body, context, grams,
    content = '', contentless_delete = 1,
    tokenize = 'unicode61 remove_diacritics 0');
