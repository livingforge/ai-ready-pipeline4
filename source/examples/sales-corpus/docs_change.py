"""追加資料（第2次リリース）の 8 文書。

``資料/`` が第1次リリースの設計書一式であるのに対し、ここが出すのは**そのあとに
届いた 2 度目の束**である。中身は 2 種類しかない。

1. **是正** ―― ``資料/`` に仕込んだ食い違い（README の C1〜C5・G6・G7）の決着。
   変更管理委員会が寄せ先を決め、**変更前・変更後・決定理由・決定日・承認者**を
   残した形で書く。差し替え版ではないので、``資料/`` の記述はそのまま残っている
   ―― 整理層は「新しい決定が古い記述を置き換えた」と読めなければならない。
2. **追加** ―― 第2次リリースで足す返品管理。要件・業務ルール・コード・テーブル・
   画面・モジュール・処理仕様まで一式ある。

``資料/`` と同じ決めごとで書く。**採番しない**（対応は名前で引く）、**セル結合と
図表を厚く使う**（2 段見出し・縦結合・表題の帯・図形＋コネクタ・結合セルの作図）、
**1 シート = 1 章**。内容の正本は :mod:`spec_change`。

図表: 業務フロー（返品）・返品の状態遷移図・ER図（追加分）・処理フロー（返品受付）
の 4 シートが図形＋コネクタ、返品入力のレイアウトが結合セルの作図。
"""

from __future__ import annotations

from pathlib import Path

import spec
import spec_change as sc
import xlsxkit as xk
from xlsxkit import Box, Diagram, DocInfo, Edge, Node

#: 追加資料の置き場（``資料/`` と同じ「サブシステム別バインダ ＞ 工程」の 2 階層）。
DIR_CHANGE = "00_全体/04_変更管理"


def build(out: Path) -> list[Path]:
    return [
        _change_control(out),
        _minutes_committee(out),
        _req_delta(out),
        _basic_delta(out),
        _table_delta(out),
        _screen_delta(out),
        _module_delta(out),
        _proc_return_regist(out),
    ]


# ── 設計変更管理表 ────────────────────────────────────────────────
def _change_control(out: Path) -> Path:
    info = DocInfo(
        doc_name="設計変更管理表",
        subsystem="全体",
        version="1.1",
        date="2026/02/13",
        author=xk.VENDOR,
        revisions=[
            ("1.0", "2026/02/09", "変更管理委員会（2026/02/06）の決定を起票", "佐藤"),
            ("1.1", "2026/02/13", "影響分析と申し送り事項を追加", "佐藤"),
        ],
    )

    def body(wb, info):
        ws = xk.add_sheet(wb, "1.変更管理の方針")
        r = xk.heading(ws, 2, 2, "1. 変更管理の方針")
        xk.kv_group_table(
            ws,
            top=r,
            left=2,
            groups=[
                ("位置づけ", [
                    ("目的", "第1次リリースの設計書どうしの食い違い、および設計書と実装の"
                            "食い違いを決着させ、あわせて第2次リリースで追加する仕様を確定する。"),
                    ("対象", "要件定義書・基本設計書・詳細設計書・帳票および実装コード。"
                            "本書は差し替え版ではなく、変更の記録である。"),
                    ("適用範囲", "本書に載せた変更のみを正式な変更として扱う。"
                              "本書に無い記述は第1次リリースの設計書のとおりとする。"),
                ]),
                ("決定の場", [
                    ("会議体", "変更管理委員会（発注者・受託者の合同）"),
                    ("開催日", f"{sc.COMMITTEE_DATE}（第1回）"),
                    ("決定の要件", "発注者の情報システム部長と受託者の PM の双方が承認したものを"
                                "決定とする。業務に関わる変更は所管部門の部長の承認を要する。"),
                ]),
                ("リリース", [
                    ("第1次リリース", "2026年4月1日。是正のみを反映する。"),
                    ("第2次リリース", f"{sc.RELEASE_DATE}。返品管理を追加する。"),
                    ("反映の考え方", "是正は上流文書（要件定義・基本設計）まで遡って直す。"
                                "実装だけが直っていたものは設計書を実装に合わせる。"),
                ]),
            ],
            group_width=14,
            label_width=20,
            width=88,
        )
        xk.set_print(ws, doc_name=info.doc_name)

        ws2 = xk.add_sheet(wb, "2.変更要求一覧")
        r = xk.heading(ws2, 2, 2, "2. 変更要求一覧")
        r = xk.table(
            ws2,
            top=r,
            left=2,
            caption="変更要求一覧（受付順）",
            groups=[("", 1), ("起票", 2), ("", 1), ("判定・反映", 3)],
            header=["No", "区分", "要求元", "変更要求の内容", "判定", "反映先の工程", "反映するリリース"],
            rows=[
                [str(i + 1), kind, src, text, judge, phase, release]
                for i, (kind, src, text, judge, phase, release) in enumerate(sc.CHANGE_REQUESTS)
            ],
            widths=[5, 8, 16, 76, 8, 22, 16],
            merge_cols=(1,),
            center_cols=(0, 1, 4, 6),
        )
        xk.note(ws2, r, 2, "※ 要求の文言は起票された原文のまま載せている（「返却」は返品のこと）。")
        xk.set_print(ws2, doc_name=info.doc_name)

        # 決定事項は 1 論点あたり 6 行の縦持ちにし、論点の列を縦結合する。
        ws3 = xk.add_sheet(wb, "3.決定事項")
        r = xk.heading(ws3, 2, 2, "3. 決定事項")
        r = xk.note(ws3, r, 2,
                    "※ 変更前の記述は第1次リリースの設計書に残っている。"
                    "本書はそれを置き換える決定であって、設計書の差し替えではない。")
        xk.kv_group_table(
            ws3,
            top=r,
            left=2,
            groups=[
                (topic, [
                    ("変更前", before),
                    ("変更後", after),
                    ("決定理由", reason),
                    ("決定日", date),
                    ("承認者", approver),
                    ("経緯の出典", origin),
                ])
                for topic, before, after, reason, date, approver, origin in sc.DECISIONS
            ],
            group_width=22,
            label_width=14,
            width=94,
        )
        xk.set_print(ws3, doc_name=info.doc_name)

        ws4 = xk.add_sheet(wb, "4.影響分析")
        r = xk.heading(ws4, 2, 2, "4. 影響分析（決定を反映する先）")
        r = xk.table(
            ws4,
            top=r,
            left=2,
            caption="影響を受ける文書と反映状況",
            groups=[("", 1), ("反映先", 2), ("", 1), ("", 1)],
            header=["論点", "対象の文書", "対象のシート・章", "反映内容", "状態"],
            rows=[[topic, doc, sheet, text, state] for topic, doc, sheet, text, state in sc.IMPACTS],
            widths=[24, 30, 26, 60, 20],
            merge_cols=(0,),
            center_cols=(4,),
        )
        xk.note(ws4, r, 2, "※「本書で改訂」は、この束（追加資料）の各文書で改訂したことを指す。")
        xk.set_print(ws4, doc_name=info.doc_name)

        ws5 = xk.add_sheet(wb, "5.申し送り事項")
        r = xk.heading(ws5, 2, 2, "5. 申し送り事項（本書では閉じないもの）")
        xk.table(
            ws5,
            top=r,
            left=2,
            caption="申し送り事項",
            groups=[("", 1), ("", 1), ("対応", 2)],
            header=["件名", "内容", "対応する工程", "担当"],
            rows=[[name, text, phase, owner] for name, text, phase, owner in sc.CARRY_OVERS],
            widths=[26, 84, 24, 10],
            center_cols=(2, 3),
            header_fill=xk.FILL_HEAD2,
        )
        xk.set_print(ws5, doc_name=info.doc_name)

    return xk.build(out, f"{DIR_CHANGE}/設計変更管理表.xlsx", info, body)


