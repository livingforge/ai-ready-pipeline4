"""02_基本設計のうち、方式・一覧・データ・連携（8 本）。

工程レイヤ 2。要件定義の記述をここで**構造**（画面・エンティティ・データ項目・
外部インターフェース）に落とす。業務ルールは「業務判断の層」までをここに置き、
実装層（検証・端数処理・異常系）は詳細設計へ回す —— jp-sier-std の
``business-rule.rule_kind`` による基本設計書／詳細設計書の振り分けに対応する。

機能・画面・帳票・外部インターフェースに ID は振らない。対応関係は
「対応する要件」「関連する画面・帳票」列に**名前**を書く。

図表: システム構成図・受注ステータスの状態遷移図・ER図・画面遷移図・
ジョブフロー図・外部連携図（いずれも Excel 図形）。

画面仕様書・帳票は docs_screen.py にある。
"""

from __future__ import annotations

from pathlib import Path

import spec
import xlsxkit as xk
from xlsxkit import Diagram, DocInfo, Edge, Node

REV = [("1.0", "2025/08/08", "初版作成", "山田"), ("1.1", "2025/08/29", "レビュー指摘を反映", "山田")]

# (機能名, サブシステム, 種別, 概要, 対応する要件, 関連する画面・帳票)
FUNCTIONS = [
    ("受注登録", "ORD", "オンライン", "受注を新規に登録し、与信確認と在庫引当を行う",
     "受注登録 / 与信チェック / 在庫引当", "受注入力"),
    ("受注変更", "ORD", "オンライン", "出荷指示前の受注の数量・納品希望日を変更する",
     "受注内容の変更", "受注入力"),
    ("受注取消", "ORD", "オンライン", "受注を取り消し、引当済の在庫を解放する",
     "受注の取消", "受注取消"),
    ("受注照会", "ORD", "オンライン", "条件を指定して受注を検索し、明細を表示する",
     "受注状況の照会", "受注一覧照会"),
    ("出荷指示", "ORD", "オンライン", "引当済の受注から出荷指示を作成し WMS へ連携する",
     "出荷指示", "出荷指示 / 出荷指示書"),
    ("EDI受注取込", "ORD", "バッチ", "EDI で受信した発注データを受注として登録する",
     "EDI受注の取込", "EDI受注取込結果照会"),
    ("注文請書出力", "ORD", "オンライン", "確定した受注の内容を注文請書として出力する",
     "受注登録", "注文請書"),
    ("在庫引当", "INV", "オンライン", "受注明細に対して有効在庫を確保する",
     "在庫引当", "—"),
    ("入庫登録", "INV", "オンライン", "仕入・返品による入庫を登録し実在庫を増やす",
     "入庫登録", "入庫登録"),
    ("出荷実績取込", "INV", "バッチ", "WMS の出荷実績で在庫を引き落とし売上を計上する",
     "出庫処理", "—"),
    ("棚卸", "INV", "オンライン", "実地棚卸を入力し帳簿在庫との差異を確定する",
     "棚卸", "棚卸入力 / 在庫棚卸表"),
    ("在庫調整", "INV", "オンライン", "理由を付けて在庫数を補正する",
     "在庫調整", "在庫調整"),
    ("在庫照会", "INV", "オンライン", "倉庫・商品ごとの在庫数を照会する",
     "在庫照会", "在庫照会"),
    ("請求締め", "BIL", "バッチ", "締め対象の得意先の売上を集計し請求を確定する",
     "請求締め", "請求締め処理"),
    ("請求書発行", "BIL", "オンライン", "確定した請求から請求書 PDF を出力する",
     "請求書発行", "請求書発行 / 請求書"),
    ("入金データ取込", "BIL", "バッチ", "全銀フォーマットの入金明細を取り込む",
     "入金消込", "—"),
    ("入金消込", "BIL", "オンライン", "入金と請求を突き合わせて消し込む",
     "入金消込", "入金消込"),
    ("売掛残高照会", "BIL", "オンライン", "得意先ごとの売掛残高と滞留状況を照会する",
     "売掛残高管理", "売掛残高照会 / 売掛残高一覧表"),
    ("売上仕訳連携", "BIL", "バッチ", "確定した売上を仕訳に変換し会計システムへ送る",
     "会計システム連携", "—"),
    ("ログイン・ログアウト", "CMN", "オンライン", "社員コードとパスワードで認証しセッションを発行する",
     "利用者認証", "ログイン"),
    ("得意先マスタ保守", "CMN", "オンライン", "得意先の登録・変更・論理削除を行う",
     "マスタ保守", "得意先マスタ保守"),
    ("商品マスタ保守", "CMN", "オンライン", "商品の登録・変更・論理削除を行う",
     "マスタ保守", "商品マスタ保守"),
    ("監査ログ出力", "CMN", "共通部品", "更新操作の内容を監査ログへ記録する",
     "監査ログ", "—"),
]

# (ジョブ名, サブシステム, 起動契機, 起動時刻, 先行ジョブ, 処理概要)
JOBS = [
    ("EDI受注取込", "受注管理", "時刻起動", "07:30/10:30/13:30/15:30/17:30/20:30", "—",
     "EDI受信ワークのデータを受注として登録する"),
    ("出荷実績取込", "在庫管理", "時刻起動", "毎時 05 分", "—",
     "WMS から受信した出荷実績で在庫を引き落とし売上を計上する"),
    ("入金データ取込", "請求管理", "時刻起動", "09:00", "—",
     "全銀フォーマットの入金明細を取り込む"),
    ("請求締め", "請求管理", "時刻起動", "21:00", "出荷実績取込",
     "締め対象の得意先の売上を集計し請求を確定する"),
    ("売上仕訳連携", "請求管理", "先行ジョブ完了", "—", "請求締め",
     "確定した売上を仕訳に変換し会計システムへ送信する"),
    ("マスタ連携取込", "共通基盤", "時刻起動", "06:00", "—",
     "人事システムから社員マスタの差分を取り込む"),
    ("監査ログ退避", "共通基盤", "時刻起動", "02:00", "—",
     "13 か月を超えた監査ログをアーカイブ領域へ退避する"),
    ("データベースバックアップ", "共通基盤", "時刻起動", "01:00", "—",
     "full backup を取得し 7 世代を保管する"),
]


