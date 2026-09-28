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
    refreshed_unix INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS passages (
    id INTEGER PRIMARY KEY,
    document TEXT NOT NULL,
    payload TEXT NOT NULL,
    title TEXT NOT NULL,
    body TEXT NOT NULL,
    context TEXT NOT NULL,
    identifiers TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS passages_document ON passages(document);
CREATE VIRTUAL TABLE IF NOT EXISTS terms USING fts5(title, body, context, grams,
    content = '', contentless_delete = 1,
    tokenize = 'unicode61 remove_diacritics 0');
