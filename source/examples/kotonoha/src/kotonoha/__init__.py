"""Kotonoha —— アダタム工業の社内エンベディング基盤。

社内の各部門（テナント）へ、文書の取り込み・エンベディング生成・検索を
API で提供する。**回答生成（LLM の呼び出し）は含まない** —— それは各テナントが
自分で行う、というのが稟議書のスコープである。ただしこの線引きは
カスタマーサポート部との間で合意が取れていない（docs/meeting/ を参照）。

外部ライブラリに依存しない。FastAPI・Pydantic・psycopg・voyageai の代わりに
``kotonoha.framework`` と ``kotonoha.demo`` の薄いスタブを使う。本番の書き方は
そのまま残るので、資材としての見た目は保たれる —— 配るだけで動かせることを
優先した判断である。

    python -m kotonoha.demo.main        20 シナリオを流す
    python -m kotonoha.demo.cli serve   HTTP で待ち受ける
"""

__version__ = "1.4.2"

#: 本稼働日。PoC は 2025 年度、正式サービス化はこの日。
SERVICE_START = "2026-04-01"