def build(out: Path) -> list[Path]:
    return [
        _system_design(out),
        _function_list(out),
        _screen_list(out),
        _table_def(out),
        _data_item_def(out),
        _code_def(out),
        _interface_spec(out),
        _batch_list(out),
    ]


# ── 基本設計書（システム方式）──────────────────────────────────────
def _system_design(out: Path) -> Path:
    info = DocInfo(
        doc_name="基本設計書（システム方式）",
        subsystem="全体",
        version="1.1",
        date="2025/08/29",
        revisions=REV,
    )

    def body(wb, info):
        # 1. システム構成図（図形）
        ws = xk.add_sheet(wb, "1.システム構成", grid=True)
        c = ws.cell(row=2, column=2, value="1. システム構成図")
        c.font = xk.F_SECTION
        for i, text in enumerate((
            "凡例",
            "実線: 同期（HTTPS / JDBC）",
            "破線: 非同期（SFTP / ファイル連携）",
            "網掛け: 本システムの対象外",
        )):
            cell = ws.cell(row=5 + i, column=62, value=text)
            cell.font = xk.F_BODY

        # 2.1 ソフトウェア構成
        ws2 = xk.add_sheet(wb, "2.1 ソフトウェア構成")
        r = xk.heading(ws2, 2, 2, "2.1 ソフトウェア構成")
        r = xk.table(
            ws2,
            top=r,
            left=2,
            caption="ソフトウェア構成",
            groups=[("", 1), ("採用製品", 2), ("", 1)],
            header=["層", "製品・技術", "版数", "用途"],
            rows=[
                ["クライアント", "Microsoft Edge / Google Chrome", "最新版", "画面の表示と操作"],
                ["Web", "nginx", "1.24", "静的コンテンツ配信・リバースプロキシ"],
                ["AP", "Java (OpenJDK)", "21 LTS", "業務ロジックの実行"],
                ["AP", "Spring Boot", "3.2", "アプリケーションフレームワーク"],
                ["AP", "MyBatis", "3.5", "O/R マッピング"],
                ["バッチ", "Spring Batch", "5.1", "夜間バッチの実行基盤"],
                ["DB", "PostgreSQL", "16", "業務データの永続化"],
                ["帳票", "SVF (Super Visual Formade)", "9.2", "PDF 帳票の生成"],
                ["ジョブ管理", "JP1/AJS3", "12", "バッチのスケジュール実行"],
                ["監視", "Zabbix", "6.4", "サーバ・プロセスの監視"],
            ],
            widths=[14, 34, 12, 46],
            merge_cols=(0,),
            center_cols=(0, 2),
        )
        xk.set_print(ws2, doc_name=info.doc_name)

        ws2b = xk.add_sheet(wb, "2.2 サーバ構成")
        r = xk.heading(ws2b, 2, 2, "2.2 サーバ構成")
        xk.table(
            ws2b,
            top=r,
            left=2,
            caption="サーバ構成",
            groups=[("", 1), ("", 1), ("スペック", 3), ("", 1)],
            header=["サーバ", "台数", "CPU", "メモリ", "ディスク", "冗長化"],
            rows=[
                ["Web/AP サーバ", "4", "8 vCPU", "32 GB", "200 GB", "ロードバランサによる冗長化"],
                ["バッチサーバ", "2", "16 vCPU", "64 GB", "500 GB", "Active-Standby"],
                ["DB サーバ", "2", "16 vCPU", "128 GB", "4 TB", "同期レプリケーション"],
                ["連携サーバ", "2", "4 vCPU", "16 GB", "1 TB", "Active-Standby"],
            ],
            widths=[18, 8, 12, 12, 14, 40],
            center_cols=(1, 2, 3, 4),
        )
        xk.set_print(ws2b, doc_name=info.doc_name)

        # 3. 共通方式
        ws3 = xk.add_sheet(wb, "3.共通方式")
        r = xk.heading(ws3, 2, 2, "3. 共通方式")
        r = xk.kv_group_table(
            ws3,
            top=r,
            left=2,
            groups=[
                ("データ整合", [
                    ("排他制御", "オンラインは楽観的排他とする。更新対象の行が持つ更新日時を画面が保持し、"
                                "更新時に一致しない場合は排他エラーとして処理を中断する。"),
                    ("トランザクション", "1 画面の 1 操作を 1 トランザクションとする。"
                                    "バッチは得意先単位でコミットし、異常時は該当得意先のみロールバックする。"),
                    ("採番", "受注番号・請求番号などの業務キーは共通の採番部品が発番する。"
                            "採番テーブルを行ロックで更新し、連番の重複を防ぐ。"),
                ]),
                ("異常時", [
                    ("エラー処理", "業務エラーは画面にメッセージを表示して処理を中断する。"
                                "システムエラーはエラー画面へ遷移させ、エラーIDを表示する。"),
                    ("ログ", "アクセスログ・アプリケーションログ・監査ログの 3 種を出力する。"
                            "監査ログは受注・在庫・請求の更新操作を対象とする。"),
                    ("バッチの多重起動防止", "ジョブ管理製品の排他制御に加え、実行管理テーブルで多重起動を検知する。"),
                ]),
                ("共通の取り決め", [
                    ("文字コード", "データベース・連携ファイル・画面のすべてを UTF-8 とする。"),
                    ("日付", "業務日付はシステム日付を用いる。営業日の判定は営業日カレンダーを参照する。"),
                    ("金額", "金額は円単位の整数で保持し、円未満は保持しない。"),
                ]),
            ],
            width=94,
        )
        xk.set_print(ws3, doc_name=info.doc_name)

        # 4. セキュリティ方式
        ws4 = xk.add_sheet(wb, "4.セキュリティ方式")
        r = xk.heading(ws4, 2, 2, "4. セキュリティ方式")
        r = xk.table(
            ws4,
            top=r,
            left=2,
            caption="セキュリティ方式",
            groups=[("", 1), ("", 1), ("対応", 1)],
            header=["観点", "方式", "対応する非機能要件"],
            rows=[
                ["認証", "社員コードとパスワードによる認証。パスワードは bcrypt でハッシュ化して保管する。", "パスワード方針"],
                ["認可", "権限ロール（営業・物流・経理・管理者）を社員に割り当て、機能単位で可否を判定する。", "利用者認証"],
                ["通信の保護", "画面・外部連携ともに TLS1.2 以上で暗号化する。", "通信の暗号化"],
                ["監査", "更新操作の実施者・日時・変更前後の値を監査ログに記録し、5 年間保存する。", "監査ログの保存期間"],
                ["セッション", "無操作 30 分でセッションを破棄し、再認証を求める。", "パスワード方針"],
                ["データの保護", "データベースの保存領域を暗号化する。バックアップも暗号化して保管する。", "バックアップ"],
            ],
            widths=[14, 84, 22],
            center_cols=(2,),
        )
        xk.set_print(ws4, doc_name=info.doc_name)

        # 5. 権限マトリクス
        ws5m = xk.add_sheet(wb, "5.権限マトリクス")
        r = xk.heading(ws5m, 2, 2, "5. 権限マトリクス")
        xk.table(
            ws5m,
            top=r,
            left=2,
            caption="権限マトリクス（○=可、△=部長職のみ可、×=不可）",
            groups=[("", 1), ("業務ロール", 3), ("", 1)],
            header=["機能", "営業", "物流", "経理", "管理者"],
            rows=[
                ["受注登録・変更・取消", "○", "×", "×", "○"],
                ["出荷指示", "○", "○", "×", "○"],
                ["与信保留の解除", "△", "×", "×", "○"],
                ["入庫登録・在庫調整", "×", "○", "×", "○"],
                ["棚卸", "×", "○", "×", "○"],
                ["請求締め・請求書発行", "×", "×", "○", "○"],
                ["入金消込", "×", "×", "○", "○"],
                ["マスタ保守", "×", "×", "×", "○"],
                ["受注・在庫・売掛の照会", "○", "○", "○", "○"],
            ],
            widths=[34, 10, 10, 10, 10],
            center_cols=(1, 2, 3, 4),
        )
        xk.set_print(ws5m, doc_name=info.doc_name)

        # 6. 受注ステータスの状態遷移（図形）
        ws5 = xk.add_sheet(wb, "6.状態遷移図", grid=True)
        c = ws5.cell(row=2, column=2, value="6. 受注ステータスの状態遷移図")
        c.font = xk.F_SECTION
        for i, text in enumerate(("凡例", "四角内の数字はコード値", "矢印の文字は契機となる操作")):
            cell = ws5.cell(row=6 + i, column=58, value=text)
            cell.font = xk.F_BODY

        ws6 = xk.add_sheet(wb, "7.状態遷移表")
        r = xk.heading(ws6, 2, 2, "7. 受注ステータスの遷移表")
        r = xk.table(
            ws6,
            top=r,
            left=2,
            caption="状態遷移表（行=遷移前、列=操作）",
            groups=[("", 1), ("操作による遷移先", 5)],
            header=["遷移前の状態", "受注登録", "与信承認", "引当完了", "出荷指示", "受注取消"],
            rows=[
                ["（なし）", "受付(10) または 与信保留(20)", "—", "—", "—", "—"],
                ["受付(10)", "—", "—", "引当済(30)", "—", "取消(90)"],
                ["与信保留(20)", "—", "受付(10)", "—", "—", "取消(90)"],
                ["引当済(30)", "—", "—", "—", "出荷指示済(40)", "取消(90)"],
                ["出荷指示済(40)", "—", "—", "—", "—", "遷移しない（取消不可）"],
                ["取消(90)", "—", "—", "—", "—", "—"],
            ],
            widths=[18, 26, 14, 14, 16, 20],
            center_cols=(1, 2, 3, 4, 5),
        )
        xk.note(ws6, r, 2, "※ 出荷指示済（40）からの取消は行えない。詳細設計の受注取消の処理仕様に従う。")
        xk.set_print(ws6, doc_name=info.doc_name)

        # 8. 業務ルール一覧（業務判断の層）
        ws8 = xk.add_sheet(wb, "8.業務ルール一覧")
        r = xk.heading(ws8, 2, 2, "8. 業務ルール一覧")
        r = xk.note(ws8, r, 2, "※ 入力チェックの詳細・端数処理・異常系の扱いは詳細設計書にて定義する。")
        xk.table(
            ws8,
            top=r,
            left=2,
            caption="業務ルール一覧",
            groups=[("区分", 3), ("", 1)],
            header=["サブシステム", "区分", "ルール名", "ルール内容"],
            rows=[
                [spec.SUBSYSTEM_NAME[sub], kind, name, text]
                for name, sub, kind, text in spec.BUSINESS_RULES
            ],
            widths=[14, 10, 28, 84],
            merge_cols=(0, 1),
            center_cols=(0, 1),
        )
        xk.set_print(ws8, doc_name=info.doc_name)

    diagrams = [
        Diagram(
            sheet="1.システム構成",
            nodes=[
                Node("client", "利用者端末\nEdge / Chrome", 4, 8, 12, 3),
                Node("lb", "ロードバランサ", 20, 8, 12, 3),
                Node("web", "Web/APサーバ\nnginx + Spring Boot\n（4台）", 36, 7, 14, 4),
                Node("db", "DBサーバ\nPostgreSQL 16\n（2台・同期レプリカ）", 56, 7, 16, 4),
                Node("batch", "バッチサーバ\nSpring Batch + JP1\n（2台）", 36, 15, 14, 4),
                Node("link", "連携サーバ\nSFTP / REST", 36, 23, 14, 3),
                Node("van", "流通BMS-VAN\n（EDI）", 12, 22, 14, 3, fill="EDEDED", line="808080"),
                Node("wms", "倉庫管理システム\nWMS", 12, 27, 14, 3, fill="EDEDED", line="808080"),
                Node("acct", f"会計システム\n{spec.ACCOUNTING_SYSTEM}", 58, 22, 14, 3,
                     fill="EDEDED", line="808080"),
                Node("bank", "全銀ネット\n（ファームバンキング）", 58, 27, 16, 3, fill="EDEDED", line="808080"),
                Node("svf", "帳票サーバ\nSVF", 56, 15, 14, 3),
                Node("mon", "監視サーバ\nZabbix", 4, 15, 12, 3),
            ],
            edges=[
                Edge("client", "lb", "HTTPS"),
                Edge("lb", "web", "HTTPS"),
                Edge("web", "db", "JDBC"),
                Edge("batch", "db", "JDBC"),
                Edge("web", "svf", "帳票要求"),
                Edge("batch", "link", "連携依頼"),
                Edge("van", "link", "受注データ"),
                Edge("wms", "link", "出荷実績"),
                Edge("link", "acct", "仕訳データ"),
                Edge("bank", "link", "入金データ"),
                Edge("mon", "web", "死活監視"),
            ],
        ),
        Diagram(
            sheet="6.状態遷移図",
            nodes=[
                Node("new", "受注の発生", 4, 12, 12, 3, shape="ellipse", fill="FFF2CC", line="BF8F00"),
                Node("s10", "受付\n10", 22, 12, 10, 3, shape="rect"),
                Node("s20", "与信保留\n20", 22, 4, 12, 3, shape="rect", fill="F8CBAD", line="C00000"),
                Node("s30", "引当済\n30", 40, 12, 10, 3, shape="rect"),
                Node("s40", "出荷指示済\n40", 58, 12, 12, 3, shape="rect", fill="E2EFDA", line="548235"),
                Node("s90", "取消\n90", 40, 22, 10, 3, shape="rect", fill="EDEDED", line="808080"),
                Node("done", "売上計上", 58, 22, 12, 3, shape="ellipse", fill="FFF2CC", line="BF8F00"),
            ],
            edges=[
                Edge("new", "s10", "受注登録"),
                Edge("new", "s20", "与信超過"),
                Edge("s20", "s10", "与信承認"),
                Edge("s10", "s30", "引当完了"),
                Edge("s30", "s40", "出荷指示"),
                Edge("s40", "done", "出荷実績受信"),
                Edge("s10", "s90", "受注取消"),
                Edge("s30", "s90", "受注取消"),
            ],
        ),
    ]
    return xk.build(out, "00_全体/02_基本設計/基本設計書_システム方式.xlsx", info, body, diagrams)


