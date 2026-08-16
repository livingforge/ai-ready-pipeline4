"""利用申請台帳・キャパシティ試算・テナント別コスト実績。

★ **利用申請台帳の法務部の上限（100,000）が、実装の既定（300,000）と
   食い違う**（README の仕込み C1）。

★ **キャパシティ試算にグラフを入れる。** Excel のチャートは図形ではない
   ので ``arp4 parse`` が拾わない（README の仕込み F2）。
"""

from __future__ import annotations

from pathlib import Path

from openpyxl.chart import BarChart, LineChart, Reference

import sheetkit as kit
import spec


def build(out: Path) -> list[Path]:
    return [_daicho(out), _capacity(out), _cost(out)]


# ── 利用申請台帳 ──────────────────────────────────────────────────
def _daicho(out: Path) -> Path:
    info = kit.DocInfo(
        doc_name="利用申請台帳", version="1.4", date="2026/03/16",
        author="情報システム部 佐野",
        revisions=[
            ("1.0", "2025/11/12", "初版作成（カスタマーサポート部の申請）", "佐野"),
            ("1.1", "2026/01/20", "品質保証部を追加", "佐野"),
            ("1.2", "2026/02/03", "法務部を追加（極秘区分の初適用）", "佐野"),
            ("1.3", "2026/02/26", "機密区分の欄を情報区分規程に合わせて修正", "佐野"),
            ("1.4", "2026/03/16", "営業本部を追加", "佐野"),
        ])
    wb = kit.new_book()
    kit.three_sheet(wb, info)

    ws = kit.add_sheet(wb, "1.申請一覧")
    row = kit.heading(ws, 2, 2, "1. 利用申請の一覧")
    row = kit.table(
        ws, top=row, left=2,
        caption="社内エンベディング基盤 Kotonoha 利用申請台帳",
        header=["テナント識別子", "所管部門", "用途", "区分", "名称",
                "月間上限", "モデル", "申請日", "承認日"],
        groups=[("", 1), ("申請部門", 2), ("機密区分", 2), ("", 4)],
        rows=[
            [tenant_id, dept, purpose, level,
             dict(spec.CLASSIFICATIONS and
                  {c[0]: c[1] for c in spec.CLASSIFICATIONS})[level],
             f"{quota:,}", model, applied, approved]
            for tenant_id, dept, purpose, level, quota, model, applied, approved in [
                ("cs-support", "カスタマーサポート部",
                 "保守問合せの回答支援。過去の問合せ履歴と保守マニュアルを引く",
                 "10", 500_000, "voyage-4", "2025/11/12", "2025/11/28"),
                ("qa-defect", "品質保証部",
                 "不具合報告の類似検索。過去トラブルの再発を早く見つける",
                 "20", 2_000_000, "voyage-4", "2026/01/20", "2026/02/02"),
                ("legal-contract", "法務部",
                 "契約書の類似条項検索。ひな形との差分をあたる",
                 "30", 100_000, "voyage-4-nano（社内GPU）", "2026/02/03", "2026/02/24"),
                ("sales-proposal", "営業本部",
                 "提案書の再利用。過去案件から近いものを探す",
                 "20", 300_000, "voyage-4", "2026/03/16", "2026/03/30"),
            ]
        ],
        widths=[16, 18, 40, 6, 8, 12, 22, 12, 12],
        center_cols=(3, 5, 7, 8))

    kit.note(ws, row, 2,
             "※ 月間上限は「埋め込んだチャンク数」。検索の回数は上限の対象外。")
    kit.set_print(ws, doc_name=info.doc_name)

    ws = kit.add_sheet(wb, "2.機密区分")
    row = kit.heading(ws, 2, 2, "2. 機密区分の扱い")
    kit.table(ws, top=row, left=2,
              header=["区分", "名称", "外部APIへの送出", "保持期間", "説明"],
              rows=[list(row_) for row_ in spec.CLASSIFICATIONS],
              widths=[6, 10, 18, 12, 52], center_cols=(0, 2, 3))
    kit.set_print(ws, doc_name=info.doc_name)

    ws = kit.add_sheet(wb, "3.費用の付替先")
    row = kit.heading(ws, 2, 2, "3. 費用の付替先")
    kit.table(ws, top=row, left=2,
              header=["テナント識別子", "所管部門", "原価センタ", "予算科目", "担当"],
              rows=[
                  ["cs-support", "カスタマーサポート部", "CC-4120",
                   "情報システム費", "宮下"],
                  ["qa-defect", "品質保証部", "CC-3310", "情報システム費", "堀"],
                  ["legal-contract", "法務部", "CC-1150", "一般管理費", "岸本"],
                  ["sales-proposal", "営業本部", "CC-2200", "販売費", "村井"],
              ], widths=[16, 22, 12, 18, 10])
    kit.set_print(ws, doc_name=info.doc_name)

    ws = kit.add_sheet(wb, "4.変更履歴")
    row = kit.heading(ws, 2, 2, "4. 申請内容の変更履歴")
    row = kit.table(ws, top=row, left=2,
                    header=["変更日", "テナント識別子", "変更内容", "依頼者", "反映者"],
                    rows=[
                        ["2026/02/24", "legal-contract",
                         "機密区分を 20 から 30 へ変更（情報セキュリティ部の指摘）",
                         "岸本", "佐野"],
                        ["2026/03/02", "qa-defect",
                         "月間上限を 1,000,000 から 2,000,000 へ", "堀", "佐野"],
                    ], widths=[12, 16, 52, 10, 10])

    # ★ 仕込み C1。台帳が実態に追いついていないことを、あえて書かない。
    kit.note(ws, row, 2,
             "※ 変更は情報システム部への依頼をもって反映する。"
             "口頭での依頼は受け付けない。")
    kit.set_print(ws, doc_name=info.doc_name)

    return kit.save(wb, out / "利用申請台帳.xlsx")


