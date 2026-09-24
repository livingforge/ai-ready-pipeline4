# 対話型Agentによる設計書生成

読取資料は `.arp/config.yml` の `agent_read_format` に従います（省略時 `toon`、選択肢は `toon` / `json`）。Agentは `workflow read` のTOONを直接読み、内部JSONファイルの先読みやJSONへの変換は行いません。JSON設定時はそのJSONを読みます。返信・差分・validate-reply/submit・機械間の受け渡しはJSONです。形式を変更した場合はreadをoffset 0から再開します。

## 最初に実行する手順

親は配布されたarp4スキルを使い、handoffのagent名でカスタムAgentへ委任する。空いた同じ役割の担当を再利用し、文書数だけ新規起動しない。同時作業は既定で最大3担当を目安に、ユーザー指定とホスト枠に合わせる。作成・修正担当を独立レビューへ転用しない。ホストが再開できない場合は小さい同段階タスクを少数ずつ渡し、担当内で1件ずつ提出する。割当済みIDを管理して重複を防ぎ、段階の適用は作業中担当がなくなってから行う。

最初に親と担当の役割を分けます。親Agentは `status → handoff --task <ID> → 担当への割当 → receipt --task <ID>` で進行を管理します。link・restructureも担当へ渡し、親が全体の原文や返信JSONを読む工程にはしません。handoffが返すCLI・引数・作業ディレクトリ・指示をそのまま渡します。これは予約操作ではないため、親が重複割当を防ぎます。

以下は担当Agentの手順です。同じCLI・作業ディレクトリ・`--root`・`--run-id`を維持し、`read → validate-reply → submit` まで完結します。

1. 担当task_refが分かれば、直ちに `read --task <ID> --max-bytes 12000` を実行します。未割当ならstatusで担当を決めます。nextは次の1件を選ぶ場合、inspectは概要を個別確認する場合だけ使います。
2. 最初のreadは `--pointer` を付けず、応答を直接読みます。続きは `page.next_command.bash` または `.powershell` のサブコマンドを、同じCLIの `spec workflow` と作業設定の後ろに付けて実行します。`page.complete: true` まで続けます。
3. 表示が省略・ファイル退避された場合は、`--max-bytes` を下げ、revisionを外してoffset 0から再開します。原文の表示加工用ファイルやPythonを作らず、head/tailや文字列スライスで切り出しません。12000 bytesは対話向けの開始値であり、表示保証ではありません。
4. 担当タスクを読み終えたら意味判断の返信を作り、検証・提出して次の担当へ進みます。全タスクの原文を先にダンプしません。共通語彙はreadのmodule_vocabularyを使い、文書間の関連はlink・レビュー工程で判断します。

`page.complete` は選択した本文の最終ページを意味します。省略された表示の読了、抽出・レビューの完了を保証しません。

## 作業先と読み取り

作業の `--root` は `.arp/config.yml` のあるリポジトリルートです。先に `documents init` を実行し、workflowの実行は `--run-id`（既定current）で区別します。実行状態は `.arp/work/workflow/<run-id>/`、共有する現在版は `.arp/registry/`、原本は既定docsです。原本のコピーや版別フォルダを増やさず、過去版はGitで確認します。

操作には `next`・`inspect`・`status` が返す `task_ref`（通常8桁）を使えます。`--task` は8桁以上の一意な接頭辞を受け付け、曖昧なら拒否します。保存・監査用の64桁IDと内部ハッシュは維持します。Agent向けのpacket・base・bases・draft revisionは実行ごとの短い参照です。同じ実行のreadが返した値をそのまま使い、ハッシュ計算や別実行からの転用はしません。

`read --task <ID>` で必要な全節の本文を読みます。workflowは `--full` 非対応です。ページ送りにはCLIが返すnext_commandを使い、版が変わったら `--revision` を外してoffset 0から再開します。子pointer探索・固定刻み・`head -c` は不要です。個別確認用の `--pointer` はGit Bashでは先頭の `/` を省略できます。

`--max-bytes` は4096〜48000（既定48000）の設定形式（TOON／JSON）の応答bytes上限で、ホストの表示トークン数の保証ではありません。出力が省略・ファイル退避される場合は上限を小さくして先頭から読み直します。省略・退避された応答を読取完了と扱いません。`page.content_bytes` は選択部分の設定形式での分割前bytes、inspectの `section_bytes` は各節の量を示します。linkの全体資料はpacketだけ、repairの原文はcontextだけに含まれます。repairのpacketは識別情報で、内部の全文資料は検証・追加文脈取得用に保持されます。

読み取り・事前検証は共有ロック、更新は排他ロックを使います。競合時はCLIが最大30秒待機します。期限超過時は稼働プロセスを確認し、待機ループを重ねたりロックファイルを削除したりしません。共通分類が分かる場合は抽出前の `init --modules <語彙.json>` で共有します。新規モジュールの名前衝突は引き続き診断として返し、Agentが意味を判断して修正します。

## 返信と続行

既存の作業では同じCLI・作業設定を使う。モデル自動呼び出しや独自の加工スクリプトは不要。