# ── 機能一覧 ──────────────────────────────────────────────────────
def _function_list(out: Path) -> Path:
    info = DocInfo(
        doc_name="機能一覧",
        subsystem="全体",
        version="1.1",
        date="2025/08/29",
        revisions=REV,
    )

    def body(wb, info):
        ws = xk.add_sheet(wb, "機能一覧")
        r = xk.heading(ws, 2, 2, "機能一覧")
        r = xk.table(
            ws,
            top=r,
            left=2,
            groups=[("", 1), ("", 1), ("", 1), ("", 1), ("対応・関連", 2)],
            header=["サブシステム", "機能名", "種別", "機能概要", "対応する要件", "関連する画面・帳票"],
            rows=[
                [spec.SUBSYSTEM_NAME[sub], name, kind, desc, req, scr]
                for name, sub, kind, desc, req, scr in FUNCTIONS
            ],
            widths=[14, 24, 12, 56, 34, 30],
            merge_cols=(0,),
            center_cols=(0, 2),
        )
        xk.set_print(ws, doc_name=info.doc_name)

        ws2 = xk.add_sheet(wb, "要件対応表")
        r = xk.heading(ws2, 2, 2, "要件 → 機能 対応表（カバレッジ確認用）")
        rows = []
        for req_name, sub, cat, _text in spec.FUNC_REQS:
            covers = [f[0] for f in FUNCTIONS if req_name in f[4].split(" / ")]
            rows.append([spec.SUBSYSTEM_NAME[sub], cat, req_name,
                         ", ".join(covers) or "（未割当）", "○" if covers else "×"])
        xk.table(
            ws2,
            top=r,
            left=2,
            caption="要件と機能の対応（要件名で引く）",
            groups=[("区分", 2), ("", 1), ("対応する機能", 2)],
            header=["サブシステム", "分類", "要件名", "機能名", "充足"],
            rows=rows,
            widths=[14, 14, 24, 44, 8],
            merge_cols=(0, 1),
            center_cols=(0, 1, 4),
        )
        xk.set_print(ws2, doc_name=info.doc_name)

    return xk.build(out, "00_全体/02_基本設計/機能一覧.xlsx", info, body)