# ── 変更管理委員会 議事録 ─────────────────────────────────────────
def _minutes_committee(out: Path) -> Path:
    """議事録は表紙を付けない（``資料/`` の 2 本と同じ組み立て）。"""
    doc_name = "第1回 変更管理委員会 議事録"
    wb = xk.new_book()

    ws = xk.add_sheet(wb, "議事録")
    r = xk.heading(ws, 2, 2, doc_name)
    r = xk.kv_group_table(
        ws,
        top=r,
        left=2,
        groups=[
            ("開催情報", [
                ("日時", "2026年2月6日（金）13:30〜17:00"),
                ("場所", f"{xk.CLIENT_SHORT} 本社 3F 大会議室 / Web 併用"),
                ("目的", "結合テストで判明した設計書どうし・設計書と実装の食い違いの決着と、"
                        "第2次リリースで追加する仕様の承認"),
            ]),
            ("出席者", [
                (xk.CLIENT_SHORT, "情報システム部 田中部長、佐々木課長、営業部 大西次長、"
                                  "物流部 森本課長、経理部 小林課長"),
                ("ベンダ", "佐藤PM、山田、鈴木、高橋"),
                ("欠席", "なし"),
            ]),
        ],
        group_width=10,
        label_width=24,
        width=84,
    )
    xk.table(
        ws,
        top=r,
        left=2,
        caption="議事内容",
        header=["No", "議題", "決定事項・討議内容"],
        rows=[
            ["1", "在庫引当のタイミング",
             "物流部の要望どおり、引当は出荷指示の実行時とすることで決定。"
             "詳細設計と実装は既にこの方式で、結合テストでも問題は出ていない。"
             "要件定義書と基本設計書だけが受注確定時のまま残っているので、そちらを直す。"
             "基本設計レビュー（2025年8月22日）からの持ち越し課題を閉じる。"],
            ["2", "受注取消の期限",
             "営業部より、翌営業日を過ぎても出荷前なら取り消せるのが実態との説明。"
             "出荷指示済より前であることを条件とし、日数による制限は置かないことで決定。"
             "出荷後の戻しは返品で扱えるようになるため、運用上も困らないことを確認した。"],
            ["3", "消費税の計算単位",
             "経理部より、明細単位・切り捨てで問題ないとの回答。適格請求書の要件は"
             "税率区分ごとの表示で満たせるため、請求単位での再計算は行わない。"
             "詳細設計（処理仕様書_請求締め）と請求書の様式イメージを直す。"
             "★実装（請求締めバッチ）が請求単位・四捨五入のままである点は申し送りとする。"],
            ["4", "請求の締め日",
             "要件定義書の「毎月20日」は 20日締めの得意先だけを見た記述だったことを確認。"
             "締日区分に従い、締め日が休日のときは前営業日へ繰り上げる形で上流を直す。"
             "要件定義レビュー（2025年5月23日）で経理部から出ていた指摘の反映漏れである。"],
            ["5", "設計書と実装の食い違い",
             "品質管理より、受注明細の上限行数（設計50行／実装100行）と採番のロック待ち"
             "（設計10秒／実装30秒）の報告。いずれも運用上の必要から実装だけを直しており、"
             "設計書を実装に合わせる。得意先コードの8桁前ゼロ埋めも実装にしか無いため"
             "業務ルールとして明記する。"],
            ["6", "モジュール定義の無い機能",
             "受注内容の変更・在庫照会・売掛残高管理・マスタ保守の4機能は、機能要件に"
             "あるのに詳細設計のモジュール一覧に無く、実装だけが存在していた。"
             "モジュール一覧へ追加してトレーサビリティを通す。"],
            ["7", "返品管理の追加",
             "営業部・物流部・経理部の3部門から要求があり、第2次リリース"
             f"（{sc.RELEASE_DATE}稼働）で追加することで承認。"
             "受付・承認・入庫・売上戻しの4段階とし、良品と不良品を分けて在庫へ戻す。"
             "第1次リリースには含めない。"],
            ["8", "与信保留のワークフロー化",
             "営業部からの要求。今回は保留とし、第3次以降で改めて要否を判断する。"
             "当面はメールによる承認を継続する。"],
        ],
        widths=[10, 24, 84],
        center_cols=(0,),
    )
    xk.set_print(ws, doc_name=doc_name)

    ws2 = xk.add_sheet(wb, "宿題事項")
    r = xk.heading(ws2, 2, 2, "宿題事項")
    xk.table(
        ws2,
        top=r,
        left=2,
        caption="宿題事項",
        groups=[("", 1), ("", 1), ("対応", 2)],
        header=["No", "内容", "担当", "期限"],
        rows=[
            ["1", "決定事項を設計変更管理表に起票し、影響を受ける文書を洗い出す", "佐藤", "2026/02/13"],
            ["2", "要件定義書・基本設計書の改訂分を作成する", "山田", "2026/02/27"],
            ["3", "請求締めバッチの端数処理を切り捨てへ修正する（実装）", "高橋", "2026/03/06"],
            ["4", "返品管理のテーブル・画面・モジュールを設計する", "鈴木", "2026/03/13"],
            ["5", "請求書の様式に税率区分ごとの欄を足す（第2次リリース）", "高橋", "2026/05/29"],
        ],
        widths=[8, 70, 16, 14],
        center_cols=(0, 2, 3),
        header_fill=xk.FILL_HEAD2,
    )
    xk.set_print(ws2, doc_name=doc_name)
    return xk.save(wb, out / "90_議事録/20260206_変更管理委員会議事録.xlsx")