# ── キャパシティ試算（★グラフ入り）─────────────────────────────
def _capacity(out: Path) -> Path:
    info = kit.DocInfo(doc_name="キャパシティ試算", version="2.0",
                       date="2026/06/15", author="小島",
                       revisions=[
                           ("1.0", "2026/02/10", "初版作成（稟議の添付）", "小島"),
                           ("2.0", "2026/06/15",
                            "品質保証部の実績が試算の 3 倍だったため見直し", "小島"),
                       ])
    wb = kit.new_book()
    kit.three_sheet(wb, info)

    ws = kit.add_sheet(wb, "1.前提")
    row = kit.heading(ws, 2, 2, "1. 試算の前提")
    kit.kv_table(ws, top=row, left=2, pairs=[
        ("分割単位", "512 トークン（オーバーラップ 64 トークン）"),
        ("1 チャンクの平均", "本文 700 文字 / ベクトル 1024 次元"),
        ("ベクトルの保管量", "1024 次元 × 4 バイト = 4KB/チャンク（float の場合）"),
        ("索引の膨らみ", "HNSW で本体の約 1.5 倍"),
        ("原文の保管", "オブジェクトストア。本試算には含めない"),
        ("外部 API の単価", "提供元の請求実績から翌月に按分（ADR-008）"),
    ], width=70, label_width=20)
    kit.set_print(ws, doc_name=info.doc_name)

    # 月次の推移。**このシートのデータからグラフを作る。**
    ws = kit.add_sheet(wb, "2.月次推移")
    kit.heading(ws, 2, 2, "2. チャンク数の推移（実績と見込み）")
    months = ["2026/04", "2026/05", "2026/06", "2026/07", "2026/08",
              "2026/09", "2026/10", "2026/11", "2026/12"]
    cs = [180, 220, 260, 300, 340, 380, 420, 460, 500]
    qa = [420, 1_180, 2_040, 2_300, 2_500, 2_700, 2_900, 3_100, 3_300]
    legal = [0, 0, 42, 60, 78, 96, 114, 132, 150]
    sales = [0, 0, 0, 85, 140, 195, 250, 305, 360]

    kit.table(ws, top=4, left=2,
              caption="単位: 千チャンク（累計）",
              header=["年月", "カスタマーサポート部", "品質保証部",
                      "法務部", "営業本部", "合計"],
              rows=[[m, f"{a}", f"{b}", f"{c}", f"{d}", f"{a + b + c + d}"]
                    for m, a, b, c, d in zip(months, cs, qa, legal, sales)],
              widths=[12, 20, 14, 12, 12, 12],
              center_cols=(1, 2, 3, 4, 5))

    # ★ 仕込み F2: Excel のチャートは図形ではないので parse が拾わない。
    #    数字は表にあるが「この形で見せている」ことは機械へ渡らない。
    chart = LineChart()
    chart.title = "チャンク数の推移（累計・千チャンク）"
    chart.style = 12
    chart.y_axis.title = "千チャンク"
    chart.x_axis.title = "年月"
    data = Reference(ws, min_col=3, max_col=6, min_row=5, max_row=5 + len(months))
    categories = Reference(ws, min_col=2, min_row=6, max_row=5 + len(months))
    chart.add_data(data, titles_from_data=True)
    chart.set_categories(categories)
    chart.height = 9
    chart.width = 20
    ws.add_chart(chart, "I4")
    kit.set_print(ws, doc_name=info.doc_name)

    ws = kit.add_sheet(wb, "3.保管量")
    row = kit.heading(ws, 2, 2, "3. 保管量の見込み")
    row = kit.table(
        ws, top=row, left=2,
        header=["時点", "チャンク数", "ベクトル（float）", "ベクトル（int8）",
                "索引込み（int8）", "備考"],
        rows=[
            ["2026/06 実績", "2,342 千", "9.4 GB", "2.3 GB", "3.5 GB",
             "6/3 に量子化の詰め替え完了"],
            ["2026/12 見込み", "4,310 千", "17.2 GB", "4.3 GB", "6.5 GB", ""],
            ["2027/12 見込み", "9,800 千", "39.2 GB", "9.8 GB", "14.7 GB",
             "現行ディスク（100GB）に収まる"],
        ],
        widths=[16, 14, 18, 18, 18, 30], center_cols=(1, 2, 3, 4))

    # ★ 量子化を入れた理由がここにある。ADR-003 には無い。
    kit.note(ws, row, 2,
             "※ 2026/05 に int8 量子化を導入。品質保証部の取り込みが"
             "初版の試算（月 40 万チャンク）の 3 倍になり、float のままでは"
             "2026 年度内にディスクが逼迫する見込みだったため。")

    bars = BarChart()
    bars.title = "ベクトルの保管量（GB）"
    bars.type = "col"
    bars.y_axis.title = "GB"
    bar_data = Reference(ws, min_col=4, max_col=5, min_row=row - 6, max_row=row - 3)
    bar_cats = Reference(ws, min_col=2, min_row=row - 5, max_row=row - 3)
    bars.add_data(bar_data, titles_from_data=True)
    bars.set_categories(bar_cats)
    bars.height = 8
    bars.width = 16
    ws.add_chart(bars, "I14")
    kit.set_print(ws, doc_name=info.doc_name)

    ws = kit.add_sheet(wb, "4.GPU")
    row = kit.heading(ws, 2, 2, "4. 社内 GPU の見込み")
    kit.table(ws, top=row, left=2,
              header=["項目", "値", "根拠"],
              rows=[
                  ["枚数", "A100 × 2",
                   "極秘区分の埋め込み専用。試算は 1 枚で置く"
                   "（残る 1 枚は再インデックスと障害時の予備）"],
                  ["1 回の処理件数", "32 件", "VRAM に載る範囲"],
                  ["1 呼び出しの所要", "2.4 秒",
                   "実測。32 件あたりの所要（外部 API は 0.9 秒／128 件）"],
                  ["月間の処理能力", "約 768 万チャンク",
                   "8 時間/日 × 20 日 = 576,000 秒 ÷ 2.4 秒 × 32 件（GPU 1 枚）"],
                  ["法務部の見込み", "月 1.8 万チャンク",
                   "2.月次推移 の法務部（月 +18 千チャンク）。処理能力に余裕がある"],
                  ["逼迫する条件", "極秘テナントが 3 部門を超えたとき",
                   "月間の総量には余裕があるが、A100 が 2 枚しかないので"
                   "取り込みが重なると待ちが出る。増設の検討が必要"],
              ], widths=[22, 22, 48])
    kit.set_print(ws, doc_name=info.doc_name)

    return kit.save(wb, out / "キャパシティ試算.xlsx")