# ── 画面一覧 ──────────────────────────────────────────────────────
def _screen_list(out: Path) -> Path:
    info = DocInfo(
        doc_name="画面一覧",
        subsystem="全体",
        version="1.1",
        date="2025/08/29",
        revisions=REV,
    )
    transitions = {
        "ログイン": "受注一覧照会（ログイン後のトップ）",
        "受注入力": "受注一覧照会（登録後）",
        "受注一覧照会": "受注入力 / 受注取消 / 出荷指示",
        "受注取消": "受注一覧照会（取消後）",
        "出荷指示": "受注一覧照会（指示後）",
        "請求締め処理": "請求書発行（締め後）",
        "請求書発行": "売掛残高照会",
        "入金消込": "売掛残高照会",
        "棚卸入力": "在庫照会",
    }
    roles = {"ORD": "営業・管理者", "INV": "物流・管理者",
             "BIL": "経理・管理者", "CMN": "全ロール"}

    def body(wb, info):
        ws = xk.add_sheet(wb, "画面一覧")
        r = xk.heading(ws, 2, 2, "画面一覧")
        r = xk.table(
            ws,
            top=r,
            left=2,
            groups=[("", 1), ("", 1), ("", 1), ("", 1), ("遷移・権限", 2)],
            header=["サブシステム", "画面名", "画面区分", "画面概要", "遷移先", "利用権限"],
            rows=[
                [spec.SUBSYSTEM_NAME[sub], name, kind, desc,
                 transitions.get(name, "—"), roles[sub]]
                for name, sub, kind, desc in spec.SCREENS
            ],
            widths=[14, 22, 10, 52, 34, 16],
            merge_cols=(0,),
            center_cols=(0, 2),
        )
        xk.note(ws, r, 2, "※ 各画面の項目・イベントはサブシステム別の画面仕様書を参照のこと。")
        xk.set_print(ws, doc_name=info.doc_name)

        ws2 = xk.add_sheet(wb, "画面遷移図", grid=True)
        c = ws2.cell(row=2, column=2, value="画面遷移図")
        c.font = xk.F_SECTION
        for i, text in enumerate(("凡例", "実線: 画面遷移", "網掛け: 共通基盤の画面")):
            cell = ws2.cell(row=6 + i, column=62, value=text)
            cell.font = xk.F_BODY

    diagrams = [
        Diagram(
            sheet="画面遷移図",
            nodes=[
                Node("login", "ログイン", 4, 14, 12, 3, fill="EDEDED", line="808080"),
                Node("list", "受注一覧照会", 20, 14, 14, 3),
                Node("entry", "受注入力", 38, 8, 12, 3),
                Node("cancel", "受注取消", 38, 14, 12, 3),
                Node("ship", "出荷指示", 38, 20, 12, 3),
                Node("stock", "在庫照会", 20, 26, 12, 3),
                Node("count", "棚卸入力", 38, 26, 12, 3),
                Node("close", "請求締め処理", 20, 32, 14, 3),
                Node("invoice", "請求書発行", 38, 32, 13, 3),
                Node("balance", "売掛残高照会", 56, 32, 14, 3),
                Node("match", "入金消込", 38, 38, 12, 3),
                Node("master", "マスタ保守\n（得意先・商品）", 4, 26, 14, 3, fill="EDEDED", line="808080"),
            ],
            edges=[
                Edge("login", "list", "認証成功"),
                Edge("list", "entry", "新規・変更"),
                Edge("list", "cancel", "取消"),
                Edge("list", "ship", "出荷指示"),
                Edge("entry", "list", "登録後"),
                Edge("count", "stock", "確定後"),
                Edge("close", "invoice", "締め後"),
                Edge("invoice", "balance", "発行後"),
                Edge("match", "balance", "消込後"),
                Edge("login", "master", "管理者"),
            ],
        )
    ]
    return xk.build(out, "00_全体/02_基本設計/画面一覧.xlsx", info, body, diagrams)


