"""稟議書・体制・要員計画・運用引継ぎ資料。

日本企業の社内基盤に必ず付く 3 冊。**スコープの争点（README の仕込み D1）は
稟議書のスコープ欄にある**。
"""

from __future__ import annotations

from pathlib import Path

import sheetkit as kit
import spec


def build(out: Path) -> list[Path]:
    return [_ringi(out), _taisei(out), _hikitsugi(out)]


# ── 稟議書 ────────────────────────────────────────────────────────
def _ringi(out: Path) -> Path:
    info = kit.DocInfo(
        doc_name="稟議書（社内エンベディング基盤の構築）",
        version="1.2", date="2026/02/26", author="田村",
        revisions=[
            ("1.0", "2026/02/12", "初版作成", "田村"),
            ("1.1", "2026/02/19", "投資額の内訳を修正（GPU 2 枚へ）", "田村"),
            ("1.2", "2026/02/26", "経理部の指摘によりチャージバックの記述を追加", "田村"),
        ])
    wb = kit.new_book()
    kit.three_sheet(wb, info)

    ws = kit.add_sheet(wb, "1.趣旨")
    row = kit.heading(ws, 2, 2, "1. 趣旨")
    kit.kv_table(ws, top=row, left=2, pairs=[
        ("件名", "社内エンベディング基盤（Kotonoha）の構築および運用"),
        ("起案部門", f"{kit.DIVISION} {kit.GROUP}"),
        ("実施時期", "2026年4月 サービス開始（2025年度に PoC 実施済み）"),
        ("投資額", "初年度 3,850 万円（内訳は 3.投資内訳）"),
        ("実施方式", "内製。運用の一部（監視・一次受け）を"
                    f"{kit.VENDOR}へ委託"),
    ], width=76, label_width=16)
    kit.set_print(ws, doc_name=info.doc_name)

    # **列幅はシート単位の資源**なので、幅の違う表は章ごとにシートを分ける
    # （sales-corpus の xlsxkit が LayoutError で弾く）。
    ws = kit.add_sheet(wb, "2.背景")
    row = kit.heading(ws, 2, 2, "2. 背景")
    kit.note(ws, row, 2,
             "社内に技術文書・保守マニュアル・問合せ履歴が大量に蓄積しているが、"
             "全文検索では言い換えに当たらず、必要な情報に辿り着けない。")
    row += 2
    kit.table(ws, top=row, left=2,
              header=["No", "現状の課題", "影響を受けている部門"],
              rows=[
                  ["1", "保守問合せの回答に過去事例を探す時間が掛かる（平均 18 分）",
                   "カスタマーサポート部"],
                  ["2", "不具合報告の重複に気づけず、同じ調査を繰り返している",
                   "品質保証部"],
                  ["3", "契約書のひな形との差分を目視で確認しており漏れが出る",
                   "法務部"],
                  ["4", "過去の提案書を再利用したいが探せない", "営業本部"],
                  ["5", "各部門が個別に PoC を始めており、費用と知見が分散している",
                   "全社"],
              ], widths=[5, 62, 22], center_cols=(0,))
    kit.set_print(ws, doc_name=info.doc_name)

    # ★ スコープ欄。CS部門との争点（仕込み D1）がここに効く。
    ws = kit.add_sheet(wb, "2.スコープ")
    row = kit.heading(ws, 2, 2, "スコープ")
    row = kit.kv_group_table(ws, top=row, left=2, groups=[
        ("含む", [
            ("文書の取り込み", "各部門の文書を分割し、埋め込んで格納する"),
            ("エンベディング生成", "テキストをベクトルにする API を提供する"),
            ("検索", "ベクトルと全文を融合した検索 API を提供する"),
            ("テナント管理", "利用部門ごとの分離・上限・API キー"),
            ("チャージバック", "使用量に応じた費用の按分"),
        ]),
        ("含まない", [
            ("回答文の生成", "検索結果をもとに回答文を作る処理は各部門で実施する"),
            ("利用部門の画面", "各部門が自部門のシステムに組み込む"),
            ("文書の一次管理", "原本の管理は各部門の既存システムで行う"),
            ("OCR", "画像のみの PDF は対象外とする"),
        ]),
    ], group_width=10, label_width=22, width=64)
    kit.note(ws, row, 2,
             "※ PoC の報告会では回答文の生成までデモしたが、本基盤の"
             "スコープは検索までとする。")
    kit.set_print(ws, doc_name=info.doc_name)

    ws = kit.add_sheet(wb, "3.投資内訳")
    row = kit.heading(ws, 2, 2, "投資内訳（初年度）")
    kit.table(ws, top=row, left=2,
              caption="単位: 万円",
              header=["区分", "項目", "金額", "備考"],
              rows=[
                  ["設備", "GPU サーバ（A100 x2）", "1,850", "極秘区分の埋め込み用"],
                  ["設備", "DB・検索基盤の増強", "420", "PostgreSQL / OpenSearch"],
                  ["運用", "外部 API 利用料", "680", "初年度見込み。翌年度以降は各部門へ按分"],
                  ["運用", "監視・一次受けの委託", "540", kit.VENDOR_SHORT],
                  ["人件", "構築要員", "360", "内製 6 名（既存要員の工数振替）"],
                  ["予備", "予備費", "—", "総額の 10% を上限に部長決裁で執行"],
              ], widths=[8, 34, 10, 40], merge_cols=(0,), center_cols=(2,))
    kit.set_print(ws, doc_name=info.doc_name)

    ws = kit.add_sheet(wb, "4.効果")
    row = kit.heading(ws, 2, 2, "期待する効果")
    kit.table(ws, top=row, left=2,
              header=["No", "効果", "測り方", "目標"],
              rows=[
                  ["1", "保守問合せの回答時間の短縮", "問合せ 1 件あたりの平均対応時間",
                   "18 分 → 12 分"],
                  ["2", "不具合調査の重複削減", "重複と判明した調査の件数", "四半期 20 件 → 5 件"],
                  ["3", "契約審査の漏れ防止", "審査後に発覚した条項の相違", "年 3 件 → 0 件"],
                  ["4", "PoC の集約", "部門個別の PoC 件数", "4 件 → 0 件"],
              ], widths=[5, 32, 34, 22], center_cols=(0,))
    kit.set_print(ws, doc_name=info.doc_name)

    ws = kit.add_sheet(wb, "5.リスク")
    row = kit.heading(ws, 2, 2, "想定されるリスクと対応")
    kit.table(ws, top=row, left=2,
              header=["No", "リスク", "対応"],
              rows=[
                  ["1", "極秘情報が外部サービスへ送出される",
                   "機密区分 30 は社内 GPU で処理し、外部 API を呼ばない仕組みを"
                   "コードで担保する。情報セキュリティ部の点検を受ける"],
                  ["2", "外部 API の値上げ・提供停止",
                   "モデルの差し替えができる構造にする。オープンウェイトの"
                   "モデルを社内に持つことで最低限の継続性を確保する"],
                  ["3", "利用が伸びず投資が回収できない",
                   "PoC で先行している 2 部門を初年度の利用者として確保済み"],
                  ["4", "運用要員の不足",
                   "監視と一次受けを委託する。二次受けは内製で持つ"],
                  ["5", "検索精度が業務に足りない",
                   "PoC で評価済み（詳細は PoC評価報告書）。"
                   "精度の継続測定は今後の課題とする"],
              ], widths=[5, 30, 58], center_cols=(0,))
    kit.set_print(ws, doc_name=info.doc_name)

    return kit.save(wb, out / "稟議書_AI基盤構築.xlsx")


