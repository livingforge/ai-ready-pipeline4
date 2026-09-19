"""CLI for the document-authority workflow (legacy specification commands stay compatible)."""
from __future__ import annotations

import argparse
import json
import re
import time
from pathlib import Path

from .contracts import DocumentError, SCHEMAS
from .store import Store


def register(subparsers) -> None:
    parser = subparsers.add_parser("documents", help="文書正本・成形履歴・Excel書き戻し")
    actions = parser.add_subparsers(dest="document_action", required=True)
    def command(name, help_text):
        item = actions.add_parser(name, help=help_text)
        item.add_argument("--root", type=Path, default=Path.cwd(), help="プロジェクト根")
        item.set_defaults(handler=run)
        return item
    init = command("init", "既存開発構成を変更せず文書管理を導入")
    init.add_argument("--directory", default="knowledge")
    command("upgrade-layout", "旧配置を移行し、原本を集約した文書候補を作成")
    imp = command("import", "Office等の原本を抽出し、採用版を変更せず候補を作る")
    imp.add_argument("source", type=Path)
    imp.add_argument("--id", required=True, dest="document_id")
    migrate = command("migrate", "旧Markdownパース結果の編集を採用候補へ移す")
    migrate.add_argument("legacy_directory", type=Path)
    migrate.add_argument("--source", type=Path, required=True)
    migrate.add_argument("--id", required=True, dest="document_id")
    spec = command("prepare-spec", "検証済み正本を既存仕様パイプラインの新規ラウンドへ渡す")
    spec.add_argument("document_id")
    record = command("record", "Agent/人が成形した候補とプロンプトを固定")
    record.add_argument("proposal")
    record.add_argument("--model", required=True, help="使用モデル。人の成形は human")
    record.add_argument("--actor", required=True)
    record.add_argument("--prompt", type=Path, required=True)
    diff = command("diff", "基準・現在・候補の差分")
    diff.add_argument("proposal")
    adopt = command("adopt", "検証済み候補をレビューして正本へ採用")
    adopt.add_argument("proposal")
    adopt.add_argument("--reviewer", required=True)
    review = command("review", "直接編集した正本の現在の内容をレビュー済みにする")
    review.add_argument("document_id")
    review.add_argument("--reviewer", required=True)
    for name in ("check", "watch"):
        check = command(name, "正本/候補の形式・出典・レビュー状態を検証" if name == "check" else "ファイル保存時に再検証（Ctrl+Cで終了）")
        check.add_argument("document_id", nargs="?")
        check.add_argument("--proposal")
        check.add_argument("--require-reviewed", action="store_true")
        check.add_argument("--format", choices=("text", "json"), default="text")
        if name == "watch":
            check.add_argument("--once", action="store_true")
    export = command("export", "Excelへの反映計画を表示。--outで検証済みExcelを出力")
    export.add_argument("document_id")
    export.add_argument("--out", type=Path)
    export.add_argument("--engine", choices=("auto", "xml", "excel"), default="auto")
    drawings = command("drawings", "原本の図形名・種別・テキストを一覧表示")
    drawings.add_argument("document_id", nargs="?")
    drawings.add_argument("--proposal")
    command("list", "通常検索する正本Markdownの一覧")
    schema = command("schema", "文書管理のJSON Schemaを表示")
    schema.add_argument("kind", choices=sorted(SCHEMAS))


def _check(store: Store, args) -> tuple[int, list[dict]]:
    if args.proposal:
        directories = [store.proposal(args.proposal)]
    elif args.document_id:
        directories = [store.document(args.document_id)]
    else:
        directories = sorted(p for p in store.documents.iterdir() if (p / "document.yml").is_file() and not p.name.startswith("."))
    results = []
    for directory in directories:
        try:
            result = store.inspect(directory, args.require_reviewed)
            warnings = []
            if not result["reviewed"]:
                warnings.append("未レビュー、またはレビュー後に編集されています")
            if not result["source_current"]:
                warnings.append("原本が変更/削除されています。再取り込みで確認してください")
            if result["pending"]:
                warnings.append(f"Excelへの対応が未確定: {len(result['pending'])} 件")
            results.append({"document": str(directory), "valid": True, "content": result["content"],
                            "reviewed": result["reviewed"], "source_current": result["source_current"],
                            "warnings": warnings})
        except (DocumentError, OSError, ValueError) as exc:
            results.append({"document": str(directory), "valid": False, "error": str(exc)})
    return (1 if any(not r["valid"] for r in results) else 0), results