# ── テーブル定義書 ────────────────────────────────────────────────
def _table_def(out: Path) -> Path:
    info = DocInfo(
        doc_name="テーブル定義書",
        subsystem="全体",
        version="1.2",
        date="2025/08/29",
        revisions=[
            ("1.0", "2025/07/25", "初版作成", "鈴木"),
            ("1.1", "2025/08/08", "得意先マスタに締日区分を追加（要件定義レビュー指摘）", "鈴木"),
            ("1.2", "2025/08/29", "在庫テーブルに安全在庫数を追加", "鈴木"),
        ],
    )

    def body(wb, info):
        ws = xk.add_sheet(wb, "テーブル一覧")
        r = xk.heading(ws, 2, 2, "テーブル一覧")
        r = xk.table(
            ws,
            top=r,
            left=2,
            groups=[("テーブル名", 2), ("", 1), ("", 1), ("", 1)],
            header=["物理名", "論理名", "区分", "内容", "想定件数"],
            rows=[[p, l, k, d, n] for p, l, k, d, n in spec.ENTITIES],
            widths=[20, 22, 14, 66, 16],
            merge_cols=(2,),
            center_cols=(2, 4),
        )
        xk.set_print(ws, doc_name=info.doc_name)

        ws_er = xk.add_sheet(wb, "ER図", grid=True)
        c = ws_er.cell(row=2, column=2, value="ER図（主要テーブル）")
        c.font = xk.F_SECTION
        for i, text in enumerate(("凡例", "実線: 1対多", "四角: エンティティ", "上段=物理名 / 下段=論理名")):
            cell = ws_er.cell(row=5 + i, column=62, value=text)
            cell.font = xk.F_BODY

        # テーブルごとの列定義シート
        for phys, logi, _kind, _desc, _n in spec.ENTITIES:
            cols = spec.COLUMNS.get(phys)
            if not cols:
                continue
            ws2 = xk.add_sheet(wb, f"{phys}")
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
                    for i, (c_phys, c_logi, c_type, c_len, c_key, c_req, c_desc) in enumerate(cols)
                ],
                widths=[5, 22, 22, 12, 8, 8, 8, 70],
                center_cols=(0, 3, 4, 5, 6),
            )
            xk.set_print(ws2, doc_name=info.doc_name)

        # インデックスは列の組み立てが列定義と揃わないので、テーブル横断で 1 シートに集める。
        ws_ix = xk.add_sheet(wb, "インデックス一覧")
        r = xk.heading(ws_ix, 2, 2, "インデックス一覧（テーブル横断）")
        xk.table(
            ws_ix,
            top=r,
            left=2,
            caption="インデックス一覧",
            groups=[("", 1), ("索引", 2), ("", 1), ("", 1)],
            header=["テーブル", "索引名", "構成列", "一意", "用途"],
            rows=[
                [phys, *row]
                for phys, _l, _k, _d, _n in spec.ENTITIES
                if spec.COLUMNS.get(phys)
                for row in _indexes(phys)
            ],
            widths=[22, 26, 40, 8, 50],
            merge_cols=(0,),
            center_cols=(3,),
            header_fill=xk.FILL_HEAD2,
        )
        xk.set_print(ws_ix, doc_name=info.doc_name)

    diagrams = [
        Diagram(
            sheet="ER図",
            nodes=[
                Node("cust", "M_CUSTOMER\n得意先マスタ", 4, 11, 14, 3, shape="rect"),
                Node("order", "T_ORDER\n受注ヘッダ", 24, 11, 14, 3, shape="rect"),
                Node("detail", "T_ORDER_DETAIL\n受注明細", 44, 11, 14, 3, shape="rect"),
                Node("prod", "M_PRODUCT\n商品マスタ", 44, 5, 14, 3, shape="rect"),
                Node("price", "M_PRICE\n得意先別単価マスタ", 24, 5, 16, 3, shape="rect"),
                Node("alloc", "T_ALLOCATION\n引当", 64, 11, 14, 3, shape="rect"),
                Node("stock", "T_STOCK\n在庫", 64, 18, 14, 3, shape="rect"),
                Node("wh", "M_WAREHOUSE\n倉庫マスタ", 44, 18, 14, 3, shape="rect"),
                Node("ship", "T_SHIPMENT\n出荷指示", 24, 18, 14, 3, shape="rect"),
                Node("inv", "T_INVOICE\n請求ヘッダ", 4, 18, 14, 3, shape="rect"),
                Node("invd", "T_INVOICE_DETAIL\n請求明細", 4, 25, 14, 3, shape="rect"),
                Node("dep", "T_DEPOSIT\n入金", 24, 25, 14, 3, shape="rect"),
                Node("hist", "T_STOCK_HISTORY\n在庫移動履歴", 64, 25, 16, 3, shape="rect"),
            ],
            edges=[
                Edge("cust", "order", "1対多"),
                Edge("cust", "price", "1対多"),
                Edge("order", "detail", "1対多"),
                Edge("prod", "detail", "1対多"),
                Edge("detail", "alloc", "1対多"),
                Edge("stock", "alloc", "1対多"),
                Edge("wh", "stock", "1対多"),
                Edge("order", "ship", "1対多"),
                Edge("cust", "inv", "1対多"),
                Edge("inv", "invd", "1対多"),
                Edge("inv", "dep", "1対多"),
                Edge("stock", "hist", "1対多"),
            ],
        )
    ]
    return xk.build(out, "00_全体/02_基本設計/テーブル定義書.xlsx", info, body, diagrams)