# ── 要件定義書 第2版差分 ──────────────────────────────────────────
def _req_delta(out: Path) -> Path:
    info = DocInfo(
        doc_name="要件定義書（第2版差分）",
        subsystem="全体",
        version="2.0",
        date="2026/02/20",
        revisions=[
            ("2.0", "2026/02/20",
             f"変更管理委員会（{sc.COMMITTEE_DATE}）の決定により、機能要件 3 件を改訂し"
             "返品管理の要件を追加", "山田"),
        ],
    )

    def body(wb, info):
        ws = xk.add_sheet(wb, "1.改訂の方針")
        r = xk.heading(ws, 2, 2, "1. 改訂の方針")
        xk.kv_group_table(
            ws,
            top=r,
            left=2,
            groups=[
                ("本書の位置づけ", [
                    ("対象", "新販売管理システム_要件定義書（第1.1版）に対する差分。"
                            "本書に載せた要件のみが改訂・追加の対象で、ほかは第1.1版のとおり。"),
                    ("根拠", f"変更管理委員会（{sc.COMMITTEE_DATE}）の決定。"
                            "詳細は設計変更管理表 3.決定事項を参照。"),
                    ("読み方", "改訂した要件は変更前・変更後を併記する。"
                            "第1.1版の記述は取り消し線を引かずに残してある。"),
                ]),
                ("改訂の理由", [
                    ("実態との乖離", "在庫引当のタイミング・受注取消の期限・請求の締め日は、"
                                "詳細設計と実装が先に実態へ合っており、要件定義だけが"
                                "取り残されていた。"),
                    ("反映漏れ", "請求の締め日は要件定義レビュー（2025年5月23日）で"
                            "指摘を受けていたが、要件定義書へ反映されないまま基本設計へ進んでいた。"),
                ]),
                ("追加の理由", [
                    ("業務要求", "出荷後に商品を戻す手段が無く、受注の起こし直しで対応している。"
                            "営業・物流・経理の3部門から返品管理の要求が出た。"),
                    ("リリース", f"返品管理は{sc.RELEASE_NAME}（{sc.RELEASE_DATE}稼働）で提供する。"
                            "第1次リリースの範囲には含めない。"),
                ]),
            ],
            group_width=16,
            label_width=18,
            width=88,
        )
        xk.set_print(ws, doc_name=info.doc_name)

        ws2 = xk.add_sheet(wb, "2.機能要件の改訂")
        r = xk.heading(ws2, 2, 2, "2. 機能要件の改訂")
        r = xk.table(
            ws2,
            top=r,
            left=2,
            caption="機能要件の改訂（第1.1版 → 第2.0版）",
            groups=[("", 1), ("要件", 3), ("要件本文", 2)],
            header=["No", "サブシステム", "分類", "要件名", "変更前（第1.1版）", "変更後（第2.0版）"],
            rows=[
                [str(i + 1), spec.SUBSYSTEM_NAME[sub], kind, name, before, after]
                for i, (name, sub, kind, before, after) in enumerate(sc.REVISED_FUNC_REQS)
            ],
            widths=[5, 14, 12, 20, 62, 62],
            center_cols=(0, 1, 2),
        )
        xk.note(ws2, r, 2, "※ 要件名は変えていない。名前で引いたときに第1.1版と対応が取れるようにするため。")
        xk.set_print(ws2, doc_name=info.doc_name)

        ws3 = xk.add_sheet(wb, "3.追加機能要件")
        r = xk.heading(ws3, 2, 2, "3. 追加する機能要件（返品管理）")
        r = xk.table(
            ws3,
            top=r,
            left=2,
            caption="追加する機能要件",
            groups=[("", 1), ("区分", 2), ("", 1), ("", 1)],
            header=["No", "サブシステム", "分類", "要件名", "要件内容"],
            rows=[
                [str(i + 1), spec.SUBSYSTEM_NAME[sub], kind, name, text]
                for i, (name, sub, kind, text) in enumerate(sc.ADDED_FUNC_REQS)
            ],
            widths=[5, 14, 14, 22, 86],
            merge_cols=(1,),
            center_cols=(0, 1, 2),
        )
        xk.note(ws3, r, 2, f"※ いずれも{sc.RELEASE_NAME}（{sc.RELEASE_DATE}稼働）で提供する。")
        xk.set_print(ws3, doc_name=info.doc_name)

        ws4 = xk.add_sheet(wb, "4.追加非機能要件")
        r = xk.heading(ws4, 2, 2, "4. 追加する非機能要件")
        xk.table(
            ws4,
            top=r,
            left=2,
            caption="追加する非機能要件",
            groups=[("", 1), ("", 1), ("", 1), ("", 1), ("確認", 1)],
            header=["No", "分類", "要件名", "要件内容", "確認方法"],
            rows=[
                [str(i + 1), kind, name, text, how]
                for i, (name, kind, text, how) in enumerate(sc.ADDED_NONFUNC_REQS)
            ],
            widths=[5, 14, 26, 66, 52],
            center_cols=(0, 1),
            header_fill=xk.FILL_HEAD2,
        )
        xk.set_print(ws4, doc_name=info.doc_name)

        ws5 = xk.add_sheet(wb, "5.用語集の追加")
        r = xk.heading(ws5, 2, 2, "5. 用語集への追加")
        r = xk.table(
            ws5,
            top=r,
            left=2,
            caption="追加する用語",
            header=["用語", "説明"],
            rows=[[term, text] for term, text in sc.GLOSSARY_ADD],
            widths=[20, 100],
        )
        xk.note(ws5, r, 2, "※ 第1.1版 6.用語集に追記する。正準の呼び方は左列の語とする。")
        xk.set_print(ws5, doc_name=info.doc_name)

        ws6 = xk.add_sheet(wb, "6.業務フロー", grid=True)
        c = ws6.cell(row=2, column=2, value="6. 業務フロー（返品）")
        c.font = xk.F_SECTION
        for i, text in enumerate(("凡例", "角丸: 作業", "菱形: 判定", "上段が担当部門")):
            cell = ws6.cell(row=6 + i, column=52, value=text)
            cell.font = xk.F_BODY

    diagrams = [
        Diagram(
            sheet="6.業務フロー",
            nodes=[
                Node("cust", "得意先\n返品の連絡", 6, 8, 14, 3, fill="EDEDED", line="808080"),
                Node("entry", "営業担当\n返品入力（返品受付）", 6, 13, 18, 3),
                Node("judge", "営業部長\n承認？", 6, 18, 14, 3, shape="diamond",
                     fill="FCE4D6", line="C55A11"),
                Node("reject", "営業担当\n差し戻し（却下）", 28, 18, 16, 3,
                     fill="F8CBAD", line="C00000"),
                Node("approve", "営業部長\n返品承認", 6, 23, 18, 3),
                Node("ware", "物流\n返品入庫（良品／不良品）", 6, 28, 20, 3),
                Node("acct", "経理\n赤伝として売上戻し", 6, 33, 18, 3),
                Node("close", "経理\n請求締めで当月の請求へ反映", 6, 38, 22, 3),
            ],
            edges=[
                Edge("cust", "entry", "返品の申し出"),
                Edge("entry", "judge", "承認依頼"),
                Edge("judge", "reject", "却下"),
                Edge("judge", "approve", "承認"),
                Edge("approve", "ware", "入庫指示"),
                Edge("ware", "acct", "入庫実績"),
                Edge("acct", "close", "赤伝"),
            ],
        )
    ]
    return xk.build(out, "00_全体/01_要件定義/要件定義書_第2版差分.xlsx", info, body, diagrams)


# ── 基本設計書（システム方式）第2版差分 ────────────────────────────
def _basic_delta(out: Path) -> Path:
    info = DocInfo(
        doc_name="基本設計書（システム方式）第2版差分",
        subsystem="全体",
        version="2.0",
        date="2026/02/27",
        revisions=[
            ("2.0", "2026/02/27",
             f"変更管理委員会（{sc.COMMITTEE_DATE}）の決定により、業務ルール 4 件を改訂し"
             "返品管理の業務ルール・コード定義・状態遷移・権限を追加", "山田"),
        ],
    )

    def body(wb, info):
        ws = xk.add_sheet(wb, "1.業務ルールの改訂")
        r = xk.heading(ws, 2, 2, "1. 業務ルールの改訂")
        r = xk.note(ws, r, 2,
                    "※ 第1.1版 8.業務ルール一覧の該当行を、変更後の本文へ置き換える。")
        xk.table(
            ws,
            top=r,
            left=2,
            caption="業務ルールの改訂（第1.1版 → 第2.0版）",
            groups=[("", 1), ("区分", 2), ("", 1), ("ルール内容", 2)],
            header=["No", "サブシステム", "区分", "ルール名", "変更前（第1.1版）", "変更後（第2.0版）"],
            rows=[
                [str(i + 1), spec.SUBSYSTEM_NAME[sub], kind, name, before, after]
                for i, (name, sub, kind, before, after) in enumerate(sc.REVISED_RULES)
            ],
            widths=[5, 14, 10, 24, 60, 60],
            center_cols=(0, 1, 2),
        )
        xk.set_print(ws, doc_name=info.doc_name)

        ws2 = xk.add_sheet(wb, "2.追加業務ルール")
        r = xk.heading(ws2, 2, 2, "2. 追加する業務ルール")
        r = xk.note(ws2, r, 2,
                    "※ 入力チェックの詳細・端数処理・異常系の扱いは詳細設計書にて定義する。")
        xk.table(
            ws2,
            top=r,
            left=2,
            caption="追加する業務ルール",
            groups=[("区分", 3), ("", 1)],
            header=["サブシステム", "区分", "ルール名", "ルール内容"],
            rows=[
                [spec.SUBSYSTEM_NAME[sub], kind, name, text]
                for name, sub, kind, text in sc.ADDED_RULES
            ],
            widths=[14, 10, 28, 84],
            merge_cols=(0, 1),
            center_cols=(0, 1),
        )
        xk.set_print(ws2, doc_name=info.doc_name)

        ws3 = xk.add_sheet(wb, "3.追加コード定義")
        r = xk.heading(ws3, 2, 2, "3. 追加するコード定義")
        xk.table(
            ws3,
            top=r,
            left=2,
            caption="追加するコード定義",
            groups=[("", 1), ("コード", 2), ("", 1)],
            header=["分類", "コード値", "名称", "内容"],
            rows=[[group, code, name, text] for group, code, name, text in sc.CODES_ADD],
            widths=[18, 12, 18, 72],
            merge_cols=(0,),
            center_cols=(0, 1),
        )
        xk.set_print(ws3, doc_name=info.doc_name)

        ws4 = xk.add_sheet(wb, "4.状態遷移図", grid=True)
        c = ws4.cell(row=2, column=2, value="4. 返品ステータスの状態遷移図")
        c.font = xk.F_SECTION
        for i, text in enumerate(("凡例", "四角内の数字はコード値", "矢印の文字は契機となる操作")):
            cell = ws4.cell(row=6 + i, column=56, value=text)
            cell.font = xk.F_BODY

        ws5 = xk.add_sheet(wb, "5.状態遷移表")
        r = xk.heading(ws5, 2, 2, "5. 返品ステータスの遷移表")
        r = xk.table(
            ws5,
            top=r,
            left=2,
            caption="状態遷移表（行=遷移前、列=操作）",
            groups=[("", 1), ("操作による遷移先", 5)],
            header=["遷移前の状態", "返品受付", "返品承認", "返品却下", "返品入庫", "売上戻し"],
            rows=[list(row) for row in sc.RETURN_TRANSITIONS],
            widths=[20, 16, 16, 16, 16, 18],
            center_cols=(1, 2, 3, 4, 5),
        )
        xk.note(ws5, r, 2,
                "※ 却下（90）からの再申請は、返品を起こし直す運用とする（遷移では戻さない）。")
        xk.set_print(ws5, doc_name=info.doc_name)

        ws6 = xk.add_sheet(wb, "6.権限マトリクスの追加")
        r = xk.heading(ws6, 2, 2, "6. 権限マトリクスへの追加")
        r = xk.table(
            ws6,
            top=r,
            left=2,
            caption="権限マトリクス 追加分（○=可、△=部長職のみ可、×=不可）",
            groups=[("", 1), ("業務ロール", 3), ("", 1)],
            header=["機能", "営業", "物流", "経理", "管理者"],
            rows=[list(row) for row in sc.PERMISSIONS_ADD],
            widths=[34, 10, 10, 10, 10],
            center_cols=(1, 2, 3, 4),
        )
        xk.note(ws6, r, 2, "※ 第1.1版 5.権限マトリクスへ追記する。返品の承認は営業部長のみ行える。")
        xk.set_print(ws6, doc_name=info.doc_name)

    diagrams = [
        Diagram(
            sheet="4.状態遷移図",
            nodes=[
                Node("s10", "受付\n10", 6, 10, 14, 3),
                Node("s20", "承認済\n20", 26, 10, 14, 3),
                Node("s30", "入庫済\n30", 46, 10, 14, 3),
                Node("s40", "売上戻し済\n40", 66, 10, 14, 3),
                Node("s90", "却下\n90", 26, 18, 14, 3, fill="F8CBAD", line="C00000"),
            ],
            edges=[
                Edge("s10", "s20", "返品承認"),
                Edge("s20", "s30", "返品入庫"),
                Edge("s30", "s40", "売上戻し"),
                Edge("s10", "s90", "返品却下"),
            ],
        )
    ]
    return xk.build(
        out, "00_全体/02_基本設計/基本設計書_システム方式_第2版差分.xlsx", info, body, diagrams
    )


