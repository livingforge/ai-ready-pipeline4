"""Versioned contracts for editable documents; shared by CLI and validators."""
from __future__ import annotations

import hashlib
import json
import re
from pathlib import Path
from typing import Any

import yaml


class DocumentError(ValueError):
    pass


def obj(properties: dict, required: list[str] | None = None) -> dict:
    return {"type": "object", "properties": properties,
            "required": list(properties) if required is None else required,
            "additionalProperties": False}


def array(items: dict) -> dict:
    return {"type": "array", "items": items}


TEXT = {"type": "string"}
NONEMPTY = {"type": "string", "minLength": 1}
ID = {"type": "string", "pattern": "^[a-zA-Z0-9][a-zA-Z0-9_-]*$"}
HASH = {"type": "string", "pattern": "^[a-f0-9]{64}$"}
VERSION = {"const": "1"}
VALUE = {"type": ["string", "number", "boolean", "null"]}
CELL = obj({"id": ID, "address": {"type": "string", "pattern": "^[A-Z]+[1-9][0-9]*$"},
            "type": {"enum": ["string", "number", "boolean", "null", "formula", "error"]}, "value": VALUE,
            "formula": {"type": ["string", "null"]}, "cached": VALUE,
            "number_format": TEXT})
CELL["allOf"] = [
    {"if": {"properties": {"type": {"const": kind}}},
     "then": {"properties": {"value": {"type": value_type}}}}
    for kind, value_type in [("string", "string"), ("error", "string"),
                             ("number", "number"), ("boolean", "boolean"), ("null", "null")]
]
CELL["allOf"].append({"if": {"properties": {"type": {"const": "formula"}}},
                      "then": {"properties": {"formula": TEXT}},
                      "else": {"properties": {"formula": {"type": "null"}, "cached": {"type": "null"}}}})
CHUNK = obj({"id": ID, "at": TEXT, "heading": TEXT,
             "rows": array(array(TEXT)),
             "cells": array({"type": "array", "items": TEXT, "minItems": 2, "maxItems": 2}), "text": TEXT})
PAGE = obj({"id": ID, "title": TEXT, "source": TEXT, "notes": array(TEXT),
            "chunks": array(CHUNK)})
SHEET = obj({"name": NONEMPTY, "part": NONEMPTY, "state": TEXT,
             "merges": array(TEXT), "cells": array(CELL)})
SOURCE = obj({"path": NONEMPTY, "sha256": HASH, "snapshot": NONEMPTY})
ORIGIN = obj({"page": ID, "block": ID})
TARGET = obj({"sheet": NONEMPTY, "cell": NONEMPTY})
ENTRY = obj({"page": ID, "block": ID, "field": {"type": ["string", "null"]},
             "origins": array(ORIGIN), "reason": TEXT,
             "writeback": {"enum": ["cell", "excluded", "pending"]},
             "target": {"anyOf": [TARGET, {"type": "null"}]}})
OMISSION = obj({"origin": {"anyOf": [ORIGIN, {"type": "null"}]},
                "target": {"anyOf": [TARGET, {"type": "null"}]}, "reason": NONEMPTY})

# Version 2 adds explicit structural operations; v1 remains readable unchanged.
FIELD_REF = obj({"page": ID, "block": ID, "field": ID})
INSERTED_TARGET = obj({"sheet": NONEMPTY, "insertion": ID,
                       "offset": {"type": "integer", "minimum": 0},
                       "column": {"type": "string", "pattern": "^[A-Z]+$"}})
ENTRY_V2 = obj({**ENTRY["properties"],
    "writeback": {"enum": ["cell", "formula", "operation", "excluded", "pending"]},
    "target": {"anyOf": [TARGET, INSERTED_TARGET, {"type": "null"}]}})
PROPERTIES = obj({name: FIELD_REF for name in (
    "text", "left", "top", "width", "height", "rotation", "fill", "line", "line_weight", "font_size")}, [])
PROPERTIES["minProperties"] = 1
OP_BASE = {"id": ID, "sheet": NONEMPTY, "reason": NONEMPTY}
ROW_OPERATION = obj({**OP_BASE, "kind": {"enum": ["insert_rows", "delete_rows"]},
    "at": {"type": "integer", "minimum": 1, "maximum": 1048576},
    "count": {"type": "integer", "minimum": 1, "maximum": 1048576},
    "style_from": {"type": "integer", "minimum": 1, "maximum": 1048576}},
    [*OP_BASE, "kind", "at", "count"])
SHAPE_UPDATE = obj({**OP_BASE, "kind": {"const": "update_shape"}, "name": NONEMPTY,
                    "properties": PROPERTIES})
SHAPE_DELETE = obj({**OP_BASE, "kind": {"const": "delete_shape"}, "name": NONEMPTY})
SHAPE_ADD = obj({**OP_BASE, "kind": {"const": "add_shape"}, "name": NONEMPTY,
    "shape_type": {"enum": ["rectangle", "rounded_rectangle", "ellipse", "textbox", "line"]},
    "properties": PROPERTIES})
PICTURE = obj({**OP_BASE, "kind": {"enum": ["add_picture", "replace_picture"]},
               "name": NONEMPTY, "asset": FIELD_REF, "properties": {**PROPERTIES, "minProperties": 0}})