def _indexes(phys: str) -> list[list[str]]:
    table = {
        "M_CUSTOMER": [
            ["PK_M_CUSTOMER", "CUSTOMER_CD", "○", "主キー"],
            ["IX_M_CUSTOMER_01", "CUSTOMER_KANA", "×", "カナ名称での検索"],
        ],
        "M_PRODUCT": [
            ["PK_M_PRODUCT", "PRODUCT_CD", "○", "主キー"],
            ["IX_M_PRODUCT_01", "JAN_CD", "×", "JANコードでの検索"],
        ],
        "T_ORDER": [
            ["PK_T_ORDER", "ORDER_NO", "○", "主キー"],
            ["IX_T_ORDER_01", "ORDER_DATE, CUSTOMER_CD", "×", "受注一覧照会の検索条件"],
            ["IX_T_ORDER_02", "CUSTOMER_CD, ORDER_STATUS", "×", "与信チェックでの未請求受注の集計"],
        ],
        "T_ORDER_DETAIL": [
            ["PK_T_ORDER_DETAIL", "ORDER_NO, ORDER_LINE_NO", "○", "主キー"],
            ["IX_T_ORDER_DETAIL_01", "PRODUCT_CD", "×", "商品別の受注実績集計"],
        ],
        "T_STOCK": [
            ["PK_T_STOCK", "WAREHOUSE_CD, PRODUCT_CD", "○", "主キー"],
            ["IX_T_STOCK_01", "PRODUCT_CD", "×", "商品を指定した在庫照会"],
        ],
        "T_INVOICE": [
            ["PK_T_INVOICE", "INVOICE_NO", "○", "主キー"],
            ["IX_T_INVOICE_01", "CLOSING_YM, CUSTOMER_CD", "○", "締め年月と得意先での一意性担保"],
        ],
    }
    return table.get(phys, [["PK_" + phys, "（主キー）", "○", "主キー"]])


# ── データ項目定義書 ──────────────────────────────────────────────
def _data_item_def(out: Path) -> Path:
    info = DocInfo(
        doc_name="データ項目定義書",
        subsystem="全体",
        version="1.1",
        date="2025/08/29",
        revisions=REV,
    )

    def body(wb, info):
        ws = xk.add_sheet(wb, "ドメイン定義")
        r = xk.heading(ws, 2, 2, "1. ドメイン定義（共通の型と桁）")
        r = xk.table(
            ws,
            top=r,
            left=2,
            caption="ドメイン定義",
            groups=[("", 1), ("データ型", 2), ("", 1), ("", 1)],
            header=["ドメイン名", "型", "桁数", "入力規則", "説明"],
            rows=[
                ["得意先コード", "CHAR", "8", "半角数字。前ゼロ埋め", "得意先を識別するコード。現行 6 桁を移行時に 8 桁へ変換する"],
                ["商品コード", "CHAR", "10", "半角英数字", "商品を識別するコード"],
                ["倉庫コード", "CHAR", "4", "半角数字", "倉庫を識別するコード"],
                ["社員コード", "CHAR", "6", "半角数字", "社員を識別するコード"],
                ["受注番号", "CHAR", "12", "R + YYYYMMDD + 3 桁連番", "受注を識別する番号。採番部品が発番する"],
                ["請求番号", "CHAR", "12", "B + YYYYMM + 5 桁連番", "請求を識別する番号"],
                ["数量", "DECIMAL", "9,2", "0 より大きい数値", "受注・在庫で扱う数量。小数第 2 位まで"],
                ["単価", "DECIMAL", "10,2", "0 以上の数値", "販売単価・標準単価。小数第 2 位まで"],
                ["金額", "DECIMAL", "12,0", "整数（円単位）", "明細金額・請求金額。円未満は保持しない"],
                ["区分", "CHAR", "1〜2", "コード定義書に定める値", "ステータスや種別を表すコード値"],
                ["フラグ", "CHAR", "1", "0 または 1", "0=off、1=on"],
                ["日付", "DATE", "—", "YYYY-MM-DD", "業務上の日付"],
                ["日時", "TIMESTAMP", "—", "YYYY-MM-DD HH:MM:SS.SSS", "更新日時など時刻を伴う項目"],
            ],
            widths=[18, 12, 10, 30, 66],
            center_cols=(1, 2),
        )
        xk.set_print(ws, doc_name=info.doc_name)

        ws2 = xk.add_sheet(wb, "項目一覧")
        r = xk.heading(ws2, 2, 2, "2. データ項目一覧（テーブル横断）")
        rows = []
        for phys, logi, _k, _d, _n in spec.ENTITIES:
            for c_phys, c_logi, c_type, c_len, _key, c_req, c_desc in spec.COLUMNS.get(phys, []):
                rows.append([logi, c_logi, c_phys, c_type, c_len, c_req, c_desc])
        r = xk.table(
            ws2,
            top=r,
            left=2,
            caption="データ項目一覧",
            groups=[("", 1), ("項目名", 2), ("データ型", 2), ("", 1), ("", 1)],
            header=["テーブル", "項目名", "物理名", "型", "桁", "必須", "内容"],
            rows=rows,
            widths=[20, 22, 22, 12, 8, 8, 74],
            merge_cols=(0,),
            center_cols=(3, 4, 5),
        )
        xk.note(ws2, r, 2, "※ 桁数・入力規則の詳細はドメイン定義に従う。個別に異なる場合のみ本表に記載する。")
        xk.set_print(ws2, doc_name=info.doc_name)

    return xk.build(out, "00_全体/02_基本設計/データ項目定義書.xlsx", info, body)