# ── 体制・要員計画（図形＋コネクタ）──────────────────────────────
def _taisei(out: Path) -> Path:
    info = kit.DocInfo(doc_name="体制・要員計画", version="1.1",
                       date="2026/03/24", author="大城")
    wb = kit.new_book()
    kit.three_sheet(wb, info)

    ws = kit.add_sheet(wb, "体制図", grid=True)
    kit.set_print(ws, doc_name=info.doc_name)
    # 凡例はセル由来なので図形とは別のアンカーに出る。
    ws["A1"] = "凡例: 実線=指揮命令 / 破線=委託"
    ws["A2"] = "作成: 2026/03/24"

    diagram = kit.Diagram(sheet="体制図", nodes=[
        kit.Node("hombu", f"{kit.DIVISION}\n本部長 大城", col=14, row=2, w=14, h=3),
        kit.Node("group", f"{kit.GROUP}\n課長 田村", col=14, row=7, w=14, h=3),
        kit.Node("dev", "基盤開発\n田村・小島・他2名", col=4, row=12, w=13, h=3),
        kit.Node("ops", "運用\n小島・他1名", col=20, row=12, w=13, h=3),
        kit.Node("vendor", f"{kit.VENDOR_SHORT}\n監視・一次受け",
                 col=34, row=12, w=13, h=3, fill="FFF2CC", line="BF8F00"),
        kit.Node("infosec", "情報セキュリティ部\n（点検）",
                 col=34, row=7, w=13, h=3, fill="FCE4D6", line="C55A11"),
        kit.Node("users", "利用部門（4 部門）\nCS・品証・法務・営業",
                 col=4, row=17, w=13, h=3, fill="E2EFDA", line="548235"),
        kit.Node("keiri", "経理部\n（チャージバック）",
                 col=20, row=17, w=13, h=3, fill="E2EFDA", line="548235"),
    ], edges=[
        kit.Edge("hombu", "group"),
        kit.Edge("group", "dev"),
        kit.Edge("group", "ops"),
        # ★ 凡例（A1）が「実線=指揮命令 / 破線=委託」と宣言しているので、
        #    委託の線は実際に破線で描く。ここを実線のままにすると、凡例が
        #    約束した描き分けが drawing XML に無いことになる。
        kit.Edge("ops", "vendor", "委託", dash="dash"),
        kit.Edge("hombu", "infosec", "点検依頼"),
        kit.Edge("dev", "users", "提供"),
        kit.Edge("ops", "keiri", "月次締め"),
    ])

    ws = kit.add_sheet(wb, "要員計画")
    row = kit.heading(ws, 2, 2, "要員計画")
    kit.table(ws, top=row, left=2,
              header=["区分", "氏名", "役割", "工数", "備考"],
              rows=[
                  ["内製", "大城", "本部長（決裁）", "0.1", ""],
                  ["内製", "田村", "課長・基盤開発", "0.8", "PoC からの継続"],
                  ["内製", "小島", "基盤開発・運用", "1.0", "PoC からの継続"],
                  ["内製", "（要員 A）", "基盤開発", "1.0", "2026/04 着任"],
                  ["内製", "（要員 B）", "基盤開発", "1.0", "2026/04 着任"],
                  ["内製", "（要員 C）", "運用", "0.5", "他業務と兼務"],
                  ["委託", kit.VENDOR_SHORT, "監視・夜間一次受け", "—",
                   "2026/04 から。契約は年度更新"],
              ], widths=[8, 14, 26, 8, 32], merge_cols=(0,), center_cols=(3,))
    kit.set_print(ws, doc_name=info.doc_name)

    return kit.save(wb, out / "体制・要員計画.xlsx", diagrams=[diagram])


