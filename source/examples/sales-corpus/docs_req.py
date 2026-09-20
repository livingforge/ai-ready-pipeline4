"""01_要件定義（5 本）。工程レイヤ 1。

上流の記述はわざと粗い。同じ事柄を下流（基本設計・詳細設計）が細かく
書き直すので、整理層が粒度差（refines の親子）を見分けられるかの材料になる。
サブシステム別の「業務要件一覧」は要件定義書と**同じ内容を別の言い方で**
書いている（重複ファクトの検体）。表記も揃えていない（取引先 / オーダー / 品目）。

要件に ID は振らない。文書間の対応は「関連する要件」列に**要件名**を書くだけで、
名前で引くしかない ——現場の Excel と同じ条件にしてある。

図表: 業務フロー図（要件定義書）とサブシステム別の業務フロー図 3 点。
"""

from __future__ import annotations

from pathlib import Path

import spec
import xlsxkit as xk
from xlsxkit import Diagram, DocInfo, Edge, Node


def build(out: Path) -> list[Path]:
    return [
        _req_spec(out),
        _biz_req_ord(out),
        _biz_req_inv(out),
        _biz_req_bil(out),
        _nonfunc(out),
    ]


# ── 要件定義書（全体）─────────────────────────────────────────────
def _req_spec(out: Path) -> Path:
    info = DocInfo(
        doc_name="要件定義書",
        subsystem="全体",
        version="1.1",
        date="2025/05/30",
        revisions=[
            ("1.0", "2025/05/23", "初版作成", "山田"),
            ("1.1", "2025/05/30", "レビュー指摘（受注取消の期限、締め日、EDI 範囲）を反映", "山田"),
        ],
    )

    def body(wb, info):
        # 1.1 背景と目的
        ws = xk.add_sheet(wb, "1.1 背景と目的")
        r = xk.heading(ws, 2, 2, "1.1 システム化の背景と目的")
        r = xk.kv_group_table(
            ws,
            top=r,
            left=2,
            groups=[
                ("背景", [
                    ("設備", "現行の販売管理システムは 2009 年に稼働し、ハードウェアの保守期限が"
                            "2026 年 3 月に到来する。"),
                    ("業務", "EDI は旧 JCA 手順のままで、主要な取引先が求める流通BMS に対応できていない。"
                            "また月次の請求締めに 5 営業日を要し、経理部の負荷が高い。"),
                ]),
                ("目的", [
                    ("刷新の目的", "受注・在庫・請求の各業務を刷新し、流通BMS 対応と締め処理の早期化を実現する。"),
                    ("システム化の範囲", "受注管理・在庫管理・請求管理・共通基盤の 4 サブシステムとする。"
                                    "購買業務および仕入業務は本プロジェクトの対象外とする。"),
                ]),
                ("利用者", [
                    ("営業部", "120 名。受注の登録・変更・取消と受注状況の照会を行う。"),
                    ("物流部", "60 名。入出庫・棚卸・在庫調整と出荷指示を行う。"),
                    ("経理部", "20 名。請求締め・請求書発行・入金消込を行う。"),
                ]),
            ],
            width=88,
        )
        xk.set_print(ws, doc_name=info.doc_name)

        ws1b = xk.add_sheet(wb, "1.2 サブシステム構成")
        r = xk.heading(ws1b, 2, 2, "1.2 サブシステムの構成")
        xk.table(
            ws1b,
            top=r,
            left=2,
            caption="サブシステムの構成",
            header=["サブシステム", "担う業務"],
            rows=[[n, d] for n, d in spec.SUBSYSTEMS],
            widths=[18, 92],
        )
        xk.set_print(ws1b, doc_name=info.doc_name)

        ws1c = xk.add_sheet(wb, "1.3 現行との対比")
        r = xk.heading(ws1c, 2, 2, "1.3 現行システムとの対比")
        xk.table(
            ws1c,
            top=r,
            left=2,
            caption="現行システムと新システムの対比",
            groups=[("", 1), ("現行", 2), ("新システム", 2)],
            header=["観点", "現行の方式", "現行の課題", "新システムの方式", "期待する効果"],
            rows=[
                ["受注の受付", "画面入力とFAXの代行入力", "FAX の手入力に 1 日 3 時間かかる",
                 "流通BMS による EDI 受注取込", "手入力の対象を 6 割減らす"],
                ["在庫の引当", "夜間バッチで一括", "日中に在庫の状況が分からない",
                 "受注の登録と連動して即時に引当", "欠品の把握を当日中に行える"],
                ["与信の確認", "月次の帳票を目視", "限度額の超過に気づくのが遅い",
                 "受注ごとに自動で照合し保留にする", "与信超過の受注を未然に止める"],
                ["請求の締め", "手作業の集計を含むバッチ", "確定に 5 営業日かかる",
                 "得意先単位に並列化したバッチ", "2 営業日で請求書を発送する"],
                ["入金の消込", "全件を手作業で突合", "月末に経理部が残業する",
                 "請求番号の一致による自動消込", "手作業を突合できない分に限定する"],
                ["会計への連携", "月次でファイル授受", "月中の残高が会計側に反映されない",
                 "日次で仕訳データを連携", "月中の残高を会計側で把握できる"],
            ],
            widths=[14, 26, 34, 30, 32],
            center_cols=(0,),
        )
        xk.set_print(ws1c, doc_name=info.doc_name)

        # 2. 業務フロー（図形）
        ws2 = xk.add_sheet(wb, "2.業務フロー", grid=True)
        c = ws2.cell(row=2, column=2, value="2. 受注から入金までの業務の流れ")
        c.font = xk.F_SECTION
        for i, text in enumerate((
            "凡例",
            "楕円: 社外の相手",
            "菱形: 判定",
            "網掛け: 本システムの対象外",
        )):
            cell = ws2.cell(row=8 + i, column=68, value=text)
            cell.font = xk.F_BODY

        # 3. 機能要件
        ws3 = xk.add_sheet(wb, "3.機能要件一覧")
        r = xk.heading(ws3, 2, 2, "3. 機能要件一覧")
        r = xk.table(
            ws3,
            top=r,
            left=2,
            caption="機能要件一覧",
            groups=[("区分", 3), ("", 1)],
            header=["サブシステム", "分類", "要件名", "要件内容"],
            rows=[
                [spec.SUBSYSTEM_NAME[sub], cat, name, text]
                for name, sub, cat, text in spec.FUNC_REQS
            ],
            widths=[14, 14, 24, 82],
            merge_cols=(0, 1),
            center_cols=(0, 1),
        )
        xk.note(ws3, r, 2, "※ 要件の実現方式は基本設計書にて定義する。")
        xk.set_print(ws3, doc_name=info.doc_name)

        # 4. 非機能要件（概要のみ。詳細は非機能要件一覧に持つ＝重複の検体）
        ws4 = xk.add_sheet(wb, "4.非機能要件")
        r = xk.heading(ws4, 2, 2, "4. 非機能要件（概要）")
        r = xk.table(
            ws4,
            top=r,
            left=2,
            caption="非機能要件（主要なもの）",
            header=["分類", "要件名", "要件内容"],
            rows=[[cat, name, text] for name, cat, text in spec.NONFUNC_REQS[:8]],
            widths=[16, 24, 90],
            merge_cols=(0,),
            center_cols=(0,),
        )
        xk.note(ws4, r, 2, "※ 全項目は別紙「非機能要件一覧」を参照のこと。")
        xk.set_print(ws4, doc_name=info.doc_name)

        # 5. 制約・前提
        ws5 = xk.add_sheet(wb, "5.制約・前提条件")
        r = xk.heading(ws5, 2, 2, "5. 制約条件・前提条件")
        xk.table(
            ws5,
            top=r,
            left=2,
            caption="前提条件・制約条件",
            header=["区分", "内容"],
            rows=[[name, text] for name, text in spec.CONSTRAINTS],
            widths=[16, 96],
        )
        xk.set_print(ws5, doc_name=info.doc_name)

        # 6. 用語集
        ws6 = xk.add_sheet(wb, "6.用語集")
        r = xk.heading(ws6, 2, 2, "6. 用語集")
        xk.table(
            ws6,
            top=r,
            left=2,
            caption="用語集",
            header=["用語", "説明"],
            rows=[[term, desc] for term, desc in spec.GLOSSARY],
            widths=[20, 95],
        )
        xk.set_print(ws6, doc_name=info.doc_name)

    diagrams = [
        Diagram(
            sheet="2.業務フロー",
            nodes=[
                Node("cust", "得意先", 4, 7, 10, 3, shape="ellipse", fill="FFF2CC", line="BF8F00"),
                Node("order", "受注受付\n（画面入力 / EDI）", 18, 7, 14, 3),
                Node("credit", "与信確認", 36, 7, 12, 3, shape="diamond", fill="FCE4D6", line="C55A11"),
                Node("alloc", "在庫引当", 52, 7, 12, 3),
                Node("ship_i", "出荷指示", 52, 14, 12, 3),
                Node("wms", "倉庫作業\n（WMS）", 52, 21, 12, 3, fill="EDEDED", line="808080"),
                Node("ship_r", "出荷実績受信\n在庫引落・売上計上", 34, 21, 15, 3),
                Node("close", "請求締め", 18, 21, 12, 3),
                Node("invoice", "請求書発行", 4, 21, 12, 3),
                Node("deposit", "入金消込", 4, 28, 12, 3),
                Node("acct", "会計システム\n（仕訳連携）", 20, 28, 14, 3, fill="EDEDED", line="808080"),
                # 銀行は入金消込の**真下**に置く（右に並べると、銀行から入金消込へ
                # 戻る線が会計システムの箱を突っ切り、絵が「入金消込 → 会計システム
                # → 銀行」の直列に見える）。
                Node("bank", "銀行\n（入金データ）", 3, 35, 14, 3, shape="ellipse",
                     fill="FFF2CC", line="BF8F00"),
                Node("hold", "与信保留\n（営業部長の承認待ち）", 34, 2, 16, 3,
                     fill="F8CBAD", line="C00000"),
            ],
            edges=[
                Edge("cust", "order", "発注"),
                Edge("order", "credit", "与信確認"),
                Edge("credit", "alloc", "限度額内"),
                Edge("credit", "hold", "限度額超過"),
                Edge("alloc", "ship_i", "引当済"),
                Edge("ship_i", "wms", "出荷指示"),
                Edge("wms", "ship_r", "出荷実績"),
                Edge("ship_r", "close", "売上"),
                Edge("close", "invoice", "請求確定"),
                Edge("invoice", "deposit", "請求送付"),
                Edge("bank", "deposit", "入金明細"),
                Edge("deposit", "acct", "入金仕訳"),
            ],
        )
    ]
    return xk.build(out, "00_全体/01_要件定義/新販売管理システム_要件定義書.xlsx", info, body, diagrams)


