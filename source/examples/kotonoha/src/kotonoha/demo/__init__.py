"""動かすための足場。**設計文書に対応する成果物ではない。**

本番は PostgreSQL・OpenSearch・Redis・S3・Voyage AI の API を使うが、
この資材は外部ライブラリとネットワークに依存しない方針なので、
すべてメモリの実装に差し替えてある。

    memory_store  すべての保存先のメモリ実装
    wiring        依存を挿して組み立てる
    fixtures      テナント 4 件と文書の見本
    main          20 シナリオを流す
    cli           HTTP で待ち受ける／運用の操作

``store/`` の SQL 実装と**同じ約束**（``*/repository.py``）に従うので、
差し替えは ``wiring`` の 1 か所で済む。
"""
