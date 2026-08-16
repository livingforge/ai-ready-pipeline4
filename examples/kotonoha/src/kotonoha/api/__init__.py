"""HTTP の受け口。

**正本は ``openapi.yaml``**（リポジトリの根）である。ここはその実装で、
経路とスキーマが仕様と食い違ったら仕様のほうが正しい。

★ arp4 は ``.yaml`` を読まないので、資料だけを機械へ渡すと API の正本が
  落ちる（README の仕込み F1）。**「資料に無い」ではなく「読めていない」**
  として申告されるのが正しい。

    app         経路をまとめる
    auth        API キーからテナントを解決する
    middleware  レート制限・監査の下ごしらえ
    errors      業務の例外を HTTP へ翻訳する
    schemas     入出力の型
    embeddings / collections / documents / search / jobs / usage
"""