# ── サブシステム別 業務要件一覧 ────────────────────────────────────
def _biz_req(out: Path, sub: str, rows: list[list[str]],
             current_issues: list[list[str]], diagram: Diagram) -> Path:
    name = spec.SUBSYSTEM_NAME[sub]
    info = DocInfo(
        doc_name=f"業務要件一覧（{name}）",
        subsystem=name,
        version="1.0",
        date="2025/05/30",
        revisions=[("1.0", "2025/05/30", "初版作成", "鈴木")],
    )

    def body(wb, info):
        ws = xk.add_sheet(wb, "現行業務の課題")
        r = xk.heading(ws, 2, 2, f"1. 現行業務の課題（{name}）")
        xk.table(
            ws,
            top=r,
            left=2,
            caption=f"現行業務の課題と解消方針（{name}）",
            groups=[("", 1), ("", 1), ("現行", 1), ("新システム", 1)],
            header=["No", "業務", "現行の課題", "新システムでの解消方針"],
            rows=current_issues,
            widths=[6, 20, 56, 56],
            center_cols=(0,),
            merge_cols=(1,),
        )
        xk.set_print(ws, doc_name=info.doc_name)

        ws_f = xk.add_sheet(wb, "業務フロー", grid=True)
        c = ws_f.cell(row=2, column=2, value=f"2. 業務フロー（{name}）")
        c.font = xk.F_SECTION
        for i, text in enumerate(("凡例", "菱形: 判定", "網掛け: 他サブシステム")):
            cell = ws_f.cell(row=6 + i, column=58, value=text)
            cell.font = xk.F_BODY

        ws2 = xk.add_sheet(wb, "業務要件一覧")
        r = xk.heading(ws2, 2, 2, f"3. 業務要件一覧（{name}）")
        r = xk.table(
            ws2,
            top=r,
            left=2,
            caption=f"業務要件一覧（{name}）",
            groups=[("", 1), ("", 1), ("", 1), ("優先度・対応", 2)],
            header=["No", "業務", "業務要件", "優先度", "関連する要件"],
            rows=rows,
            widths=[6, 20, 82, 10, 22],
            merge_cols=(1,),
            center_cols=(0, 3),
        )
        xk.note(ws2, r, 2, "優先度: A=必須、B=可能なら対応、C=次期フェーズ／"
                           "「関連する要件」は要件定義書 3.機能要件一覧の要件名を指す")
        xk.set_print(ws2, doc_name=info.doc_name)

    return xk.build(
        out, spec.path_of(sub, "要件定義", f"{name}_業務要件一覧.xlsx"), info, body, [diagram]
    )


