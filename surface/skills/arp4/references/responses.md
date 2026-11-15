# CLI応答の読み方

ページ送り、省略、エラー、状態の確認。

- 通常実行は成功・失敗とも標準出力の1行JSON（`spec workflow read` の読取資料は設定形式、既定TOON）。`ok: false` または終了コード2なら `error` と対象の状態を確認する。`ok: true` はコマンド成功であり、成形完了・採用権限・書き戻し可能の保証ではない。
- `ignored_options` があれば、その引数は効果がないため無視されている。以後のそのコマンドでは付けない。`invalid_arguments` は `error.accepted`（受け付けるオプション）・`error.hint`（置く位置）・`error.subcommands` を見て一度で直し、`--help` を読み直さない。
- check/statusの `items[].state` は `needs_record`（実際の成形後にrecord）、`ready_to_adopt`（委任された範囲でadopt）、`needs_review`（採用直後または変更後で、内容を確認してreview）、`imported`（`import --adopt` で抽出したまま採用し、編集がない。レビュー不要）、`reviewed`、`blocked`（blockersを解消）、`invalid`（errorを確認）。採用済み文書の変更にはrecordを再実行しない。
- 一覧・差分・export計画は `summary` が全体、`items` が最大20件。`page.next_offset` が数値なら、同じ読み取りコマンドに `--offset <値>` を付けて続ける。`--limit 1..100` で件数を指定できる。読み取り中に文書を変更した場合は先頭から確認し直す。
- 通常応答は最大16 KiB（workflowのread/inspect/handoff/receipt/next/validate-replyと `spec structure` の各コマンドは対象外）。`omitted_fields` にあるフィールドや `omitted: true` は未表示であり、空値・変更なしではない。`page.next_offset: null` でも値が省略されていれば全内容を確認したと扱わない。
- diffの詳細は `items` の `path/kind/before/after`（比較が複数種類のときだけ `comparison`）。原本位置での値変更 `semantic_changes` は `--full` か `--out` の保存ファイルにある。大きな値は `documents diff ... --out <新規JSONパス>` で完全な差分を保存し、必要な箇所を読む。exportの `--out` は原本と同形式の文書の書き込みで、応答は件数と `report` のパス。続きの取得のために書き込みコマンドを再実行しない。
- schemaは既定でプロパティ一覧。制約は `documents schema <種類> --pointer /properties/<名前>` で取得する。`--full` は全件・全値を返すため大きくなる。必要な読み取り操作だけで指定し、巨大な結果はファイルへリダイレクトする。
- ハッシュ照合が必要な場合だけ `--include-hashes` を付ける。本文は `content/`、検証状態はcheck/statusを参照する。保存された証跡とexportの `.report.json` は完全なハッシュと値を保持する。