# ── テーブル定義書 追加分 ─────────────────────────────────────────
def _table_delta(out: Path) -> Path:
    info = DocInfo(
        doc_name="テーブル定義書（追加分）",
        subsystem="全体",
        version="2.0",
        date="2026/02/27",
        revisions=[
            ("2.0", "2026/02/27", "返品管理のテーブル 2 本と既存テーブルへの列追加 3 件を定義", "鈴木"),
        ],
    )

    def body(wb, info):
        ws = xk.add_sheet(wb, "テーブル一覧")
        r = xk.heading(ws, 2, 2, "テーブル一覧（追加分）")
        r = xk.table(
            ws,
            top=r,
            left=2,
            groups=[("テーブル名", 2), ("", 1), ("", 1), ("", 1)],
            header=["物理名", "論理名", "区分", "内容", "想定件数"],
            rows=[[p, l, k, d, n] for p, l, k, d, n in sc.ENTITIES_ADD],
            widths=[22, 22, 14, 64, 16],
            center_cols=(2, 4),
        )
        xk.note(ws, r, 2,
                f"※ 第1.2版のテーブル一覧に追記する。返品番号の書式は {sc.RETURN_NO_FORMAT}。")
        xk.set_print(ws, doc_name=info.doc_name)

        for phys, logi, _kind, _desc, _n in sc.ENTITIES_ADD:
            ws2 = xk.add_sheet(wb, phys)
            r = xk.heading(ws2, 2, 2, f"{phys}（{logi}）")
            xk.table(
                ws2,
                top=r,
                left=2,
                caption=f"{logi}　列定義",
                groups=[("", 1), ("列名", 2), ("データ型", 2), ("制約", 2), ("", 1)],
                header=["No", "物理名", "論理名", "型", "桁", "キー", "必須", "内容"],
                rows=[
                    [str(i + 1), c_phys, c_logi, c_type, c_len, c_key or "—", c_req, c_desc]
                    for i, (c_phys, c_logi, c_type, c_len, c_key, c_req, c_desc)
                    in enumerate(sc.COLUMNS_ADD[phys])
                ],
                widths=[5, 22, 22, 12, 8, 8, 8, 70],
                center_cols=(0, 3, 4, 5, 6),
            )
            xk.set_print(ws2, doc_name=info.doc_name)

        ws3 = xk.add_sheet(wb, "既存テーブルへの列追加")
        r = xk.heading(ws3, 2, 2, "既存テーブルへの列追加")
        r = xk.table(
            ws3,
            top=r,
            left=2,
            caption="既存テーブルへの列追加",
            groups=[("", 1), ("列名", 2), ("データ型", 2), ("", 1), ("", 1), ("", 1)],
            header=["テーブル", "物理名", "論理名", "型", "桁", "必須", "内容", "追加する理由"],
            rows=[
                [tbl, c_phys, c_logi, c_type, c_len, c_req, c_desc, why]
                for tbl, c_phys, c_logi, c_type, c_len, c_req, c_desc, why in sc.COLUMN_ADDITIONS
            ],
            widths=[20, 22, 20, 12, 8, 8, 56, 56],
            merge_cols=(0,),
            center_cols=(0, 3, 4, 5),
        )
        xk.note(ws3, r, 2,
                "※ 既存データには既定値を設定して移行する（返品済数量は 0、不良品倉庫フラグは 0）。")
        xk.set_print(ws3, doc_name=info.doc_name)

        ws4 = xk.add_sheet(wb, "ER図", grid=True)
        c = ws4.cell(row=2, column=2, value="ER図（追加分と既存テーブルの関連）")
        c.font = xk.F_SECTION
        for i, text in enumerate(
            ("凡例", "実線: 1対多", "網掛け: 既存のテーブル", "上段=物理名 / 下段=論理名")
        ):
            cell = ws4.cell(row=5 + i, column=58, value=text)
            cell.font = xk.F_BODY

        ws5 = xk.add_sheet(wb, "インデックス一覧")
        r = xk.heading(ws5, 2, 2, "インデックス一覧（追加分）")
        xk.table(
            ws5,
            top=r,
            left=2,
            caption="インデックス一覧",
            groups=[("", 1), ("索引", 2), ("", 1), ("", 1)],
            header=["テーブル", "索引名", "構成列", "一意", "用途"],
            rows=[[tbl, name, cols, uniq, use] for tbl, name, cols, uniq, use in sc.INDEXES_ADD],
            widths=[24, 28, 40, 8, 46],
            merge_cols=(0,),
            center_cols=(3,),
            header_fill=xk.FILL_HEAD2,
        )
        xk.set_print(ws5, doc_name=info.doc_name)

    diagrams = [
        Diagram(
            sheet="ER図",
            nodes=[
                Node("cust", "M_CUSTOMER\n得意先マスタ", 4, 8, 16, 3, shape="rect",
                     fill="EDEDED", line="808080"),
                Node("order", "T_ORDER\n受注ヘッダ", 26, 8, 14, 3, shape="rect",
                     fill="EDEDED", line="808080"),
                Node("orderd", "T_ORDER_DETAIL\n受注明細", 46, 8, 16, 3, shape="rect",
                     fill="EDEDED", line="808080"),
                Node("prod", "M_PRODUCT\n商品マスタ", 68, 8, 14, 3, shape="rect",
                     fill="EDEDED", line="808080"),
                Node("ret", "T_RETURN\n返品ヘッダ", 26, 16, 14, 3, shape="rect"),
                Node("retd", "T_RETURN_DETAIL\n返品明細", 46, 16, 16, 3, shape="rect"),
                Node("wh", "M_WAREHOUSE\n倉庫マスタ", 68, 16, 14, 3, shape="rect",
                     fill="EDEDED", line="808080"),
            ],
            edges=[
                Edge("cust", "ret", "1対多"),
                Edge("order", "ret", "1対多"),
                Edge("ret", "retd", "1対多"),
                Edge("orderd", "retd", "1対多"),
                Edge("prod", "retd", "1対多"),
                Edge("wh", "retd", "1対多"),
            ],
        )
    ]
    return xk.build(out, "00_全体/02_基本設計/テーブル定義書_追加分.xlsx", info, body, diagrams)