# ── テナント別コスト実績 ──────────────────────────────────────────
def _cost(out: Path) -> Path:
    info = kit.DocInfo(doc_name="テナント別コスト実績", version="1.3",
                       date="2026/07/07", author="小島")
    wb = kit.new_book()
    kit.three_sheet(wb, info)

    ws = kit.add_sheet(wb, "1.月次実績")
    row = kit.heading(ws, 2, 2, "1. 月次のチャージバック実績")
    row = kit.table(
        ws, top=row, left=2,
        caption="単位: 円（円未満切り捨て）",
        header=["年月", "テナント", "原価センタ", "エンベディング", "検索",
                "リランク", "社内GPU", "合計"],
        groups=[("", 3), ("内訳", 4), ("", 1)],
        # 数量は**当月の増分**で数える（ADR-008「日次で計測し、月次で締める」）。
        # キャパシティ試算 2.月次推移 は累計なので、その差を取る。
        #   cs    04 180 千 / 05 +40 千 / 06 +40 千
        #   qa    04 420 千 / 05 +760 千 / 06 +860 千
        #   legal 06 +42 千（04・05 は 0）
        # 2026/04 が大きいのは既存文書の初期一括取り込みが入るためである。
        rows=[
            ["2026/04", "cs-support", "CC-4120", "3,240", "1,120", "0", "0", "4,360"],
            ["2026/04", "qa-defect", "CC-3310", "7,560", "2,480", "0", "0", "10,040"],
            ["2026/05", "cs-support", "CC-4120", "720", "1,360", "0", "0", "2,080"],
            ["2026/05", "qa-defect", "CC-3310", "13,680", "3,120", "0", "0", "16,800"],
            ["2026/06", "cs-support", "CC-4120", "720", "1,480", "0", "0", "2,200"],
            ["2026/06", "qa-defect", "CC-3310", "15,480", "3,880", "4,656", "0", "24,016"],
            # 法務部の埋め込みは社内 GPU（voyage-4-nano）なので、外部 API の
            # 単価（0.0180）ではなく社内 GPU 版の単価（0.0060）を当てる。
            #   42,000 チャンク × 0.0060 = 252 円
            # 社内GPU は占有秒。キャパシティ試算 4.GPU の実測（32 件/回・
            # 2.4 秒/回）で 42,000 チャンク = 3,150 秒。
            #   3,150 秒 × 0.0140 = 44.1 → 44 円（円未満切り捨て）
            ["2026/06", "legal-contract", "CC-1150", "252", "240", "0", "44", "536"],
        ],
        widths=[10, 16, 12, 16, 10, 10, 10, 12],
        merge_cols=(0,), center_cols=(3, 4, 5, 6, 7))

    row = kit.note(ws, row, 2,
                   "※ 数量は当月の増分で数える（ADR-008）。キャパシティ試算 "
                   "2.月次推移 は累計なので、その差が当月の課金チャンク数に"
                   "あたる。2026/04 が大きいのは既存文書の初期一括取り込みを"
                   "含むためである。")
    row = kit.note(ws, row, 2,
                   "※ 2026/06 からリランクの費用が発生している（品質保証部のみ）。")
    kit.note(ws, row, 2,
             "※ 法務部（極秘）は社内 GPU の voyage-4-nano で埋め込むため、"
             "エンベディングの単価が他部門と異なる（2.単価 を参照）。"
             "GPU の占有秒による按分は 社内GPU の欄に別建てで計上する"
             "（42,000 チャンク ÷ 32 件/回 × 2.4 秒 = 3,150 秒。"
             "× 0.0140 円/秒 = 44 円）。")
    kit.set_print(ws, doc_name=info.doc_name)

    ws = kit.add_sheet(wb, "2.単価")
    row = kit.heading(ws, 2, 2, "2. 適用した単価")
    kit.table(ws, top=row, left=2,
              header=["適用月", "品目", "単価（円）", "根拠"],
              rows=[
                  ["2026/04-", "エンベディング（チャンク・外部API）", "0.0180",
                   "voyage-4 の請求実績から按分"],
                  ["2026/06-", "エンベディング（チャンク・社内GPU）", "0.0060",
                   "voyage-4-nano。社内 GPU の運用費のうち占有秒に依らない分"
                   "（モデル配信・監視）をチャンク数で按分。法務部のみ"],
                  ["2026/04-", "検索（呼び出し）", "0.0400", "計算資源と運用費の按分"],
                  ["2026/04-", "リランク（呼び出し）", "0.1200",
                   "rerank-2.5 の請求実績から按分"],
                  ["2026/02-", "社内GPU（占有秒）", "0.0140",
                   "減価償却と電力を占有秒で按分"],
              ], widths=[12, 34, 14, 58], center_cols=(2,))
    kit.set_print(ws, doc_name=info.doc_name)

    # 母集合を外部 API に揃える。1.月次実績 のうち外部 API に由来するのは
    # エンベディング（外部API分）とリランクだけで、検索（計算資源）と
    # 社内GPU（占有秒）は自社の費用なのでこの表には入れない。
    #   2026/04  3,240 + 7,560                   = 10,800
    #   2026/05  720 + 13,680                    = 14,400
    #   2026/06  720 + 15,480 + 4,656            = 20,856
    ws = kit.add_sheet(wb, "3.端数")
    row = kit.heading(ws, 2, 2, "3. 按分と外部 API 実額の差")
    row = kit.table(ws, top=row, left=2,
                    header=["年月", "外部APIの実額", "テナントへの按分（外部API分）",
                            "差額", "扱い"],
                    rows=[
                        ["2026/04", "12,600", "10,800", "1,800", "基盤の持ち出し"],
                        ["2026/05", "17,120", "14,400", "2,720", "基盤の持ち出し"],
                        ["2026/06", "23,364", "20,856", "2,508", "基盤の持ち出し"],
                    ], widths=[12, 16, 26, 12, 24], center_cols=(1, 2, 3))
    kit.note(ws, row, 2,
             "※ この表の対象は外部 API（voyage-4 のエンベディングと "
             "rerank-2.5 のリランク）だけである。検索（計算資源）と"
             "社内GPU（占有秒）は自社の費用なので含めない。")
    row += 2
    kit.note(ws, row, 2,
             "※ 差額はテナントへ紐づけられない利用（基盤側の動作確認・"
             "モデルの評価・取り込み失敗分の再実行）である。按分は"
             "単価 × 数量で求め円未満を切り捨てるが、2026/04-06 は"
             "いずれも割り切れており切り捨ては発生していない。"
             "差額は AI基盤グループの予算で負担する（経理部と合意済み）。")
    kit.set_print(ws, doc_name=info.doc_name)

    return kit.save(wb, out / "テナント別コスト実績.xlsx")