# ── コード定義書 ──────────────────────────────────────────────────
def _code_def(out: Path) -> Path:
    info = DocInfo(
        doc_name="コード定義書",
        subsystem="全体",
        version="1.0",
        date="2025/08/29",
        revisions=[("1.0", "2025/08/29", "初版作成", "鈴木")],
    )

    def body(wb, info):
        ws = xk.add_sheet(wb, "コード定義")
        r = xk.heading(ws, 2, 2, "コード定義")
        r = xk.table(
            ws,
            top=r,
            left=2,
            caption="コード定義（業務データの値）",
            groups=[("", 1), ("コード", 2), ("", 1)],
            header=["コード種別", "値", "名称", "意味"],
            rows=[[g, v, n, d] for g, v, n, d in spec.CODES],
            widths=[20, 10, 18, 66],
            merge_cols=(0,),
            center_cols=(1,),
        )
        xk.note(ws, r, 2, "※ コード値の追加はマスタ保守では行えない。追加時はプログラム修正を伴う。")
        xk.set_print(ws, doc_name=info.doc_name)

    return xk.build(out, "00_全体/02_基本設計/コード定義書.xlsx", info, body)


# ── 外部インターフェース仕様書 ────────────────────────────────────
_IF_LAYOUTS = {
    "EDI受注データ受信": [
        ["1", "発注番号", "X(20)", "必須", "取引先が採番した発注の番号"],
        ["2", "発注日", "9(8)", "必須", "YYYYMMDD"],
        ["3", "取引先コード", "X(13)", "必須", "GLN。得意先コードへ読み替える"],
        ["4", "商品コード（JAN）", "X(13)", "必須", "商品マスタの JANコードで読み替える"],
        ["5", "発注数量", "9(7)V9(2)", "必須", "ケース単位"],
        ["6", "希望納品日", "9(8)", "必須", "YYYYMMDD"],
        ["7", "届け先コード", "X(13)", "任意", "指定がなければ得意先の既定届け先を使う"],
    ],
    "売上仕訳連携": [
        ["1", "伝票日付", "9(8)", "必須", "売上計上日。YYYYMMDD"],
        ["2", "借方勘定科目コード", "X(6)", "必須", "売掛金の科目コード（固定: 130010）"],
        ["3", "貸方勘定科目コード", "X(6)", "必須", "売上高の科目コード（固定: 410010）"],
        ["4", "取引先コード", "X(8)", "必須", "得意先コード"],
        ["5", "金額", "9(12)", "必須", "税抜の売上金額"],
        ["6", "消費税額", "9(12)", "必須", "消費税額"],
        ["7", "摘要", "X(40)", "任意", "受注番号を設定する"],
    ],
    "入金データ受信": [
        ["1", "データ区分", "9(1)", "必須", "1=ヘッダ、2=データ、8=トレーラ"],
        ["2", "入金日", "9(8)", "必須", "YYYYMMDD"],
        ["3", "振込依頼人名", "X(48)", "必須", "半角カナ。得意先の照合に使う"],
        ["4", "入金金額", "9(10)", "必須", "円単位"],
        ["5", "取引区分", "9(1)", "必須", "1=振込、2=手形、3=相殺"],
        ["6", "EDI情報", "X(20)", "任意", "請求番号が設定されていれば消込に使う"],
    ],
}


def _interface_spec(out: Path) -> Path:
    info = DocInfo(
        doc_name="外部インターフェース仕様書",
        subsystem="全体",
        version="1.1",
        date="2025/08/29",
        revisions=[
            ("1.0", "2025/08/08", "初版作成", "山田"),
            ("1.1", "2025/08/29", "出荷実績受信を暫定とする旨を追記", "山田"),
        ],
    )

    def body(wb, info):
        ws = xk.add_sheet(wb, "IF一覧")
        r = xk.heading(ws, 2, 2, "外部インターフェース一覧")
        r = xk.table(
            ws,
            top=r,
            left=2,
            groups=[("", 1), ("", 1), ("連携方式", 3), ("", 1)],
            header=["IF名", "相手システム", "方向", "方式", "周期", "内容"],
            rows=[[n, p, d, m, c, t] for n, p, d, m, c, t in spec.INTERFACES],
            widths=[22, 28, 8, 26, 12, 60],
            center_cols=(2, 4),
        )
        r = xk.note(
            ws, r, 2,
            "※ 出荷実績受信は WMS ベンダから API 仕様が未提示のため暫定である。"
            "仕様入手後に本書を改訂する。（課題管理表を参照）",
        )
        xk.set_print(ws, doc_name=info.doc_name)

        ws_d = xk.add_sheet(wb, "外部連携図", grid=True)
        c = ws_d.cell(row=2, column=2, value="外部システム連携図")
        c.font = xk.F_SECTION
        for i, text in enumerate(("凡例", "→ の向きがデータの流れ", "網掛け: 社外・他システム")):
            cell = ws_d.cell(row=6 + i, column=60, value=text)
            cell.font = xk.F_BODY

        for name, partner, direction, method, cycle, desc in spec.INTERFACES:
            ws2 = xk.add_sheet(wb, name[:31])
            r = xk.heading(ws2, 2, 2, name)
            r = xk.kv_group_table(
                ws2,
                top=r,
                left=2,
                groups=[
                    ("連携の概要", [
                        ("相手システム", partner),
                        ("方向", direction),
                        ("方式", method),
                        ("周期", cycle),
                        ("概要", desc),
                    ]),
                    ("異常時", [
                        ("異常時の扱い", _if_error(name)),
                        ("再送・再実行", _if_retry(name)),
                    ]),
                ],
                width=94,
            )
            xk.set_print(ws2, doc_name=info.doc_name)

            # レイアウトは No 列が要るぶん概要と列幅が揃わないので別シートにする。
            if name in _IF_LAYOUTS:
                ws3 = xk.add_sheet(wb, f"{name} レイアウト"[:31])
                r = xk.heading(ws3, 2, 2, f"{name}　レイアウト")
                xk.table(
                    ws3,
                    top=r,
                    left=2,
                    caption=f"{name}　レイアウト",
                    groups=[("", 1), ("", 1), ("形式", 2), ("", 1)],
                    header=["No", "項目名", "型・桁", "必須", "内容"],
                    rows=_IF_LAYOUTS[name],
                    widths=[6, 26, 16, 8, 70],
                    center_cols=(0, 2, 3),
                )
                xk.set_print(ws3, doc_name=info.doc_name)

    diagrams = [
        Diagram(
            sheet="外部連携図",
            nodes=[
                Node("sps", "新販売管理システム", 30, 16, 18, 4),
                Node("van", "流通BMS-VAN", 4, 10, 14, 3, fill="EDEDED", line="808080"),
                Node("wms", "倉庫管理システム", 4, 22, 16, 3, fill="EDEDED", line="808080"),
                Node("acct", "会計システム", 56, 10, 14, 3, fill="EDEDED", line="808080"),
                Node("bank", "全銀ネット", 56, 22, 14, 3, fill="EDEDED", line="808080"),
                Node("credit", "信用調査会社", 30, 4, 14, 3, fill="EDEDED", line="808080"),
                Node("hr", "人事システム", 30, 28, 14, 3, fill="EDEDED", line="808080"),
            ],
            edges=[
                Edge("van", "sps", "EDI受注データ受信"),
                Edge("wms", "sps", "出荷実績受信"),
                Edge("sps", "acct", "売上仕訳連携"),
                Edge("bank", "sps", "入金データ受信"),
                Edge("sps", "credit", "与信情報照会"),
                Edge("hr", "sps", "社員マスタ連携"),
            ],
        )
    ]
    return xk.build(out, "00_全体/02_基本設計/外部インターフェース仕様書.xlsx", info, body, diagrams)