# ── 受注管理 画面仕様書 追加分 ────────────────────────────────────
#: 返品入力画面のイベント（画面ごとに 1 表）。
_EVENTS = {
    "返品入力": [
        ["受注番号の入力", "フォーカスアウト", "受注ヘッダ・明細を読み込み、得意先名と出荷済の明細を表示する",
         "返品受付の期限 / 得意先コードの正規化"],
        ["明細の選択", "チェックボックス操作", "返品する明細を選び、返品可能数を明細ごとに表示する",
         "返品数量の上限"],
        ["返品数量の入力", "フォーカスアウト", "明細金額と消費税額を計算し直し、返品金額合計を表示する",
         "返品の売上戻し"],
        ["登録ボタン", "クリック", "入力内容を検証して返品を登録する。返品ステータスは 10（受付）とする",
         "返品受付の期限 / 返品数量の上限"],
        ["クリアボタン", "クリック", "入力内容を破棄して初期表示に戻す", "—"],
    ],
    "返品承認": [
        ["検索", "クリック", "返品ステータスが 10（受付）の返品を一覧に表示する", "—"],
        ["明細の展開", "行のクリック", "選んだ返品の明細・品質区分・戻し先倉庫を表示する", "返品の品質区分"],
        ["承認ボタン", "クリック", "返品ステータスを 20（承認済）に更新し、返品入庫を依頼する",
         "返品の品質区分 / 返品の売上戻し"],
        ["却下ボタン", "クリック", "却下理由を入力させ、返品ステータスを 90（却下）に更新する", "—"],
    ],
    "返品一覧照会": [
        ["検索", "クリック", "返品日・取引先・返品ステータスを条件に返品を検索する", "—"],
        ["CSV出力", "クリック", "検索結果を CSV で出力する（最大 10,000 件）", "—"],
        ["明細の表示", "行のクリック", "選んだ返品の明細を表示する", "—"],
    ],
}

#: 返品入力の画面項目（項目名, 物理名, 種別, 型・桁, 必須, 初期値, 備考）
_ITEMS_RETURN = [
    ["返品番号", "returnNo", "表示", "CHAR(12)", "—", "（自動採番）", f"登録時に採番する（{sc.RETURN_NO_FORMAT}）"],
    ["返品日", "returnDate", "入力", "DATE", "必須", "システム日付", "未来日は入力できない"],
    ["受注番号", "orderNo", "入力", "CHAR(12)", "必須", "—", "出荷指示済（40）の受注のみ指定できる"],
    ["得意先コード", "customerCd", "表示", "CHAR(8)", "—", "—", "受注から引く。8桁へ前ゼロ埋めして表示する"],
    ["得意先名", "customerName", "表示", "VARCHAR(120)", "—", "—", "得意先マスタから引く"],
    ["納品日", "deliveryDate", "表示", "DATE", "—", "—", "返品受付の期限の起算日"],
    ["返品可能期限", "returnLimitDate", "表示", "DATE", "—", "—", "納品日 + 返品受付日数（既定 14 日）"],
    ["返品理由区分", "returnReason", "選択", "CHAR(1)", "必須", "—", "1=誤発注、2=品違い、3=数量違い、4=破損、5=期限切れ、9=その他"],
    ["商品コード", "productCd", "表示", "CHAR(10)", "—", "—", "明細行ごと。受注明細から引く"],
    ["出荷数量", "shippedQty", "表示", "DECIMAL(9,2)", "—", "—", "明細行ごと"],
    ["返品可能数", "returnableQty", "表示", "DECIMAL(9,2)", "—", "—", "出荷数量 − 返品済数量"],
    ["返品数量", "returnQty", "入力", "DECIMAL(9,2)", "必須", "—", "返品可能数を超える入力はエラー"],
    ["品質区分", "qualityType", "選択", "CHAR(1)", "必須", "1", "1=良品、2=不良品。不良品は不良品倉庫へ入庫する"],
    ["戻し先倉庫", "warehouseCd", "選択", "CHAR(4)", "必須", "（出荷元倉庫）", "品質区分が不良品のときは不良品倉庫のみ選べる"],
    ["返品金額合計", "totalAmount", "表示", "DECIMAL(12,0)", "—", "0", "明細金額の合計（税抜）"],
    ["消費税額", "taxAmount", "表示", "DECIMAL(12,0)", "—", "0", "明細単位に計算した消費税額の合計"],
    ["登録ボタン", "btnRegist", "ボタン", "—", "—", "—", "返品を登録する"],
    ["クリアボタン", "btnClear", "ボタン", "—", "—", "—", "入力内容を破棄する"],
]


def _mock_return_entry() -> list[Box]:
    """返品入力のレイアウト（結合セルと罫線で描く）。"""
    b: list[Box] = [
        Box(4, 2, 74, 2, "新販売管理システム　返品入力", "title"),
        Box(6, 2, 74, 1, "返品情報", "label"),
    ]
    header = [
        (7, [("返品番号", 10, "（自動採番）", 14, "display"),
             ("返品日", 8, "2026/10/09", 12, "input"),
             ("返品理由", 10, "2: 品違い", 12, "input")]),
        (9, [("受注番号", 10, "R20260925017", 14, "input"),
             ("得意先名", 8, "株式会社　中央フードサービス", 24, "display")]),
        (11, [("納品日", 10, "2026/09/29", 14, "display"),
              ("返品可能期限", 10, "2026/10/13", 12, "display"),
              ("受注ステータス", 10, "40: 出荷指示済", 12, "display")]),
    ]
    for row, cells in header:
        col = 2
        for label, lw, value, vw, kind in cells:
            b.append(Box(row, col, lw, 2, label, "label"))
            b.append(Box(row, col + lw, vw, 2, value, kind))
            col += lw + vw
    b.append(Box(14, 2, 74, 1, "返品明細（出荷済みの受注明細から選ぶ）", "label"))
    cols = [("選択", 4), ("商品コード", 10), ("商品名", 18), ("出荷数量", 8),
            ("返品可能数", 8), ("返品数量", 8), ("品質区分", 8), ("戻し先倉庫", 10)]
    col = 2
    for name, w in cols:
        b.append(Box(15, col, w, 2, name, "title"))
        col += w
    samples = [
        ["■", "4901234055", "米酢 900ml×12", "5", "5", "2", "1: 良品", "0102"],
        ["■", "4901234090", "料理酒 1.8L×6", "10", "10", "3", "2: 不良品", "0901"],
        ["□", "4901234001", "特選しょうゆ 1L×6", "20", "20", "", "", ""],
    ]
    for i, values in enumerate(samples):
        row = 17 + i * 2
        col = 2
        for (name, w), value in zip(cols, values):
            kind = "input" if name in ("選択", "返品数量", "品質区分", "戻し先倉庫") else "display"
            b.append(Box(row, col, w, 2, value, kind))
            col += w
    b += [
        Box(24, 46, 14, 2, "返品金額合計", "label"),
        Box(24, 60, 16, 2, "10,020", "display"),
        Box(26, 46, 14, 2, "消費税額", "label"),
        Box(26, 60, 16, 2, "801", "display"),
        Box(29, 2, 12, 2, "受注読込", "button"),
        Box(29, 16, 12, 2, "クリア", "button"),
        Box(29, 52, 12, 2, "登録", "button"),
        Box(29, 66, 10, 2, "戻る", "button"),
        Box(32, 2, 74, 2,
            "※ 品質区分に「2: 不良品」を選ぶと、戻し先倉庫は不良品倉庫フラグが 1 の倉庫だけに絞られる。",
            "plain"),
        Box(34, 2, 74, 2,
            "※ 返品可能期限を過ぎた受注番号を入力した場合は、読込の時点でエラーを表示する。",
            "plain"),
    ]
    return b