1. statusでtask_refとstageを確認する。割り当て済みの担当はreadから始める。明示rootを使う案件は `workflow --root <root>`、別実行を使う案件は同じrun-idで実行する。
2. `read --task <ID>` の全ページを読む。指示、packet、context、scope、reply_schema、module_vocabulary、references、previous_error、previous_reply、draft、reply_templateが含まれる。`entries` のpointerは本文の位置を示すラベルで、再取得は不要。連続した配列要素は `array_offset`（先頭の0始まり添字）と `array_total`（配列全体の要素数）付きでまとめて返る。長い文字列は `string_offset` と `string_total_bytes`（UTF-8 bytes）付きで連続して返る。参照資料は出典ではなく判断材料で、`ref:<パス>` として message や reason に引用する。
3. 抽出原文のsources.rowsはsources.columns順で、table列はsources.tablesの添字。列定義・表情報も一括読み取りに含まれる。scope.sourcesは抽出対象、scope.contextは文脈のみ。contextが不足する場合はrequest_tablesで要求する。task文字列からJSONを切り出さない。
4. `/reply_template` を参考に返信JSONを作る。雛形は未作業であり、空のaudits/findingsをレビュー実施の代わりにしない。分類reasonとverificationは必要な場合だけ記載する。statementは構造化内容の言い直しなら省略できる。そこだけに含まれる役割・例外は保持する。数量・条件に記載した根拠はCLIがevidenceへ合流するので二重記述しない。追加の見出し・文脈の根拠はevidenceに残す。同じ意味・対象・条件の重複は各出典を保持して統合する。不要表現は引用範囲と理由を付けて除外し、意味不明・未処理の記述と区別する。
5. 返信と判断理由をUTF-8ファイルに保存し、`validate-reply --task <ID> --reply reply.json` で事前検証し、`submit --task <ID> --reply reply.json --origin interactive-agent --actor <担当> --reason-file reason.txt` で提出する。短い理由は `--reason <判断理由>` でもよいが、引用符・JSON断片・改行を含む理由はファイルを使う。`--reason` と `--reason-file` は同時指定不可。返信はUTF-8 stdinの `--reply -` も使える。PowerShellのパイプはUTF-8に設定する。実際のモデル名が分かる場合だけ `--model` を付ける。
6. 拒否時は診断と `/previous_error`・`/previous_reply`・`/draft` を確認する。下書きがあれば同じsubmitに `{"draft":"<revision>","set":{"/items/1/name":"修正値"}}` のように変更箇所だけ提出できる。既存フィールド・配列要素の削除は `remove`。CLIが結合後の返信全体を検証し、再拒否時は最新の版を返す。保持された部分は未受理で、意味的な正解の保証ではない。validate-replyは状態を保存しないため、その場の診断を使う。成功は受理可能という意味で、意味の正しさや設計書の完成の保証ではない。
7. `receipt --task <ID>` で提出記録を確認し、親へ300字以内でtask_ref・submission_accepted・要判断事項・成果物パスを返す。原文・返信JSON・詳しい理由はファイルに残す。別taskを自分で取得しない。親がstatus/next_actionsで次の割当を決める。`next: null` は完了とは限らない。適用待ちはadvance、実行中はrecoverの案内に従う。

## 分担とレビュー

親Agentは概要・割当・進捗・未解決事項を管理する。extract・link・review・global-review・repair・restructureすべてで担当がreadから判断・返信ファイル作成・検証・提出まで行う。親は委譲前の本文取得、子の返信の書き戻し、共通ブリーフの複製をしない。独立レビューは作成担当とは別のAgentへ割り当てる。担当ごとの作業ファイルを使い、自分のactorで提出する。拒否への修正も同じ担当が行う。

親は短い完了通知を受け、receiptの提出記録とstatusの進行状態を確認する。receiptは最終提出の受理可否と担当を返し、本文を返さない。nullは未提出、trueは意味的正解・公開承認ではない。advance後に引退したtaskも確認できる。各通知に長文で返答せず、同じ段階の担当が揃ったら次へ進める。担当Agentを利用できない環境では担当ごとに新しいセッションへ引き継ぐ。

提出後の結合・関連検証・repair/restructure適用はCLIが行う。state.json・objects・採番済みモデルを直接編集しない。

レビューの `context.escalation` にある `findings`（既知の指摘）と `unresolved`（repairが理由付きで変更なしと返した指摘）は再報告しない。同じaction・itemsの再報告は既存記録に畳み込まれる。repairで直せない場合は `changes: []` と `--reason` で理由を返す。複数文書にまたがる指摘はrestructureへ回る。

独立レビューは抽出とは別のAgent実行または人が担当する。同じAgentの自己点検を独立レビューと記録しない。数量をtext、平均をscalar、条件をunspecifiedへ変えて検証を通さない。固定語彙外の単位は `interpretation.kind=opaque_unit` と `unit_basis` で原文に結び付け、未登録の表現は `quantity_expression` として解釈待ちにする。独立レビューが未実施なら未完了として扱う。

next/inspectは概要専用。原文・契約・診断はreadから取得する。

同一文書内の複数項目にまたがる指摘は、全対象をfindings.itemsに列挙する。CLIが一括repairにまとめるため、1項目ずつ別の指摘へ分割しない。restructureでも解決不能なら `{"defer":{"kind":"information_required","reason":"不足する情報を具体的に記載"}}` を返す。kindはinformation_required / validator_support / unresolved。これは解決済みではなく保留であり、対象文書の内容が変わるまで同じ診断を再発行しない。open_issuesはedits内でリストを差し替えられるため、変更しないitemsを返信しない。

実測値が取得できる場合はsubmitに `--usage-file usage.json` を付ける。形式は `{"reported_cost_usd":0.25,"usage":{"input_tokens":100,"output_tokens":20,"cached_input_tokens":0,"reasoning_tokens":0}}`。取得できない値は省略し、推測で埋めない。費用未計測はnull、部分計測はcost_complete=false。集計対象はCLIが記録した呼び出し・提出で、親Agentや未提出作業の消費を自動取得するものではない。

validate-replyのquality_checkで引用文字のカバレッジと数量診断を確認する。repairでは変更前後を比較する。valid=trueは受理可能という意味で、原文の全主張の抽出や意味の正しさを保証しない。statusのreferencesは件数のみで、本文はreadから取得する。
