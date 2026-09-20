"""文書の取り込み（インジェスト・登録）。

原文を受け取り、抽出 → 正規化 → 分割 → 埋め込み → 格納まで運ぶ。
件数が多いので API は受付だけ返し、実際の処理はワーカが非同期に回す。

    extract    形式ごとにテキストを起こす（text/markdown/html/pdf/csv）
    normalizer 表記を揃える
    chunker    ★ 分割規則の唯一の正本
    dedupe     同じ文書の再取り込みを飛ばす
    pipeline   1 文書ぶんの流れ
    queue      受付と取り出し
    worker     キューを回す
    job        進捗の記録
    service    受付の入口
"""