CONNECTION = obj({"name": NONEMPTY, "site": {"type": "integer", "minimum": 1}})
CONNECTOR = obj({**OP_BASE, "kind": {"const": "add_connector"}, "name": NONEMPTY,
    "begin": CONNECTION, "end": CONNECTION,
    "connector_type": {"enum": ["straight", "elbow", "curve"]}, "properties": PROPERTIES},
    [*OP_BASE, "kind", "name", "begin", "end", "connector_type"])
OPERATION = {"oneOf": [ROW_OPERATION, SHAPE_UPDATE, SHAPE_DELETE, SHAPE_ADD, PICTURE, CONNECTOR]}

SCHEMAS = {
    "project-config": obj({"schema_version": VERSION,
                           "documents": obj({"directory": NONEMPTY}),
                           "spec": obj({"directory": NONEMPTY})}),
    "config": obj({"schema_version": VERSION, "directory": NONEMPTY}),
    "document": obj({"schema_version": VERSION, "document_id": ID,
                     "extraction": HASH, "source": SOURCE}),
    "frontmatter": obj({"schema_version": VERSION, "document_id": ID, "page_id": ID}),
    "mappings": {"oneOf": [
        obj({"schema_version": VERSION, "entries": array(ENTRY), "omissions": array(OMISSION)}),
        obj({"schema_version": {"const": "2"}, "entries": array(ENTRY_V2),
             "omissions": array(OMISSION), "operations": array(OPERATION)})]},
    "extraction": obj({"schema_version": VERSION, "document_id": ID, "source": SOURCE,
                       "parser": NONEMPTY, "pages": array(PAGE), "sheets": array(SHEET),
                       "findings": array(obj({"level": TEXT, "code": TEXT,
                                               "message": TEXT})),
                       "assets": array(obj({"path": NONEMPTY, "sha256": HASH}))}),
    "proposal": obj({"schema_version": VERSION, "document_id": ID,
                     "base": {"type": ["string", "null"], "pattern": "^[a-f0-9]{64}$"}}),
    "formation": obj({"schema_version": VERSION, "document_id": ID,
                      "extraction": HASH, "content": HASH, "actor": NONEMPTY,
                      "model": NONEMPTY, "prompt_sha256": HASH}),
    "review": obj({"schema_version": VERSION, "content": HASH,
                   "reviewer": NONEMPTY, "formation": HASH}),
}
for _schema in SCHEMAS.values():
    _schema["$schema"] = "https://json-schema.org/draft/2020-12/schema"


def validate(kind: str, data: Any) -> None:
    from jsonschema import Draft202012Validator
    errors = sorted(Draft202012Validator(SCHEMAS[kind]).iter_errors(data),
                    key=lambda e: str(list(e.path)))
    if errors:
        raise DocumentError("; ".join(f"{kind}/{ '/'.join(map(str, e.path))}: {e.message}"
                                      for e in errors))


def unique_pairs(pairs: list[tuple]) -> dict:
    result = {}
    for key, value in pairs:
        if key in result:
            raise DocumentError(f"duplicate key: {key}")
        result[key] = value
    return result


class StrictLoader(yaml.SafeLoader):
    pass


def _mapping(loader, node):
    return unique_pairs([(loader.construct_object(k), loader.construct_object(v))
                         for k, v in node.value])


StrictLoader.add_constructor(yaml.resolver.BaseResolver.DEFAULT_MAPPING_TAG, _mapping)


def yaml_text(text: str) -> Any:
    try:
        return yaml.load(text, Loader=StrictLoader)
    except (yaml.YAMLError, TypeError) as exc:
        raise DocumentError(str(exc)) from exc


def json_text(text: str) -> Any:
    def bad(value):
        raise DocumentError(f"invalid JSON number: {value}")
    try:
        return json.loads(text, object_pairs_hook=unique_pairs, parse_constant=bad)
    except (ValueError, TypeError) as exc:
        raise DocumentError(str(exc)) from exc


def read(path: Path, kind: str | None = None) -> Any:
    try:
        text = path.read_text(encoding="utf-8")
        data = json_text(text) if path.suffix == ".json" else yaml_text(text)
        if kind:
            validate(kind, data)
        return data
    except (OSError, UnicodeError, DocumentError) as exc:
        raise DocumentError(f"{path}: {exc}") from exc


def encoded(data: Any) -> bytes:
    return (json.dumps(data, ensure_ascii=False, sort_keys=True, indent=2,
                       allow_nan=False) + "\n").encode("utf-8")


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def write(path: Path, data: Any, kind: str | None = None) -> None:
    if kind:
        validate(kind, data)
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.suffix == ".json":
        path.write_bytes(encoded(data))
    else:
        path.write_text(yaml.safe_dump(data, allow_unicode=True, sort_keys=False),
                        encoding="utf-8", newline="\n")


def under(root: Path, relative: str) -> Path:
    path = (root / relative).resolve()
    if Path(relative).is_absolute() or path == root.resolve() or not path.is_relative_to(root.resolve()):
        raise DocumentError(f"path must stay inside {root}: {relative}")
    return path


def identifier(value: str) -> str:
    if not re.fullmatch(ID["pattern"], value):
        raise DocumentError(f"invalid ID: {value}")
    if value.upper().split('.')[0] in {"CON", "PRN", "AUX", "NUL", *[f"COM{i}" for i in range(1, 10)], *[f"LPT{i}" for i in range(1, 10)]}:
        raise DocumentError(f"reserved ID: {value}")
    return value
