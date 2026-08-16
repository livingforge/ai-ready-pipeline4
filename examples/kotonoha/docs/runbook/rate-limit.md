# レート制限

最終更新: 2026-04-08（田村）

## 方針

**1 テナントあたり 60 rps。** Ingress（Kubernetes）とアプリの両方で
同じ値を設定し、揃えておく。

- Ingress: `nginx.ingress.kubernetes.io/limit-rps: "60"`
- アプリ: `KOTONOHA_RATE_LIMIT_RPS=60`

**二重に掛けるのは意図したもの。** Ingress は乱暴な呼び出しを手前で
落とすため、アプリはテナント単位で均すため。Ingress は接続元 IP でしか
分けられないので、同じ Pod から複数テナントの呼び出しが来ると
テナントごとの制限にならない。

## 変えるとき

**必ず両方を変える。** 片方だけ変えると、どちらが効いているのか
分からなくなる。

```sh
# Ingress
kubectl -n kotonoha annotate ingress kotonoha \
  nginx.ingress.kubernetes.io/limit-rps=60 --overwrite

# アプリ（ConfigMap）
kubectl -n kotonoha set env deploy/kotonoha-api KOTONOHA_RATE_LIMIT_RPS=60
kubectl -n kotonoha rollout status deploy/kotonoha-api
```

変更したらこの文書の値も直すこと。

## 429 が出たとき

利用部門から「429 が返る」と言われたら、まず**どちらで落ちているか**を
見分ける。

| 見分け方 | どちら |
| --- | --- |
| 応答の本文が空、`nginx` の既定ページ | Ingress |
| 本文に `{"error":{"code":"rate_limited"}}` | アプリ |

アプリ側なら監査ログに記録が残っている。Ingress 側なら Ingress の
アクセスログを見る。

## よくある原因

- **取り込みを 1 件ずつ投げている。** `POST /v1/collections/{id}/documents`
  は 1 回に 1,000 件まで受けるので、まとめて投げてもらう
- **リトライが指数バックオフになっていない。** 429 を受けて即座に
  再送すると、ずっと 429 が返り続ける
- **バッチが夜間に集中している。** 品質保証部の同期が毎晩 2 時に
  始まるので、その時間帯に他部門の取り込みが重なると当たりやすい

---

## ★ アプリ側の値が揃っていない（未解決）

2026/05/22、品質保証部の取り込みが詰まったとき、**アプリ側だけを
100 rps へ上げた**。Ingress は 60 のままである。

上げた本人（小島）は「一時的に上げて、落ち着いたら戻す」つもりだった
が、そのままになっている。いまのコードの既定値は
`settings.rate_limit_rps = 100`。

**どちらが正しいのか誰も確かめていない。**

- Ingress が 60 のままなら、アプリの 100 は事実上効いていない
  （手前で落ちる）
- ただし同一 Pod からの複数テナント呼び出しでは効き方が変わる

戻すか、両方 100 にするかを決める必要がある。決めたらこの文書と
`common/settings.py` の既定値を揃えること。