def _biz_req_ord(out: Path) -> Path:
    # 表記ゆれ: 取引先 / オーダー / 品目。要件定義書と同じ事柄を別の言い方で書く。
    diagram = Diagram(
        sheet="業務フロー",
        nodes=[
            Node("fax", "FAX注文", 4, 6, 10, 3, shape="ellipse", fill="FFF2CC", line="BF8F00"),
            Node("edi", "EDI発注データ", 4, 11, 12, 3, shape="ellipse", fill="FFF2CC", line="BF8F00"),
            Node("entry", "オーダー入力\n（代行入力）", 20, 6, 14, 3),
            Node("import", "EDI受注取込", 20, 11, 14, 3),
            Node("j1", "与信の枠内？", 38, 8, 13, 3, shape="diamond", fill="FCE4D6", line="C55A11"),
            Node("hold", "保留（営業部長の承認）", 38, 2, 16, 3, fill="F8CBAD", line="C00000"),
            Node("alloc", "在庫の引き当て\n（在庫管理）", 55, 8, 14, 3, fill="EDEDED", line="808080"),
            Node("ship", "出荷の依頼", 55, 15, 12, 3),
            Node("print", "出荷指示書の印刷", 55, 21, 15, 3),
            Node("cancel", "オーダー取消", 20, 21, 13, 3),
        ],
        edges=[
            Edge("fax", "entry", "手入力"),
            Edge("edi", "import", "自動取込"),
            Edge("entry", "j1", "登録"),
            Edge("import", "j1", "登録"),
            Edge("j1", "hold", "超過"),
            Edge("j1", "alloc", "枠内"),
            Edge("alloc", "ship", "引当済"),
            Edge("ship", "print", "指示"),
            Edge("cancel", "alloc", "引当の解放"),
        ],
    )
    return _biz_req(
        out,
        "ORD",
        rows=[
            ["1", "受注受付", "営業担当が取引先・品目・数量・希望納期を入力し、オーダーを登録できること。", "A", "受注登録"],
            ["2", "受注受付", "登録したオーダーの数量および希望納期を、出荷の指示を出す前であれば修正できること。", "A", "受注内容の変更"],
            ["3", "受注受付", "登録したオーダーを取り消せること。取消は出荷指示を出すまでの間に限る。", "A", "受注の取消"],
            ["4", "受注受付", "取引先から流通BMS で送られた発注データを自動で取り込めること。", "A", "EDI受注の取込"],
            ["5", "受注受付", "取込に失敗したデータの内容とエラー理由を、オーダーNo を指定して画面で確認できること。", "A", "EDI受注の取込"],
            ["6", "与信管理", "オーダー登録時に取引先の売掛残高を確認し、与信の枠を超える場合は保留にできること。", "A", "与信チェック"],
            ["7", "与信管理", "保留になったオーダーを営業部長が承認して解除できること。", "B", "与信チェック"],
            ["8", "出荷", "在庫の確保が済んだオーダーについて、倉庫へ出荷の依頼を出せること。", "A", "出荷指示"],
            ["9", "出荷", "出荷依頼の内容を出荷指示書として印刷できること。", "A", "出荷指示"],
            ["10", "照会", "受注日・取引先・状態・オーダーNo を条件にオーダーを検索し、明細まで確認できること。", "A", "受注状況の照会"],
            ["11", "照会", "検索した結果を CSV に出力できること。", "B", "受注状況の照会"],
        ],
        current_issues=[
            ["1", "受注受付", "FAX 注文を事務が現行システムへ手入力しており、1 日あたり 3 時間を要している。",
             "流通BMS による EDI 受注を拡大し、手入力の対象を減らす。"],
            ["2", "受注受付", "オーダーの取消可否を担当者の記憶に頼っており、出荷済みの取消が発生している。",
             "システムで取消の可否を判定し、不可の場合はエラーとする。"],
            ["3", "与信管理", "与信の確認が月次の帳票のみで、超過に気づくのが遅い。",
             "受注の都度、売掛残高と与信枠を自動で照合する。"],
            ["4", "出荷", "出荷指示書を Excel で作成しており、内容の転記ミスが月に数件発生している。",
             "受注データから出荷指示書を直接出力する。"],
        ],
        diagram=diagram,
    )