def _print_check(results: list[dict], fmt: str):
    if fmt == "json":
        print(json.dumps(results, ensure_ascii=False, indent=2))
        return
    for result in results:
        if not result["valid"]:
            error = result["error"]
            match = re.match(r"^(.*?):(\d+): (.*)$", error, re.S)
            if match:
                print(f"{match[1]}:{match[2]}: error: {match[3]}")
            else:
                print(f"{result['document']}/document.yml:1: error: {error}")
        else:
            print(f"valid: {result['document']}")
            for warning in result["warnings"]:
                print(f"{result['document']}/document.yml:1: warning: {warning}")


def run(args: argparse.Namespace) -> int:
    try:
        action = args.document_action
        if action == "schema":
            print(json.dumps(SCHEMAS[args.kind], ensure_ascii=False, indent=2))
            return 0
        if action == "init":
            store = Store.init(args.root, args.directory)
            print(f"文書管理を作成しました: {store.directory}\nVSCode: {store.arp / 'documents.code-workspace'}")
            return 0
        if action == "upgrade-layout":
            print(json.dumps(Store.upgrade_layout(args.root), ensure_ascii=False, indent=2))
            return 0
        store = Store(args.root)
        if action in ("import", "migrate"):
            source = args.source if args.source.is_absolute() else args.root / args.source
            if action == "migrate":
                legacy = args.legacy_directory if args.legacy_directory.is_absolute() else args.root / args.legacy_directory
                result = store.migrate(source, args.document_id, legacy)
            else:
                result = store.import_source(source, args.document_id)
            print(f"候補: {result}\nAgentで成形 → check --proposal {result.name} → record → diff → adopt")
        elif action == "prepare-spec":
            print(json.dumps(store.prepare_spec(args.document_id), ensure_ascii=False, indent=2))
        elif action == "record":
            print(json.dumps(store.record(args.proposal, args.model, args.actor, args.prompt), ensure_ascii=False, indent=2))
        elif action == "diff":
            print(json.dumps(store.diff(args.proposal), ensure_ascii=False, indent=2))
        elif action == "adopt":
            print(store.adopt(args.proposal, args.reviewer))
        elif action == "review":
            print(json.dumps(store.review(args.document_id, args.reviewer), ensure_ascii=False, indent=2))
        elif action == "export":
            output = args.out if args.out is None or args.out.is_absolute() else args.root / args.out
            report = store.export(args.document_id, output, engine=args.engine)
            print(json.dumps(report, ensure_ascii=False, indent=2))
            return 0 if report["complete"] else 1
        elif action == "drawings":
            from . import excel
            from .contracts import under, read
            if not args.proposal and not args.document_id:
                raise DocumentError("document_id or --proposal is required")
            directory = store.proposal(args.proposal) if args.proposal else store.document(args.document_id)
            meta = read(directory / "document.yml", "document")
            extraction = store.extraction(meta["extraction"])
            print(json.dumps(excel.drawings(under(store.root, extraction["source"]["snapshot"])), ensure_ascii=False, indent=2))
        elif action in ("check", "watch"):
            previous = None
            while True:
                code, results = _check(store, args)
                if action == "check" or results != previous:
                    if action == "watch":
                        print("ARP validation started", flush=True)
                    _print_check(results, args.format)
                    if action == "watch":
                        print("ARP validation finished", flush=True)
                if action == "check" or args.once:
                    return code
                previous = results
                time.sleep(0.5)
        elif action == "list":
            for directory in sorted(store.documents.iterdir()):
                if directory.is_dir() and not directory.name.startswith("."):
                    for path in sorted((directory / "content").rglob("*.md")):
                        print(path.relative_to(store.root).as_posix())
        return 0
    except KeyboardInterrupt:
        return 130
    except (DocumentError, OSError, ValueError, KeyError) as exc:
        config_name = "config.yml" if (args.root / ".arp/config.yml").exists() else "documents.yml"
        print(f"{args.root}/.arp/{config_name}:1: error: {exc}")
        return 1