# ── 運用引継ぎ資料 ────────────────────────────────────────────────
def _hikitsugi(out: Path) -> Path:
    info = kit.DocInfo(doc_name="運用引継ぎ資料（委託範囲）", version="1.0",
                       date="2026/03/27", author="小島")
    wb = kit.new_book()
    kit.three_sheet(wb, info)

    ws = kit.add_sheet(wb, "1.委託範囲")
    row = kit.heading(ws, 2, 2, f"1. {kit.VENDOR}への委託範囲")
    row = kit.table(ws, top=row, left=2,
                    header=["区分", "作業", "委託", "内製", "備考"],
                    rows=[
                        ["監視", "警報の一次受け", "○", "", "夜間・休日を含む"],
                        ["監視", "ダッシュボードの保守", "○", "", "Grafana"],
                        ["監視", "警報のしきい値の変更", "", "○", "内製が決める"],
                        ["障害", "一次切り分け", "○", "", "手順書の範囲"],
                        ["障害", "二次対応", "", "○", "平日 8:00-20:00"],
                        ["障害", "極秘に関わる事象", "", "○",
                         "★ 時間帯に関わらず内製が対応する"],
                        ["定常", "取り込み滞留の解消", "○", "", "ワーカの増減のみ"],
                        ["定常", "月次締め", "", "○", "経理との突合があるため"],
                        ["定常", "再インデックス", "", "○", "影響が大きいため"],
                        ["変更", "設定の変更", "", "○", ""],
                        ["変更", "モデルの変更", "", "○", ""],
                    ], widths=[8, 34, 6, 6, 36], merge_cols=(0,),
                    center_cols=(2, 3))

    kit.note(ws, row, 2,
             "※ 極秘（機密区分 30）に関わる事象は、委託先へ情報を渡さずに"
             "内製で対応する。委託先は極秘テナントのデータに触れられない。")
    kit.set_print(ws, doc_name=info.doc_name)

    ws = kit.add_sheet(wb, "2.連絡体制")
    row = kit.heading(ws, 2, 2, "2. 連絡体制")
    kit.table(ws, top=row, left=2,
              header=["時間帯", "一次", "二次", "連絡手段"],
              rows=[
                  ["平日 8:00-20:00", f"{kit.GROUP}", "課長 田村", "社内チャット・電話"],
                  ["平日 20:00-8:00", kit.VENDOR_SHORT, f"{kit.GROUP}（当番）", "電話"],
                  ["休日", kit.VENDOR_SHORT, f"{kit.GROUP}（当番）", "電話"],
              ], widths=[18, 24, 24, 26])
    kit.set_print(ws, doc_name=info.doc_name)

    ws = kit.add_sheet(wb, "3.SLO")
    row = kit.heading(ws, 2, 2, "3. サービス水準")
    kit.table(ws, top=row, left=2,
              header=["指標", "目標", "測り方", "確認"],
              rows=[[name, target, how, cycle]
                    for name, target, how, cycle in spec.SLOS],
              widths=[24, 28, 30, 10])
    kit.note(ws, row + len(spec.SLOS) + 3, 2,
             "※ 対象は平日 8:00-20:00。夜間・休日は best effort とする。")
    kit.set_print(ws, doc_name=info.doc_name)

    return kit.save(wb, out / "運用引継ぎ資料.xlsx")