def _if_error(name: str) -> str:
    return {
        "EDI受注データ受信": "項目の形式不正・マスタ未登録は当該明細をエラーとし、"
                     "EDI受注取込結果へ理由を記録する。"
                     "ファイル全体が読めない場合は取込を中断し、運用担当へ通知する。",
        "売上仕訳連携": "送信に失敗した場合はファイルを保持し、翌日の連携で再送する。",
        "出荷実績受信": "受信した出荷実績に対応する出荷指示が存在しない場合はエラーとし、"
                  "物流部へ通知して手動で調査する。",
        "与信情報照会": "外部 API がタイムアウトした場合は与信チェックを保留とし、"
                  "受注ステータスを与信保留（20）にする。",
        "入金データ受信": "得意先を特定できない入金は「未消込」として保留し、経理部が画面から手動で消し込む。",
    }[name]


def _if_retry(name: str) -> str:
    return {
        "EDI受注データ受信": "同一の発注番号を再受信した場合は後着を無視する（冪等）。",
        "売上仕訳連携": "運用担当の指示により、日付を指定して再作成できる。",
        "出荷実績受信": "3 回まで自動再試行する。間隔は 1 分・5 分・15 分とする。",
        "与信情報照会": "自動再試行は行わない。営業担当が画面から再照会する。",
        "入金データ受信": "同一ファイルの再取込は取込済チェックで拒否する。",
    }[name]


# ── バッチ処理一覧（基本設計側）──────────────────────────────────
def _batch_list(out: Path) -> Path:
    info = DocInfo(
        doc_name="バッチ処理一覧",
        subsystem="全体",
        version="1.0",
        date="2025/08/29",
        revisions=[("1.0", "2025/08/29", "初版作成", "高橋")],
    )

    def body(wb, info):
        ws = xk.add_sheet(wb, "バッチ一覧")
        r = xk.heading(ws, 2, 2, "バッチ処理一覧")
        r = xk.table(
            ws,
            top=r,
            left=2,
            groups=[("", 1), ("", 1), ("起動", 3), ("", 1)],
            header=["ジョブ名", "サブシステム", "起動契機", "起動時刻", "先行ジョブ", "処理概要"],
            rows=[[n, s, t, h, p, d] for n, s, t, h, p, d in JOBS],
            widths=[24, 14, 16, 34, 20, 60],
            center_cols=(1, 2),
        )
        xk.set_print(ws, doc_name=info.doc_name)

        ws_e = xk.add_sheet(wb, "異常時の運用")
        r = xk.heading(ws_e, 2, 2, "異常時の運用")
        xk.table(
            ws_e,
            top=r,
            left=2,
            groups=[("", 1), ("異常時", 2)],
            header=["ジョブ名", "異常時の扱い", "リカバリ手順"],
            rows=[
                ["EDI受注取込", "エラー明細をスキップし処理を継続する", "取込結果照会でエラーを確認し、画面から手入力する"],
                ["出荷実績取込", "エラー明細をスキップし処理を継続する", "物流部が調査のうえ在庫調整で補正する"],
                ["請求締め", "得意先単位でロールバックし処理を継続する", "原因を除去したうえで得意先を指定して再実行する"],
                ["売上仕訳連携", "ジョブを異常終了させる", "原因を除去したうえで日付を指定して再実行する"],
            ],
            widths=[20, 42, 62],
            header_fill=xk.FILL_HEAD2,
        )
        xk.set_print(ws_e, doc_name=info.doc_name)

        ws2 = xk.add_sheet(wb, "ジョブフロー図", grid=True)
        c = ws2.cell(row=2, column=2, value="夜間バッチのジョブフロー")
        c.font = xk.F_SECTION
        for i, text in enumerate(("凡例", "実線: ジョブの先行後続", "括弧内は起動時刻")):
            cell = ws2.cell(row=6 + i, column=58, value=text)
            cell.font = xk.F_BODY

    diagrams = [
        Diagram(
            sheet="ジョブフロー図",
            nodes=[
                Node("start", "日次処理開始\n（01:00）", 4, 10, 13, 3, shape="ellipse",
                     fill="FFF2CC", line="BF8F00"),
                Node("backup", "データベース\nバックアップ", 20, 10, 14, 3),
                Node("audit", "監査ログ退避\n（02:00）", 20, 16, 14, 3),
                Node("master", "マスタ連携取込\n（06:00）", 38, 10, 14, 3),
                Node("edi", "EDI受注取込\n（07:30〜20:30 / 6回）", 38, 16, 18, 3),
                Node("deposit", "入金データ取込\n（09:00）", 38, 22, 14, 3),
                Node("ship", "出荷実績取込\n（毎時05分）", 60, 16, 14, 3),
                Node("close", "請求締め\n（21:00）", 60, 22, 13, 3, fill="E2EFDA", line="548235"),
                Node("journal", "売上仕訳連携\n（締め完了後）", 60, 28, 15, 3, fill="E2EFDA", line="548235"),
                Node("end", "日次処理終了", 38, 28, 14, 3, shape="ellipse", fill="FFF2CC", line="BF8F00"),
            ],
            edges=[
                Edge("start", "backup"),
                Edge("backup", "audit"),
                Edge("backup", "master"),
                Edge("master", "edi"),
                Edge("edi", "ship"),
                Edge("edi", "deposit"),
                Edge("ship", "close", "先行"),
                Edge("close", "journal", "先行"),
                Edge("journal", "end"),
            ],
        )
    ]
    return xk.build(out, "00_全体/02_基本設計/バッチ処理一覧.xlsx", info, body, diagrams)