def _screen_delta(out: Path) -> Path:
    name = spec.SUBSYSTEM_NAME["ORD"]
    info = DocInfo(
        doc_name=f"画面仕様書（{name}）追加分",
        subsystem=name,
        version="2.0",
        date="2026/03/06",
        revisions=[
            ("2.0", "2026/03/06", "返品管理の画面 3 本を追加", "山田"),
        ],
    )
    role = "営業・管理者"

    def body(wb, info):
        ws = xk.add_sheet(wb, "画面一覧")
        r = xk.heading(ws, 2, 2, f"{name} 画面一覧（追加分）")
        r = xk.table(
            ws,
            top=r,
            left=2,
            caption=f"{name}　画面一覧（追加分）",
            groups=[("", 1), ("", 1), ("", 1)],
            header=["画面名", "画面区分", "画面概要"],
            rows=[[sname, kind, desc] for sname, _s, kind, desc in sc.SCREENS_ADD],
            widths=[22, 10, 84],
            center_cols=(1,),
        )
        xk.note(ws, r, 2, "※ 第1.1版の画面一覧に追記する。画面遷移図は第2次リリースの基本設計で更新する。")
        xk.set_print(ws, doc_name=info.doc_name)

        wsl = xk.add_sheet(wb, "返品入力 レイアウト", grid=True)
        c = wsl.cell(row=2, column=2, value="返品入力　画面レイアウト")
        c.font = xk.F_SECTION
        xk.layout(wsl, _mock_return_entry())

        for sname, _s, kind, desc in sc.SCREENS_ADD:
            # 画面概要・イベント一覧・業務ルールは B〜E の列幅を共有できるので 1 シートに積む。
            ws2 = xk.add_sheet(wb, sname[:31])
            r = xk.heading(ws2, 2, 2, sname)
            r = xk.kv_group_table(
                ws2,
                top=r,
                left=2,
                groups=[
                    ("画面の位置づけ", [
                        ("画面名", sname),
                        ("画面区分", kind),
                        ("機能概要", desc),
                        ("利用権限", "営業部長・管理者（承認のみ）" if sname == "返品承認" else role),
                        ("リリース", f"{sc.RELEASE_NAME}（{sc.RELEASE_DATE}稼働）"),
                    ]),
                    ("方式", [
                        ("排他制御", "楽観的排他。更新日時が一致しない場合は排他エラーとする"),
                        ("表示件数", "一覧は 1 ページ 50 件とし、ページ送りで表示する"),
                    ]),
                ],
                group_width=22,
                label_width=20,
                width=80,
            )
            events = _EVENTS.get(sname)
            if events:
                r = xk.heading(ws2, r, 2, "イベント・処理一覧")
                r = xk.table(
                    ws2,
                    top=r,
                    left=2,
                    caption=f"{sname}　イベント・処理一覧",
                    groups=[("", 1), ("", 1), ("", 1), ("関連", 1)],
                    header=["イベント", "契機", "処理概要", "関連する業務ルール"],
                    rows=events,
                    widths=[22, 20, 80, 34],
                    header_fill=xk.FILL_HEAD2,
                )
            rules = [
                [kind_, rname, text]
                for rname, rsub, kind_, text in sc.ADDED_RULES
                if rsub in ("ORD", "INV", "BIL")
            ]
            r = xk.heading(ws2, r, 2, "関連する業務ルール")
            xk.table(
                ws2,
                top=r,
                left=2,
                caption="関連する業務ルール（基本設計書 第2.0版差分より転記）",
                header=["区分", "ルール名", "ルール内容"],
                rows=rules,
                widths=[22, 20, 80],
                merge_cols=(0,),
                center_cols=(0,),
                header_fill=xk.FILL_HEAD2,
            )
            xk.set_print(ws2, doc_name=info.doc_name)

        ws3 = xk.add_sheet(wb, "返品入力 項目")
        r = xk.heading(ws3, 2, 2, "返品入力　画面項目一覧")
        xk.table(
            ws3,
            top=r,
            left=2,
            caption="返品入力　画面項目一覧",
            groups=[("", 1), ("項目", 2), ("属性", 3), ("", 1), ("", 1)],
            header=["No", "項目名", "物理名", "種別", "型・桁", "必須", "初期値", "入力チェック・備考"],
            rows=[[str(i + 1)] + row for i, row in enumerate(_ITEMS_RETURN)],
            widths=[5, 20, 18, 8, 16, 8, 20, 64],
            center_cols=(0, 3, 4, 5),
        )
        xk.set_print(ws3, doc_name=info.doc_name)

    return xk.build(out, "10_受注管理/02_基本設計/受注管理_画面仕様書_追加分.xlsx", info, body)