def _biz_req_inv(out: Path) -> Path:
    # 表記ゆれ: 品目 / 製品 / 引き当て / 倉庫在庫。
    diagram = Diagram(
        sheet="業務フロー",
        nodes=[
            Node("order", "オーダー確定\n（受注管理）", 4, 6, 14, 3, fill="EDEDED", line="808080"),
            Node("alloc", "在庫の引き当て", 22, 6, 14, 3),
            Node("j1", "在庫は足りるか", 40, 6, 14, 3, shape="diamond", fill="FCE4D6", line="C55A11"),
            Node("part", "一部引き当て\n不足数を通知", 40, 1, 14, 3, fill="F8CBAD", line="C00000"),
            Node("ship", "出荷の実績受信", 58, 6, 14, 3),
            Node("issue", "出庫（在庫の減算）", 58, 12, 15, 3),
            Node("recv", "入庫登録\n（仕入・返品）", 4, 14, 14, 3),
            Node("count", "棚卸の入力", 4, 20, 12, 3),
            Node("diff", "差異の確定\n（在庫調整）", 22, 20, 14, 3),
            Node("hist", "在庫移動履歴", 40, 17, 14, 3),
            Node("view", "在庫照会", 58, 20, 12, 3),
        ],
        edges=[
            Edge("order", "alloc", "引当依頼"),
            Edge("alloc", "j1", "在庫確認"),
            Edge("j1", "part", "不足"),
            Edge("j1", "ship", "充足"),
            Edge("ship", "issue", "出荷確定"),
            Edge("recv", "hist", "増加"),
            Edge("issue", "hist", "減少"),
            Edge("count", "diff", "差異"),
            Edge("diff", "hist", "調整"),
            Edge("hist", "view", "現在数量"),
        ],
    )
    return _biz_req(
        out,
        "INV",
        rows=[
            ["1", "在庫引当", "オーダーの明細ごとに、倉庫在庫から必要な数量を引き当てできること。", "A", "在庫引当"],
            ["2", "在庫引当", "在庫が足りない場合は、引き当てできた分だけを確保し、不足数を担当者へ通知できること。", "A", "在庫引当"],
            ["3", "在庫引当", "引き当ては入庫の古いものから順に行うこと。", "B", "在庫引当"],
            ["4", "入出庫", "仕入および返品による入庫を登録し、在庫を増やせること。", "A", "入庫登録"],
            ["5", "入出庫", "倉庫から受け取った出荷の実績にもとづき、在庫を減らせること。", "A", "出庫処理"],
            ["6", "棚卸", "実地棚卸の数量を入力し、帳簿の在庫との差を一覧できること。", "A", "棚卸"],
            ["7", "棚卸", "棚卸の差を在庫の調整として確定し、履歴に残せること。", "A", "棚卸"],
            ["8", "照会", "倉庫と製品を指定して、実在庫・引き当て済・引き当て可能な数量を照会できること。", "A", "在庫照会"],
            ["9", "在庫調整", "破損・廃棄などの理由を選んで在庫を補正できること。補正には理由の入力を必須とする。", "A", "在庫調整"],
            ["10", "照会", "安全在庫を下回った品目を一覧で確認できること。", "C", "在庫照会"],
        ],
        current_issues=[
            ["1", "在庫引当", "引き当てが夜間バッチのため、日中に在庫の状況が分からない。",
             "受注の登録と同時に引き当てを行い、在庫を即時に反映する。"],
            ["2", "棚卸", "棚卸の集計を Excel で行っており、確定までに 3 日かかる。",
             "棚卸の入力から差異の確定までをシステム上で完結させる。"],
            ["3", "照会", "引き当て済の数量が画面に出ないため、受注できる数量が分からない。",
             "実在庫・引き当て済・引き当て可能な数量を同じ画面に表示する。"],
        ],
        diagram=diagram,
    )


