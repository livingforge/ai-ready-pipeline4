# capture・設計書生成の個別CLI操作

入力の固定化（capture）、原文の出力、workflowを使わない明示的な意味抽出・レビュー・採番・check・render。

- `spec capture --extraction <パース結果JSON>... --out <入力JSON>` で入力を固定化する。入力は候補または `.arp/documents/<文書ID>/extraction.json`。
- `spec sources --input <入力JSON> --out <原文.md>` でシート・セル順の全文を読む。大きい入力は `--document <文書ID>` で分割する。
- 新規生成は `spec semantic packet --input <入力> --document <ID> --out <タスク>` で文書別の全文・表情報・短い出典参照と意味判断契約を受け取る。解釈・分類・条件・数量・引用・関連・検証方法だけを返信し、コード・引用位置・正式ID・完全モデルを作らない。`spec semantic assemble --input <入力> --reply <返信>... --actor <担当> --out <フォルダー>` がモデル・分類・初回採番計画・診断を構築する。未処理範囲の自動除外は禁止。数量の未対応表現は診断を報告し、Agent自身が検証器の文法を探索するループを回さない。
- `spec semantic review-packet --input <入力> --model <モデル> --catalog <分類> [--document <ID>] --out <タスク>` で文書別の独立した原文レビューと、document未指定の文書間レビューを実施する。実施した確認だけ返信する。`review-apply` はpacket一致を検証し、部分レビューと未解決指摘も保存する。保存成功は最終受理ではない。`workflow init` と `review-apply` の `--review-plan` で理由付きの監査対象外出典を指定できる。省略時は保存済みの計画を引き継ぎ、計画のない新規モデルでは全出典を監査する。全体レビューと計画上の必須監査、修正対象の指摘の解消が最終受理条件となる。`finalize` はレビュー済みモデルと分類、初回採番計画から永続ID・台帳・分類参照を構築する。承認はしない。詳しくは docs/reference/semantic-generation.md。従来の直接編集時だけ `spec prompt` / `spec schema` の完全モデル契約を使う。
- 再実行は `spec semantic changes --before <旧入力> --after <新入力> --out <計画>` で変更文書を絞り、同一packetの返信だけを再利用する。変更された主張は文書間レビューをやり直す。finalizeは初回専用で、正本更新は既存のregistry applyまたはassign-ids --previousの明示的な同一性判断を維持する。
- `spec check --input <入力JSON> --model <モデル.yml>` で確認する。blocked の矛盾・未処理範囲・レビュー不足を解消し、`spec render --input <入力JSON> --model <モデル.yml> --out <設計書.md>` で生成する。未解決の草稿は `--draft` を指定する。
- 対象範囲はパース済み情報。ready は意味上の完全性を保証しない。矛盾は根拠ある判断を decisions に残し、原文の記述を削除して隠さない。
- check の summary は全問題の種類別件数、coverage は全入力の処理・監査件数。`--out <診断.json>` で全診断を保存できる。render は本文と `<出力名>.report.json` を保存し、草稿本文は診断集計のみを表示する。
- spec の相対パスは実行ディレクトリ基準。workflowとregistryの `--root` はリポジトリルート。省略時は `.arp/config.yml` をGit境界内で探索する。workflowの実行は `--run-id` で区別し、作業状態は `.arp/work/workflow/<run-id>/` に置く。
- 要件・仕様の名称は name、一時キーは id に分ける。`spec assign-ids --input <入力> --model <候補> --plan <対応表> --out <採番済みモデル>` で採番してから check/render する。継続時は `--previous <前回モデル>` を指定し、前回モデルと `.ids.json` 台帳を維持する。対応表は全項目に new/retain/replace と理由を指定し、旧項目の廃止は retire に理由を記す。分割・統合は predecessors で旧IDを記録し、新IDを発行する。名称だけで同一性を判断しない。監査自由文の一時キーは台帳の history.aliases で追跡する。
- 数量の別セル見出しが「平均」を指定する場合は `semantics: mean` と `semantics_basis`（「平均」の完全一致引用）を使う。引用を item.evidence に含め、同じ文書・シートの同じ行または同じ列の上方にある別セルを指定する。実際の見出しとの対応・否定・例外は原文レビューが必要。数量句全体の basis と比較方法も保持する。
- 同じ意味・対象・条件の重複は1項目へ統合し、各出典を根拠に残す。条件・例外の差や矛盾は消さない。不要表現は引用範囲と理由を付けて除外する。見出し・単位・注記は解釈根拠として保持し、意味不明・未処理の範囲を不要として除外しない。原文の処理状況と独立レビューの実施状況は別に報告する。