# ── モジュール一覧 追加分 ─────────────────────────────────────────
def _module_delta(out: Path) -> Path:
    info = DocInfo(
        doc_name="モジュール一覧（追加分）",
        subsystem="全体",
        version="2.0",
        date="2026/03/13",
        revisions=[
            ("2.0", "2026/03/13",
             "定義が漏れていたモジュール 4 本と返品管理のモジュール 2 本を追加し、"
             "実装との差異 4 件を反映", "鈴木"),
        ],
    )

    def body(wb, info):
        ws = xk.add_sheet(wb, "1.追加モジュール一覧")
        r = xk.heading(ws, 2, 2, "1. 追加するモジュール一覧")
        r = xk.table(
            ws,
            top=r,
            left=2,
            groups=[("", 1), ("", 1), ("モジュール", 2), ("", 1), ("関連", 2)],
            header=["サブシステム", "区分", "モジュール名", "クラス名", "処理概要",
                    "対応する機能要件", "追加の理由"],
            rows=[
                [spec.SUBSYSTEM_NAME[sub], kind, name, cls, desc, req, why]
                for name, cls, sub, kind, desc, why, req in sc.MODULES_ADD
            ],
            widths=[14, 12, 26, 30, 56, 26, 30],
            merge_cols=(0, 1),
            center_cols=(0, 1),
        )
        xk.note(ws, r, 2,
                "※ 上の 4 本は機能要件にあるのに詳細設計へ起こされておらず、実装だけが"
                "存在していた。下の 2 本は第2次リリースで新規に作る。")
        xk.set_print(ws, doc_name=info.doc_name)

        ws2 = xk.add_sheet(wb, "2.メソッド一覧")
        r = xk.heading(ws2, 2, 2, "2. メソッド一覧（追加分）")
        name_of = {cls: name for name, cls, _s, _k, _d, _w, _r in sc.MODULES_ADD}
        xk.table(
            ws2,
            top=r,
            left=2,
            groups=[("所属モジュール", 2), ("メソッド", 2), ("", 1)],
            header=["モジュール名", "クラス名", "メソッド名", "シグネチャ", "処理概要"],
            rows=[
                [name_of.get(cls, ""), cls, mname, sig, desc]
                for cls, mname, sig, desc in sc.METHODS_ADD
            ],
            widths=[24, 30, 24, 56, 66],
            merge_cols=(0, 1),
        )
        xk.set_print(ws2, doc_name=info.doc_name)

        ws3 = xk.add_sheet(wb, "3.呼出関係")
        r = xk.heading(ws3, 2, 2, "3. モジュール間の呼出関係（追加分）")
        xk.table(
            ws3,
            top=r,
            left=2,
            caption="呼出関係（呼出元 → 呼出先）",
            groups=[("呼出", 2), ("", 1)],
            header=["呼出元", "呼出先", "呼び出す目的"],
            rows=[[src, dst, why] for src, dst, why in sc.CALLS_ADD],
            widths=[28, 32, 60],
            merge_cols=(0,),
        )
        xk.set_print(ws3, doc_name=info.doc_name)

        ws4 = xk.add_sheet(wb, "4.実装との差異の反映")
        r = xk.heading(ws4, 2, 2, "4. 実装との差異の反映")
        r = xk.note(ws4, r, 2,
                    "※ 結合テストで設計書と実装を突き合わせた結果。運用上の必要から実装だけを"
                    "直していたもので、設計書を実装に合わせる。")
        r = xk.table(
            ws4,
            top=r,
            left=2,
            caption="設計書と実装の差異（第1次リリース分）",
            groups=[("", 1), ("差異", 2), ("", 1), ("確認", 1)],
            header=["対象", "設計書の記載（改訂前）", "実装の実際", "改訂後の記載", "確認した実装箇所"],
            rows=[[t, before, impl, after, where]
                  for t, before, impl, after, where in sc.IMPL_ALIGNMENTS],
            widths=[26, 40, 34, 56, 30],
        )
        xk.note(ws4, r, 2,
                "※ 消費税の端数処理だけは逆で、設計（明細単位・切り捨て）へ実装を合わせる。"
                "設計変更管理表 5.申し送り事項を参照。")
        xk.set_print(ws4, doc_name=info.doc_name)

        ws5 = xk.add_sheet(wb, "5.メッセージの追加")
        r = xk.heading(ws5, 2, 2, "5. メッセージの追加")
        xk.table(
            ws5,
            top=r,
            left=2,
            caption="追加するメッセージ",
            groups=[("", 1), ("", 1), ("", 1), ("出力の契機", 1)],
            header=["No", "種別", "メッセージ本文", "出力条件"],
            rows=[
                [str(i + 1), kind, text, cond]
                for i, (kind, text, cond) in enumerate(sc.MESSAGES_ADD)
            ],
            widths=[5, 10, 62, 66],
            merge_cols=(1,),
            center_cols=(0, 1),
            header_fill=xk.FILL_HEAD2,
        )
        xk.set_print(ws5, doc_name=info.doc_name)

    return xk.build(out, "00_全体/03_詳細設計/モジュール一覧_追加分.xlsx", info, body)