def _biz_req_bil(out: Path) -> Path:
    # 表記ゆれ: 顧客 / 締め処理 / 売価。
    diagram = Diagram(
        sheet="業務フロー",
        nodes=[
            Node("sales", "売上の計上\n（出荷実績）", 4, 6, 14, 3, fill="EDEDED", line="808080"),
            Node("j1", "締め日か", 22, 6, 12, 3, shape="diamond", fill="FCE4D6", line="C55A11"),
            Node("close", "締め処理\n（売上の集計）", 38, 6, 14, 3),
            Node("tax", "消費税の計算", 56, 6, 13, 3),
            Node("fix", "請求金額の確定", 56, 12, 14, 3),
            Node("issue", "請求書の発行", 38, 12, 13, 3),
            Node("send", "顧客へ送付", 20, 12, 12, 3, shape="ellipse", fill="FFF2CC", line="BF8F00"),
            Node("bank", "入金データ受信\n（全銀）", 4, 18, 15, 3),
            Node("match", "入金の消込", 24, 18, 12, 3),
            Node("j2", "請求と一致するか", 40, 18, 15, 3, shape="diamond", fill="FCE4D6", line="C55A11"),
            Node("hold", "保留（担当者が選択）", 40, 24, 16, 3, fill="F8CBAD", line="C00000"),
            Node("balance", "売掛残高の更新", 60, 18, 14, 3),
            Node("acct", "会計システムへ連携", 60, 24, 16, 3, fill="EDEDED", line="808080"),
        ],
        edges=[
            Edge("sales", "j1", "日次"),
            Edge("j1", "close", "締め日"),
            Edge("close", "tax", "集計結果"),
            Edge("tax", "fix", "税額"),
            Edge("fix", "issue", "確定"),
            Edge("issue", "send", "請求書"),
            Edge("bank", "match", "入金明細"),
            Edge("match", "j2", "突合"),
            Edge("j2", "hold", "不一致"),
            Edge("j2", "balance", "一致"),
            Edge("balance", "acct", "仕訳"),
        ],
    )
    return _biz_req(
        out,
        "BIL",
        rows=[
            ["1", "締め処理", "顧客ごとに締め期間の売上を集計し、請求の金額を確定できること。売価は受注時の値を用いる。", "A", "請求締め"],
            ["2", "締め処理", "顧客の締め日は 20 日と末日の 2 種類を扱えること。", "A", "請求締め"],
            ["3", "締め処理", "確定した請求を取り消して再度締め直せること。", "B", "請求締め"],
            ["4", "請求書", "確定した請求から請求書を PDF で出力できること。", "A", "請求書発行"],
            ["5", "請求書", "請求書を顧客ごとにメールまたは郵送で送付できること。", "B", "請求書発行"],
            ["6", "入金", "銀行から受け取った入金の明細を取り込めること。", "A", "入金消込"],
            ["7", "入金", "入金と請求を突き合わせ、自動で消し込めること。突合できないものは候補を出して担当者が選ぶこと。", "A", "入金消込"],
            ["8", "売掛管理", "顧客ごとの売掛の残高と、支払期日を過ぎた滞留の状況を照会できること。", "A", "売掛残高管理"],
            ["9", "会計連携", "確定した売上を仕訳のデータに変換し、会計システムへ日次で渡せること。", "A", "会計システム連携"],
            ["10", "売掛管理", "滞留が一定の期間を超えた顧客を営業担当へ通知できること。", "C", "売掛残高管理"],
        ],
        current_issues=[
            ["1", "締め処理", "締めの集計に 5 営業日かかり、請求書の発送が翌月 10 日ごろになる。",
             "締めをバッチで自動化し、2 営業日で請求書を発送できるようにする。"],
            ["2", "入金", "入金の消込を手作業で行っており、月末に経理部が残業している。",
             "請求番号の一致による自動消込を行い、手作業を突合できない分のみに限定する。"],
            ["3", "会計連携", "仕訳データを月次でしか渡せておらず、月中の残高が会計側に反映されない。",
             "日次で仕訳データを連携する。"],
        ],
        diagram=diagram,
    )


