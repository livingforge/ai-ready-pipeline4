"""保存先の実装（PostgreSQL / pgvector / OpenSearch）。

**SQL はここにしかない。** 業務層は ``*/repository.py`` の約束だけを見て
いるので、表の名前も pgvector の演算子もここから外へ出ない。

このパッケージは外部ライブラリに依存しない ——``psycopg`` を import せず、
:class:`~kotonoha.store.connection.Connection` という約束越しに SQL を投げる。
本番はそこへ psycopg のカーソルを挿す。**動かすための実装は
``demo.memory_store``** で、そちらはメモリだけで動く。

ここを読む価値は SQL そのものにある —— ``ddl/`` の表定義と対で見ると、
どの列がどう使われているかが分かる。
"""