# ── 処理仕様書（返品受付）─────────────────────────────────────────
def _proc_return_regist(out: Path) -> Path:
    info = DocInfo(
        doc_name="処理仕様書（返品受付）",
        subsystem=spec.SUBSYSTEM_NAME["ORD"],
        version="1.0",
        date="2026/03/13",
        revisions=[("1.0", "2026/03/13", "初版作成（第2次リリース）", "鈴木")],
    )

    def body(wb, info):
        ws = xk.add_sheet(wb, "1.処理概要")
        r = xk.heading(ws, 2, 2, "1. 処理概要")
        xk.kv_group_table(
            ws,
            top=r,
            left=2,
            groups=[
                ("対象", [
                    ("モジュール名", "返品受付サービス"),
                    ("クラス名", "ReturnRegistService"),
                    ("関連する機能要件", "返品受付"),
                    ("関連する画面", "返品入力"),
                    ("リリース", f"{sc.RELEASE_NAME}（{sc.RELEASE_DATE}稼働）"),
                ]),
                ("呼出関係", [
                    ("呼出元", "ReturnController#regist（返品入力画面の登録ボタン）"),
                    ("呼出先", "NumberingService, TaxCalculator, BusinessDayCalendar, AuditLogger"),
                    ("呼び出さないもの", "★StockUpdateService。在庫への戻しは返品承認サービスが"
                                    "承認後に行う（受付の時点では在庫を動かさない）。"),
                ]),
                ("方式", [
                    ("トランザクション", "本メソッドの開始から終了までを 1 トランザクションとする。"
                                "例外が発生した場合はすべてロールバックする。"),
                    ("排他制御", "受注明細の返品済数量を更新するため、対象の受注明細を"
                            "更新日時で楽観的に排他する。一致しない場合は排他エラーとする。"),
                ]),
                ("処理の要点", [
                    ("処理概要", "返品入力画面から渡された返品データを検証し、元の受注明細の"
                            "販売単価で金額を確定して返品ヘッダ・返品明細を登録する。"),
                    ("対象の受注", "★受注ステータスが出荷指示済（40）の受注だけを対象とする。"
                            "出荷指示より前の戻しは受注取消で処理する。"),
                    ("単価の扱い", "★販売単価は元の受注明細の値をそのまま引き継ぐ。"
                            "返品時点の単価マスタは参照しない。"),
                    ("消費税の扱い", "★明細単位に計算し円未満を切り捨てる"
                            f"（変更管理委員会 {sc.COMMITTEE_DATE} の決定による）。"),
                ]),
            ],
            width=94,
        )
        xk.set_print(ws, doc_name=info.doc_name)

        ws2 = xk.add_sheet(wb, "2.メソッド一覧")
        r = xk.heading(ws2, 2, 2, "2. メソッド一覧")
        xk.table(
            ws2,
            top=r,
            left=2,
            caption="ReturnRegistService　メソッド一覧",
            groups=[("メソッド", 2), ("", 1)],
            header=["メソッド名", "シグネチャ", "処理概要"],
            rows=[[mname, sig, desc]
                  for _c, mname, sig, desc in sc.added_methods_of("ReturnRegistService")],
            widths=[24, 58, 70],
        )
        xk.set_print(ws2, doc_name=info.doc_name)

        ws3 = xk.add_sheet(wb, "3.処理フロー", grid=True)
        c = ws3.cell(row=2, column=2, value="3. 処理フロー（返品受付）")
        c.font = xk.F_SECTION
        for i, text in enumerate(("凡例", "菱形: 判定", "角丸: 処理", "網掛け: 他モジュール呼出")):
            cell = ws3.cell(row=5 + i, column=52, value=text)
            cell.font = xk.F_BODY

        ws4 = xk.add_sheet(wb, "4.処理詳細")
        r = xk.heading(ws4, 2, 2, "4. 処理詳細")
        xk.table(
            ws4,
            top=r,
            left=2,
            caption="処理詳細（ステップ順）",
            groups=[("", 1), ("", 1), ("アクセスするテーブル", 2), ("", 1)],
            header=["No", "処理内容", "参照", "更新", "例外時の動作"],
            rows=[
                ["1", "得意先コードを 8 桁へ前ゼロ埋めして正規化する",
                 "—", "—", "—"],
                ["2", "対象の受注ヘッダを取得し、受注ステータスが出荷指示済（40）であることを確認する",
                 "T_ORDER", "—", "「出荷指示前の受注は返品できません」を返す"],
                ["3", "納品日から返品可能期限を求める。得意先マスタの返品受付日数（未設定なら 14 日）"
                 "を加算し、営業日カレンダーで休日なら翌営業日まで延ばす",
                 "M_CUSTOMER, M_CALENDAR", "—", "期限を過ぎている場合はエラーを返す"],
                ["4", "返品対象の受注明細ごとに、出荷数量から返品済数量を差し引いた返品可能数を求め、"
                 "返品数量がその範囲内であることを確認する",
                 "T_ORDER_DETAIL", "—", "超える場合はエラー一覧を返す"],
                ["5", "品質区分が不良品（2）の明細は、戻し先倉庫が不良品倉庫フラグ 1 の倉庫で"
                 "あることを確認する",
                 "M_WAREHOUSE", "—", "不良品倉庫でない場合はエラーを返す"],
                ["6", "明細金額（返品数量 × 元の受注明細の販売単価、円未満切り捨て）を計算し、"
                 "返品金額合計を求める",
                 "T_ORDER_DETAIL", "—", "—"],
                ["7", "消費税額を消費税計算部品で計算する。★明細単位に計算し円未満は切り捨てる",
                 "M_PRODUCT", "—", "—"],
                ["8", f"返品番号を採番する（{sc.RETURN_NO_FORMAT}）",
                 "—", "T_NUMBERING", "採番テーブルのロック待ちが 30 秒を超えた場合はエラーとする"],
                ["9", "返品ヘッダと返品明細を登録する。返品ステータスは 10（受付）とする",
                 "—", "T_RETURN, T_RETURN_DETAIL", "一意制約違反時はロールバックして排他エラーを返す"],
                ["10", "受注明細の返品済数量に今回の返品数量を加算する",
                 "—", "T_ORDER_DETAIL", "更新日時が一致しない場合は排他エラーを返す"],
                ["11", "監査ログを出力する（操作種別＝返品受付、変更前＝なし、変更後＝登録内容）",
                 "—", "T_AUDIT_LOG", "監査ログの出力に失敗した場合は本処理を異常終了させる"],
                ["12", "返品番号を画面へ返す。不良品を含む場合は警告メッセージを併せて返す",
                 "—", "—", "—"],
            ],
            widths=[6, 86, 26, 26, 46],
            center_cols=(0,),
        )
        xk.set_print(ws4, doc_name=info.doc_name)

        ws5 = xk.add_sheet(wb, "5.入力チェック")
        r = xk.heading(ws5, 2, 2, "5. 入力チェック一覧")
        xk.table(
            ws5,
            top=r,
            left=2,
            caption="入力チェック一覧",
            groups=[("", 1), ("", 1), ("", 1), ("", 1), ("出力するメッセージ", 1)],
            header=["No", "対象項目", "チェック種別", "チェック内容", "メッセージ"],
            rows=[
                ["1", "受注番号", "必須", "未入力でないこと", "{0}を入力してください。"],
                ["2", "受注番号", "存在", "受注ヘッダに存在すること", "{0}を入力してください。"],
                ["3", "受注番号", "業務", "受注ステータスが出荷指示済（40）であること",
                 "出荷指示前の受注は返品できません。受注取消で処理してください。"],
                ["4", "返品日", "必須", "未入力でないこと", "{0}を入力してください。"],
                ["5", "返品日", "業務", "納品日から返品受付日数（既定 14 日）以内であること",
                 "返品可能期限（納品日から{0}日）を過ぎているため返品できません。"],
                ["6", "返品理由区分", "必須", "選択されていること", "{0}を入力してください。"],
                ["7", "返品明細", "範囲", "1 行以上選択されていること", "{0}を入力してください。"],
                ["8", "返品明細", "範囲", "1 行以上 100 行以内であること", "{0}は{1}以下で入力してください。"],
                ["9", "返品数量", "必須", "選択した明細行ごとに未入力でないこと", "{0}を入力してください。"],
                ["10", "返品数量", "範囲", "0 より大きい数値であること", "受注数量は0より大きい値を入力してください。"],
                ["11", "返品数量", "書式", "整数部 7 桁・小数部 2 桁以内であること", "{0}は{1}以下で入力してください。"],
                ["12", "返品数量", "業務", "出荷数量から返品済数量を引いた数量以内であること",
                 "返品数量が返品可能数を超えています。（商品:{0} 返品可能数:{1}）"],
                ["13", "品質区分", "必須", "選択されていること", "{0}を入力してください。"],
                ["14", "戻し先倉庫", "業務", "品質区分が不良品のときは不良品倉庫であること",
                 "{0}を入力してください。"],
            ],
            widths=[6, 24, 16, 72, 40],
            center_cols=(0, 2),
        )
        xk.set_print(ws5, doc_name=info.doc_name)

        ws6 = xk.add_sheet(wb, "6.異常系の扱い")
        r = xk.heading(ws6, 2, 2, "6. 異常系の扱い")
        xk.table(
            ws6,
            top=r,
            left=2,
            caption="異常系の扱い",
            groups=[("", 1), ("", 1), ("", 1), ("出力するメッセージ", 1)],
            header=["No", "発生条件", "システムの動作", "メッセージ"],
            rows=[
                ["1", "採番テーブルのロックが 30 秒以上取得できない",
                 "処理を中断しロールバックする。システムエラー画面へ遷移させる", "（システムエラー画面）"],
                ["2", "受注明細の更新日時が画面の保持する値と一致しない",
                 "ロールバックし、再読み込みを促す", "他の利用者が更新しました。再度読み込んでください。"],
                ["3", "同じ受注明細に対する返品が同時に登録された",
                 "後勝ちにせず、返品済数量の更新で排他エラーとして中断する",
                 "他の利用者が更新しました。再度読み込んでください。"],
                ["4", "返品ヘッダの登録で一意制約に違反した",
                 "ロールバックのうえ再採番して 1 回だけ再試行する。再度失敗した場合はエラーとする",
                 "他の利用者が更新しました。再度読み込んでください。"],
                ["5", "監査ログの出力に失敗した",
                 "トランザクション全体をロールバックし異常終了させる", "（システムエラー画面）"],
                ["6", "データベース接続が切断された",
                 "システムエラー画面へ遷移させ、エラーIDを表示する", "（システムエラー画面）"],
            ],
            widths=[6, 50, 62, 40],
            center_cols=(0,),
            header_fill=xk.FILL_HEAD2,
        )
        xk.set_print(ws6, doc_name=info.doc_name)

    diagrams = [
        Diagram(
            sheet="3.処理フロー",
            nodes=[
                Node("start", "開始", 6, 8, 8, 2, shape="ellipse", fill="FFF2CC", line="BF8F00"),
                Node("valid", "入力値検証\nvalidateReturn", 6, 12, 16, 3),
                Node("j1", "検証OK？", 6, 17, 12, 3, shape="diamond", fill="FCE4D6", line="C55A11"),
                Node("err", "エラー返却\n（画面へメッセージ）", 26, 17, 16, 3,
                     fill="F8CBAD", line="C00000"),
                Node("j2", "返品期限内？", 6, 22, 14, 3, shape="diamond",
                     fill="FCE4D6", line="C55A11"),
                Node("over", "期限超過エラー\n（返品不可）", 26, 22, 16, 3,
                     fill="F8CBAD", line="C00000"),
                Node("j3", "返品可能数以内？", 6, 27, 16, 3, shape="diamond",
                     fill="FCE4D6", line="C55A11"),
                Node("qty", "数量超過エラー\n（返品可能数を表示）", 28, 27, 18, 3,
                     fill="F8CBAD", line="C00000"),
                Node("calc", "金額・消費税計算\ncalcReturnAmount", 6, 32, 18, 3),
                Node("no", "返品番号採番\nNumberingService", 6, 37, 16, 3,
                     fill="EDEDED", line="808080"),
                Node("insert", "返品ヘッダ・明細登録\nT_RETURN / T_RETURN_DETAIL", 6, 42, 20, 3),
                Node("upd", "返品済数量を加算\nT_ORDER_DETAIL", 6, 47, 18, 3),
                Node("audit", "監査ログ出力\nAuditLogger", 6, 52, 16, 3,
                     fill="EDEDED", line="808080"),
                Node("end", "終了", 6, 57, 8, 2, shape="ellipse", fill="FFF2CC", line="BF8F00"),
            ],
            edges=[
                Edge("start", "valid"),
                Edge("valid", "j1"),
                Edge("j1", "err", "NG"),
                Edge("j1", "j2", "OK"),
                Edge("j2", "over", "超過"),
                Edge("j2", "j3", "以内"),
                Edge("j3", "qty", "超過"),
                Edge("j3", "calc", "以内"),
                Edge("calc", "no"),
                Edge("no", "insert"),
                Edge("insert", "upd"),
                Edge("upd", "audit"),
                Edge("audit", "end"),
            ],
        )
    ]
    return xk.build(
        out, "10_受注管理/03_詳細設計/処理仕様書_返品受付.xlsx", info, body, diagrams
    )