# ── 非機能要件一覧 ────────────────────────────────────────────────
def _nonfunc(out: Path) -> Path:
    info = DocInfo(
        doc_name="非機能要件一覧",
        subsystem="全体",
        version="1.0",
        date="2025/05/30",
        revisions=[("1.0", "2025/05/30", "初版作成", "高橋")],
    )

    # 分類ごとの測定・確認方法（要件定義書側には無い情報＝下流で足された記述）
    verify = {
        "性能": ("性能テストにて測定する", "受入テスト"),
        "可用性": ("設計レビューおよび運用テストで確認する", "総合テスト"),
        "運用": ("運用テストで確認する", "総合テスト"),
        "セキュリティ": ("セキュリティ診断および設計レビューで確認する", "総合テスト"),
        "ユーザビリティ": ("受入テストで確認する", "受入テスト"),
        "保守性": ("設計レビューで確認する", "詳細設計"),
        "移行": ("移行リハーサルで確認する", "移行"),
    }

    def body(wb, info):
        ws = xk.add_sheet(wb, "非機能要件一覧")
        r = xk.heading(ws, 2, 2, "非機能要件一覧")
        r = xk.table(
            ws,
            top=r,
            left=2,
            groups=[("", 1), ("", 1), ("", 1), ("検証", 2)],
            header=["分類", "要件名", "要件内容", "確認方法", "確認する工程"],
            rows=[
                [cat, name, text, *verify.get(cat, ("設計レビューで確認する", "基本設計"))]
                for name, cat, text in spec.NONFUNC_REQS
            ],
            widths=[16, 24, 70, 30, 16],
            merge_cols=(0,),
            center_cols=(0, 4),
        )
        # 非機能要件一覧と同じ列幅（B/C/D）を使うので同じシートに積める。
        r = xk.heading(ws, r, 2, "システム規模の想定")
        xk.table(
            ws,
            top=r,
            left=2,
            groups=[("", 1), ("想定値", 2)],
            header=["項目", "値", "備考"],
            rows=[
                ["登録利用者数", "500 名", "営業 120・物流 60・経理 20・その他"],
                ["同時接続数", "最大 300 名", "月末の締め期間に集中する"],
                ["得意先件数", "3,000 件", "うち EDI 対応は 400 件"],
                ["商品件数", "20,000 件", "うち稼働品目は 8,000 件程度"],
                ["受注件数", "1,200,000 件/年", "1 日あたり約 5,000 件（ピーク 12,000 件）"],
                ["受注明細件数", "4,800,000 件/年", "1 受注あたり平均 4 明細"],
                ["データ保存期間", "7 年", "法定保存期間に合わせる"],
            ],
            widths=[16, 24, 70],
            center_cols=(1,),
        )
        xk.set_print(ws, doc_name=info.doc_name)

    return xk.build(out, "00_全体/01_要件定義/非機能要件一覧.xlsx", info, body)
