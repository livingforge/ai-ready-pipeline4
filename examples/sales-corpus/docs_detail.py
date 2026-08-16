"""03_詳細設計（8 本）。工程レイヤ 3。

ここが上流と食い違う層である。詳細設計は基本設計より後に書かれ、途中で
決まった変更（課題管理表に載っている 4 件）が**上流に反映されないまま**
下流にだけ入っている、という現場でよくある状態を再現している:

- 在庫引当のタイミング   基本設計「受注確定時」 ↔ 詳細設計「出荷指示の実行時」
- 受注取消の期限         要件定義「翌営業日まで」 ↔ 詳細設計「出荷指示前まで」
- 消費税の計算単位       基本設計「明細単位・切り捨て」 ↔ 詳細設計「請求単位・四捨五入」
- 請求の締め日           基本設計「毎月20日」 → 詳細設計「締日区分に従う」（粒度差）

モジュール・メソッド・メッセージに ID は振らない。参照はクラス名・メソッド名・
メッセージ本文で行う（クラス名はコード上の識別子なのでそのまま書く）。

図表: 受注登録・受注取消・在庫引当・請求締めの処理フロー図（Excel 図形）。
"""

from __future__ import annotations

from pathlib import Path

import spec
import xlsxkit as xk
from xlsxkit import Diagram, DocInfo, Edge, Node

REV = [
    ("1.0", "2025/10/31", "初版作成", "鈴木"),
    ("1.1", "2025/11/28", "詳細設計レビュー指摘を反映", "鈴木"),
]

# 機能名との対応（クラス名 → 基本設計の機能名）。ID ではなく名前で引く。
_FUNC_OF = {
    "OrderRegistService": "受注登録",
    "OrderCancelService": "受注取消",
    "EdiOrderImportBatch": "EDI受注取込",
    "ShipmentInstructionService": "出荷指示",
    "OrderSearchService": "受注照会",
    "StockAllocationService": "在庫引当",
    "StockUpdateService": "入庫登録",
    "InventoryCountService": "棚卸",
    "ShipmentResultImportService": "出荷実績取込",
    "BillingCloseBatch": "請求締め",
    "InvoicePrintService": "請求書発行",
    "DepositMatchingService": "入金消込",
    "SalesJournalExportBatch": "売上仕訳連携",
    "AuthService": "ログイン・ログアウト",
    "AuditLogger": "監査ログ出力",
    "NumberingService": "—",
    "TaxCalculator": "—",
    "BusinessDayCalendar": "—",
}


def build(out: Path) -> list[Path]:
    return [
        _module_list(out),
        _proc_order_regist(out),
        _proc_order_cancel(out),
        _proc_allocation(out),
        _proc_billing_close(out),
        _batch_spec(out),
        _common_parts(out),
        _message_def(out),
    ]


# ── モジュール一覧 ────────────────────────────────────────────────
def _module_list(out: Path) -> Path:
    info = DocInfo(
        doc_name="モジュール一覧",
        subsystem="全体",
        version="1.1",
        date="2025/11/28",
        revisions=REV,
    )

    def body(wb, info):
        ws = xk.add_sheet(wb, "モジュール一覧")
        r = xk.heading(ws, 2, 2, "モジュール一覧")
        r = xk.table(
            ws,
            top=r,
            left=2,
            groups=[("", 1), ("", 1), ("モジュール", 2), ("", 1), ("関連", 1)],
            header=["サブシステム", "区分", "モジュール名", "クラス名", "処理概要", "関連する機能"],
            rows=[
                [spec.SUBSYSTEM_NAME[sub], kind, name, cls, desc, _FUNC_OF.get(cls, "—")]
                for name, cls, sub, kind, desc in spec.MODULES
            ],
            widths=[14, 12, 26, 30, 62, 22],
            merge_cols=(0, 1),
            center_cols=(0, 1),
        )
        xk.set_print(ws, doc_name=info.doc_name)

        ws2 = xk.add_sheet(wb, "メソッド一覧")
        r = xk.heading(ws2, 2, 2, "メソッド一覧")
        name_of = {cls: name for name, cls, _s, _k, _d in spec.MODULES}
        xk.table(
            ws2,
            top=r,
            left=2,
            groups=[("所属モジュール", 2), ("メソッド", 2), ("", 1)],
            header=["モジュール名", "クラス名", "メソッド名", "シグネチャ", "処理概要"],
            rows=[
                [name_of.get(cls, ""), cls, name, sig, desc]
                for cls, name, sig, desc in spec.METHODS
            ],
            widths=[24, 30, 24, 56, 66],
            merge_cols=(0, 1),
        )
        xk.set_print(ws2, doc_name=info.doc_name)

        # レイヤと責務・呼出関係は B〜E の列幅を共有できるので 1 シートに積む。
        ws3 = xk.add_sheet(wb, "クラス構成")
        r = xk.heading(ws3, 2, 2, "レイヤと責務")
        r = xk.table(
            ws3,
            top=r,
            left=2,
            groups=[("", 1), ("", 1), ("", 1), ("規約", 1)],
            header=["レイヤ", "パッケージ", "責務", "命名規約"],
            rows=[
                ["プレゼンテーション", "jp.co.contoso.sps.<sub>.web", "画面からの入力を受け取り、サービスを呼び出す", "〜Controller"],
                ["アプリケーション", "jp.co.contoso.sps.<sub>.service", "業務ロジックとトランザクション境界を持つ", "〜Service"],
                ["ドメイン", "jp.co.contoso.sps.<sub>.domain", "業務ルールの判定と計算を行う", "〜Rule / 〜Calculator"],
                ["データアクセス", "jp.co.contoso.sps.<sub>.repository", "テーブルへの参照・更新を行う", "〜Repository / 〜Mapper"],
                ["バッチ", "jp.co.contoso.sps.<sub>.batch", "Spring Batch のジョブ・ステップを構成する", "〜Batch / 〜Tasklet"],
                ["共通", "jp.co.contoso.sps.common", "採番・消費税計算・営業日判定・監査ログを提供する", "〜Service / 〜Logger"],
            ],
            widths=[26, 32, 60, 24],
            center_cols=(0,),
        )
        r = xk.heading(ws3, r, 2, "モジュール間の呼出関係")
        xk.table(
            ws3,
            top=r,
            left=2,
            caption="呼出関係（呼出元 → 呼出先）",
            groups=[("呼出", 2), ("", 1)],
            header=["呼出元", "呼出先", "呼び出す目的"],
            rows=[
                ["OrderRegistService", "NumberingService", "受注番号を採番する"],
                ["OrderRegistService", "TaxCalculator", "消費税額の参考値を計算する"],
                ["OrderRegistService", "BusinessDayCalendar", "納品希望日が営業日かを判定する"],
                ["OrderRegistService", "AuditLogger", "受注登録の操作を記録する"],
                ["OrderCancelService", "StockAllocationService", "引当を解放する"],
                ["ShipmentInstructionService", "StockAllocationService", "在庫を引き当てる"],
                ["ShipmentResultImportService", "StockUpdateService", "出庫として在庫を減算する"],
                ["BillingCloseBatch", "TaxCalculator", "請求単位の消費税額を計算する"],
                ["BillingCloseBatch", "NumberingService", "請求番号を採番する"],
                ["BillingCloseBatch", "BusinessDayCalendar", "締め日が休日かを判定する"],
                ["DepositMatchingService", "AuditLogger", "消込の操作を記録する"],
            ],
            widths=[26, 32, 60],
        )
        xk.set_print(ws3, doc_name=info.doc_name)

    return xk.build(out, "00_全体/03_詳細設計/モジュール一覧.xlsx", info, body)


# ── 処理仕様書の共通の型 ──────────────────────────────────────────
def _proc_spec(
    out: Path,
    *,
    rel: str,
    doc_name: str,
    sub: str,
    class_name: str,
    overview: list[tuple[str, list[tuple[str, str]]]],
    steps: list[list[str]],
    checks: list[list[str]],
    errors: list[list[str]],
    diagram: Diagram,
    revisions: list[tuple[str, str, str, str]] | None = None,
    extra: list[tuple[str, list[str], list[list[str]], list[int], list]] | None = None,
) -> Path:
    info = DocInfo(
        doc_name=doc_name,
        subsystem=spec.SUBSYSTEM_NAME[sub],
        version="1.1",
        date="2025/11/28",
        revisions=revisions or REV,
    )

    def body(wb, info):
        ws = xk.add_sheet(wb, "1.処理概要")
        r = xk.heading(ws, 2, 2, "1. 処理概要")
        xk.kv_group_table(ws, top=r, left=2, groups=overview, width=94)
        xk.set_print(ws, doc_name=info.doc_name)

        ws1b = xk.add_sheet(wb, "2.メソッド一覧")
        r = xk.heading(ws1b, 2, 2, "2. メソッド一覧")
        xk.table(
            ws1b,
            top=r,
            left=2,
            caption=f"{class_name}　メソッド一覧",
            groups=[("メソッド", 2), ("", 1)],
            header=["メソッド名", "シグネチャ", "処理概要"],
            rows=[[name, sig, desc] for _c, name, sig, desc in spec.methods_of(class_name)],
            widths=[24, 58, 70],
        )
        xk.set_print(ws1b, doc_name=info.doc_name)

        ws2 = xk.add_sheet(wb, diagram.sheet, grid=True)
        c = ws2.cell(row=2, column=2, value=f"3. 処理フロー（{doc_name}）")
        c.font = xk.F_SECTION
        for i, text in enumerate(("凡例", "菱形: 判定", "角丸: 処理", "網掛け: 他モジュール呼出")):
            cell = ws2.cell(row=5 + i, column=58, value=text)
            cell.font = xk.F_BODY

        ws3 = xk.add_sheet(wb, "4.処理詳細")
        r = xk.heading(ws3, 2, 2, "4. 処理詳細")
        r = xk.table(
            ws3,
            top=r,
            left=2,
            caption="処理詳細（ステップ順）",
            groups=[("", 1), ("", 1), ("アクセスするテーブル", 2), ("", 1)],
            header=["No", "処理内容", "参照", "更新", "例外時の動作"],
            rows=steps,
            widths=[6, 86, 26, 26, 46],
            center_cols=(0,),
        )
        xk.set_print(ws3, doc_name=info.doc_name)

        ws4 = xk.add_sheet(wb, "5.入力チェック")
        r = xk.heading(ws4, 2, 2, "5. 入力チェック一覧")
        xk.table(
            ws4,
            top=r,
            left=2,
            caption="入力チェック一覧",
            groups=[("", 1), ("", 1), ("", 1), ("", 1), ("出力するメッセージ", 1)],
            header=["No", "対象項目", "チェック種別", "チェック内容", "メッセージ"],
            rows=checks,
            widths=[6, 24, 16, 72, 40],
            center_cols=(0, 2),
        )
        xk.set_print(ws4, doc_name=info.doc_name)

        ws4b = xk.add_sheet(wb, "6.異常系の扱い")
        r = xk.heading(ws4b, 2, 2, "6. 異常系の扱い")
        xk.table(
            ws4b,
            top=r,
            left=2,
            caption="異常系の扱い",
            groups=[("", 1), ("", 1), ("", 1), ("出力するメッセージ", 1)],
            header=["No", "発生条件", "システムの動作", "メッセージ"],
            rows=errors,
            widths=[6, 50, 62, 40],
            center_cols=(0,),
            header_fill=xk.FILL_HEAD2,
        )
        xk.set_print(ws4b, doc_name=info.doc_name)

        for title, header, rows, widths, groups in extra or []:
            ws5 = xk.add_sheet(wb, title[:31])
            r = xk.heading(ws5, 2, 2, title)
            xk.table(ws5, top=r, left=2, header=header, rows=rows,
                     widths=widths, groups=groups or None)
            xk.set_print(ws5, doc_name=info.doc_name)

    return xk.build(out, rel, info, body, [diagram])


# ── 処理仕様書（受注登録）─────────────────────────────────────────
def _proc_order_regist(out: Path) -> Path:
    diagram = Diagram(
        sheet="3.処理フロー",
        nodes=[
            Node("start", "開始", 6, 8, 8, 2, shape="ellipse", fill="FFF2CC", line="BF8F00"),
            Node("valid", "入力値検証\nvalidateOrder", 6, 12, 14, 3),
            Node("j1", "検証OK？", 6, 17, 12, 3, shape="diamond", fill="FCE4D6", line="C55A11"),
            Node("err", "エラー返却\n（画面へメッセージ）", 24, 17, 16, 3, fill="F8CBAD", line="C00000"),
            Node("price", "販売単価決定\nresolveUnitPrice", 6, 22, 14, 3),
            Node("calc", "金額・消費税計算\ncalcAmount", 6, 27, 14, 3),
            Node("credit", "与信確認\ncheckCredit", 6, 32, 14, 3),
            Node("j2", "与信内？", 6, 37, 12, 3, shape="diamond", fill="FCE4D6", line="C55A11"),
            Node("hold", "受注ステータス\n= 20（与信保留）", 24, 37, 14, 3, fill="FFF2CC", line="BF8F00"),
            Node("no", "受注番号採番\nNumberingService", 6, 42, 14, 3, fill="EDEDED", line="808080"),
            Node("insert", "受注ヘッダ・明細登録\nT_ORDER / T_ORDER_DETAIL", 6, 47, 16, 3),
            Node("audit", "監査ログ出力\nAuditLogger", 6, 52, 14, 3, fill="EDEDED", line="808080"),
            Node("end", "終了", 6, 57, 8, 2, shape="ellipse", fill="FFF2CC", line="BF8F00"),
        ],
        edges=[
            Edge("start", "valid"),
            Edge("valid", "j1"),
            Edge("j1", "err", "NG"),
            Edge("j1", "price", "OK"),
            Edge("price", "calc"),
            Edge("calc", "credit"),
            Edge("credit", "j2"),
            Edge("j2", "hold", "超過"),
            Edge("j2", "no", "以内"),
            Edge("hold", "no"),
            Edge("no", "insert"),
            Edge("insert", "audit"),
            Edge("audit", "end"),
        ],
    )
    return _proc_spec(
        out,
        rel=spec.path_of("ORD", "詳細設計", "処理仕様書_受注登録.xlsx"),
        doc_name="処理仕様書（受注登録）",
        sub="ORD",
        class_name="OrderRegistService",
        overview=[
            ("対象", [
                ("モジュール名", "受注登録サービス"),
                ("クラス名", "OrderRegistService"),
                ("関連する機能", "受注登録（基本設計の機能一覧）"),
                ("関連する画面", "受注入力"),
            ]),
            ("呼出関係", [
                ("呼出元", "OrderController#regist（受注入力画面の登録ボタン）"),
                ("呼出先", "NumberingService, TaxCalculator, BusinessDayCalendar, AuditLogger"),
                ("呼び出さないもの", "★StockAllocationService。引当は出荷指示の実行時に行う"),
            ]),
            ("方式", [
                ("トランザクション", "本メソッドの開始から終了までを 1 トランザクションとする。"
                            "例外が発生した場合はすべてロールバックする。"),
                ("排他制御", "得意先マスタ・商品マスタは参照のみ。受注は新規登録のため排他は不要。"),
            ]),
            ("処理の要点", [
                ("処理概要", "受注入力画面から渡された受注データを検証し、販売単価と金額を確定し、"
                        "与信を確認したうえで受注ヘッダ・受注明細を登録する。"),
                ("引当の扱い", "★本処理では在庫の引当を行わない。引当は出荷指示の実行時に "
                        "在庫引当サービスが行う（課題管理表の「在庫引当のタイミング」の決定による）。"),
                ("消費税の扱い", "★受注時点の消費税額は参考値である。確定は請求締めで行う。"),
            ]),
        ],
        steps=[
            ["1", "入力値を検証する。エラーがあれば以降を実行せずエラー一覧を返す",
             "M_CUSTOMER, M_PRODUCT", "—", "検証エラーを画面へ返す"],
            ["2", "得意先の取引停止フラグを確認し、1 の場合はエラーとする",
             "M_CUSTOMER", "—", "「取引停止中のため受注できません」を返す"],
            ["3", "納品希望日が受注日以降の営業日であることを営業日カレンダーで確認する",
             "M_CALENDAR", "—", "「受注日以降の営業日を指定してください」を返す"],
            ["4", "明細ごとに販売単価を決定する。得意先別単価マスタに適用日が有効な単価が"
             "あればそれを採用し、無ければ商品マスタの標準単価を採用する",
             "M_PRICE, M_PRODUCT", "—", "単価が取得できない場合は必須入力エラーを返す"],
            ["5", "明細金額（受注数量 × 販売単価、円未満切り捨て）を計算し、受注金額合計を求める",
             "—", "—", "—"],
            ["6", "消費税額を消費税計算部品で計算する。★消費税は請求単位で計算するため、"
             "受注時点では参考値として保持する（円未満四捨五入）",
             "M_PRODUCT, M_TAX_RATE", "—", "—"],
            ["7", "得意先の売掛残高と未請求の受注金額を集計し、今回受注金額を加えた額が"
             "与信限度額を超えるか判定する",
             "T_INVOICE, T_ORDER, M_CUSTOMER", "—", "外部の与信照会がタイムアウトした場合は"
             "与信保留として扱う"],
            ["8", "与信限度額を超える場合は受注ステータスを 20（与信保留）とし、"
             "超えない場合は 10（受付）とする",
             "—", "—", "—"],
            ["9", "受注番号を採番する（R + YYYYMMDD + 3 桁連番）",
             "—", "T_NUMBERING", "採番テーブルのロック待ちが 10 秒を超えた場合はエラーとする"],
            ["10", "受注ヘッダと受注明細を登録する。明細ステータスは 10（未引当）とする",
             "—", "T_ORDER, T_ORDER_DETAIL", "一意制約違反時はロールバックして排他エラーを返す"],
            ["11", "監査ログを出力する（操作種別＝受注登録、変更前＝なし、変更後＝登録内容）",
             "—", "T_AUDIT_LOG", "監査ログの出力に失敗した場合は本処理を異常終了させる"],
            ["12", "受注番号を画面へ返す。与信保留の場合は警告メッセージを併せて返す",
             "—", "—", "—"],
        ],
        checks=[
            ["1", "得意先コード", "必須", "未入力でないこと", "{0}を入力してください。"],
            ["2", "得意先コード", "存在", "得意先マスタに削除フラグ 0 で存在すること", "{0}を入力してください。"],
            ["3", "得意先コード", "業務", "取引停止フラグが 0 であること", "得意先「{0}」は取引停止中のため受注できません。"],
            ["4", "納品希望日", "必須", "未入力でないこと", "{0}を入力してください。"],
            ["5", "納品希望日", "業務", "受注日以降であり、かつ営業日であること", "納品希望日は受注日以降の営業日を指定してください。"],
            ["6", "商品コード", "必須", "明細行ごとに未入力でないこと", "{0}を入力してください。"],
            ["7", "商品コード", "存在", "商品マスタに削除フラグ 0 で存在すること", "{0}を入力してください。"],
            ["8", "受注数量", "必須", "未入力でないこと", "{0}を入力してください。"],
            ["9", "受注数量", "範囲", "0 より大きい数値であること", "受注数量は0より大きい値を入力してください。"],
            ["10", "受注数量", "書式", "整数部 7 桁・小数部 2 桁以内であること", "{0}は{1}以下で入力してください。"],
            ["11", "販売単価", "範囲", "0 以上の数値であること", "{0}は{1}以下で入力してください。"],
            ["12", "明細行数", "範囲", "1 行以上 50 行以内であること", "{0}は{1}以下で入力してください。"],
            ["13", "備考", "書式", "200 文字以内であること", "{0}は{1}以下で入力してください。"],
        ],
        errors=[
            ["1", "採番テーブルのロックが 10 秒以上取得できない", "処理を中断しロールバックする。"
             "システムエラー画面へ遷移させる", "（システムエラー画面）"],
            ["2", "与信照会 API がタイムアウトした", "与信保留（ステータス20）として"
             "受注を登録し、警告を表示する", "与信限度額を超えたため受注を保留にしました。"],
            ["3", "受注ヘッダの登録で一意制約に違反した", "ロールバックのうえ再採番して 1 回だけ"
             "再試行する。再度失敗した場合はエラーとする", "他の利用者が更新しました。再度読み込んでください。"],
            ["4", "監査ログの出力に失敗した", "トランザクション全体をロールバックし異常終了させる", "（システムエラー画面）"],
            ["5", "データベース接続が切断された", "システムエラー画面へ遷移させ、"
             "エラーIDを表示する", "（システムエラー画面）"],
        ],
        diagram=diagram,
        revisions=[
            ("1.0", "2025/10/31", "初版作成", "鈴木"),
            ("1.1", "2025/11/14", "引当を出荷指示時に行う方式へ変更", "鈴木"),
            ("1.2", "2025/11/28", "消費税を請求単位で計算する旨を追記", "鈴木"),
        ],
    )


# ── 処理仕様書（受注取消）─────────────────────────────────────────
def _proc_order_cancel(out: Path) -> Path:
    diagram = Diagram(
        sheet="3.処理フロー",
        nodes=[
            Node("start", "開始", 6, 8, 8, 2, shape="ellipse", fill="FFF2CC", line="BF8F00"),
            Node("read", "受注ヘッダ取得", 6, 12, 14, 3),
            Node("j0", "受注あり？", 6, 17, 12, 3, shape="diamond", fill="FCE4D6", line="C55A11"),
            Node("nf", "エラー返却\n（該当なし）", 24, 17, 14, 3, fill="F8CBAD", line="C00000"),
            Node("j1", "更新日時一致？", 6, 22, 14, 3, shape="diamond", fill="FCE4D6", line="C55A11"),
            Node("lock", "排他エラー\n（再読込を促す）", 24, 22, 14, 3, fill="F8CBAD", line="C00000"),
            Node("j2", "ステータスは\n40 より前？", 6, 27, 14, 4, shape="diamond",
                 fill="FCE4D6", line="C55A11"),
            Node("ng", "取消不可エラー", 24, 28, 14, 3, fill="F8CBAD", line="C00000"),
            Node("rel", "引当解放\nreleaseAllocation", 6, 33, 16, 3, fill="EDEDED", line="808080"),
            Node("upd_d", "受注明細を 90（取消）へ", 6, 38, 16, 3),
            Node("upd_h", "受注ヘッダを 90（取消）へ\n取消理由・取消日時を記録", 6, 43, 18, 4),
            Node("audit", "監査ログ出力\nAuditLogger", 6, 49, 14, 3, fill="EDEDED", line="808080"),
            Node("end", "終了", 6, 54, 8, 2, shape="ellipse", fill="FFF2CC", line="BF8F00"),
        ],
        edges=[
            Edge("start", "read"),
            Edge("read", "j0"),
            Edge("j0", "nf", "なし"),
            Edge("j0", "j1", "あり"),
            Edge("j1", "lock", "不一致"),
            Edge("j1", "j2", "一致"),
            Edge("j2", "ng", "40 以降"),
            Edge("j2", "rel", "40 より前"),
            Edge("rel", "upd_d"),
            Edge("upd_d", "upd_h"),
            Edge("upd_h", "audit"),
            Edge("audit", "end"),
        ],
    )
    return _proc_spec(
        out,
        rel=spec.path_of("ORD", "詳細設計", "処理仕様書_受注取消.xlsx"),
        doc_name="処理仕様書（受注取消）",
        sub="ORD",
        class_name="OrderCancelService",
        overview=[
            ("対象", [
                ("モジュール名", "受注取消サービス"),
                ("クラス名", "OrderCancelService"),
                ("関連する機能", "受注取消（基本設計の機能一覧）"),
                ("関連する画面", "受注取消"),
            ]),
            ("呼出関係", [
                ("呼出元", "OrderCancelController#cancel（受注取消画面の取消ボタン）"),
                ("呼出先", "StockAllocationService#releaseAllocation, AuditLogger"),
            ]),
            ("方式", [
                ("トランザクション", "受注の更新・明細の更新・引当の解放を 1 トランザクションで行う。"),
                ("排他制御", "楽観的排他。画面が保持する更新日時と受注ヘッダの更新日時が"
                        "一致しない場合は排他エラーとする。"),
            ]),
            ("処理の要点", [
                ("処理概要", "指定された受注の取消可否を判定し、取消可能であれば受注ヘッダと"
                        "受注明細のステータスを 90（取消）に更新し、引当済の在庫を解放する。"),
                ("取消可能な条件", "★受注ステータスが 40（出荷指示済）より前であること。"
                        "出荷指示を出した後は取消できない（要件定義の「翌営業日まで」は"
                        "運用の実態と合わないため、詳細設計ではステータスで判定する）。"),
            ]),
        ],
        steps=[
            ["1", "受注番号で受注ヘッダを取得する。存在しない場合はエラーとする",
             "T_ORDER", "—", "必須入力エラーを返す"],
            ["2", "画面が保持する更新日時と受注ヘッダの更新日時を比較する",
             "T_ORDER", "—", "不一致の場合は排他エラーを返す"],
            ["3", "受注ステータスを判定する。40（出荷指示済）または 90（取消）の場合は"
             "取消できないものとする",
             "T_ORDER", "—", "「取消可能期限を過ぎているため取り消せません」を返す"],
            ["4", "取消理由が「その他」の場合、取消理由備考が入力されていることを確認する",
             "—", "—", "必須入力エラーを返す"],
            ["5", "受注明細ごとに引当を解放する。引当済数量を在庫の引当済数量から減算し、"
             "引当テーブルの該当行を削除する",
             "T_ALLOCATION", "T_STOCK, T_ALLOCATION", "在庫行のロック待ちが 10 秒を"
             "超えた場合はロールバックする"],
            ["6", "受注明細のステータスを 90（取消）に更新する",
             "—", "T_ORDER_DETAIL", "—"],
            ["7", "受注ヘッダのステータスを 90（取消）に更新し、取消理由と取消日時を記録する",
             "—", "T_ORDER", "—"],
            ["8", "監査ログを出力する（操作種別＝受注取消、変更前＝取消前のステータス）",
             "—", "T_AUDIT_LOG", "出力に失敗した場合は異常終了させる"],
        ],
        checks=[
            ["1", "受注番号", "必須", "未入力でないこと", "{0}を入力してください。"],
            ["2", "受注番号", "存在", "受注ヘッダに存在すること", "{0}を入力してください。"],
            ["3", "受注ステータス", "業務", "40（出荷指示済）および 90（取消）でないこと",
             "取消可能期限を過ぎているため取り消せません。"],
            ["4", "取消理由", "必須", "未選択でないこと", "{0}を入力してください。"],
            ["5", "取消理由備考", "条件必須", "取消理由が「その他」の場合は入力されていること", "{0}を入力してください。"],
            ["6", "更新日時", "排他", "画面が保持する値と一致すること", "他の利用者が更新しました。再度読み込んでください。"],
        ],
        errors=[
            ["1", "取消の対象となる受注が他の利用者に更新された", "排他エラーとして処理を中断し、"
             "再読込を促す", "他の利用者が更新しました。再度読み込んでください。"],
            ["2", "引当の解放中に在庫行のロックが取得できない", "ロールバックして再実行を促す", "（システムエラー画面）"],
            ["3", "既に出荷実績を受信済みの受注が指定された", "取消不可としてエラーを返す",
             "取消可能期限を過ぎているため取り消せません。"],
            ["4", "EDI 由来の受注が指定された", "取消は可能とするが、取引先への取消連絡は"
             "システムでは行わないため、画面に注意を表示する", "（画面に注意文を表示）"],
        ],
        diagram=diagram,
        revisions=[
            ("1.0", "2025/10/31", "初版作成", "鈴木"),
            ("1.1", "2025/11/28", "取消可否をステータスで判定する方式に統一", "鈴木"),
        ],
    )


# ── 処理仕様書（在庫引当）─────────────────────────────────────────
def _proc_allocation(out: Path) -> Path:
    diagram = Diagram(
        sheet="3.処理フロー",
        nodes=[
            Node("start", "開始", 6, 8, 8, 2, shape="ellipse", fill="FFF2CC", line="BF8F00"),
            Node("read", "受注明細取得\n（未引当のもの）", 6, 12, 15, 3),
            Node("sort", "商品コード昇順に並替\n（デッドロック回避）", 6, 17, 17, 3),
            Node("loop", "明細ごとに繰返し", 6, 22, 15, 3, fill="E2EFDA", line="548235"),
            Node("find", "引当可能在庫検索\nfindAllocatableStock\n（入庫日の古い順）", 6, 27, 16, 4),
            Node("j1", "在庫あり？", 6, 33, 12, 3, shape="diamond", fill="FCE4D6", line="C55A11"),
            Node("short", "不足として記録\n（不足数を返す）", 26, 33, 15, 3, fill="F8CBAD", line="C00000"),
            Node("j2", "全数確保？", 6, 39, 12, 3, shape="diamond", fill="FCE4D6", line="C55A11"),
            Node("part", "明細ステータス\n= 20（一部引当）", 26, 39, 14, 3, fill="FFF2CC", line="BF8F00"),
            Node("full", "明細ステータス\n= 30（引当済）", 6, 45, 14, 3),
            Node("upd", "在庫更新\n引当済数量 += 確保数", 6, 50, 16, 3),
            Node("ins", "引当登録\nT_ALLOCATION", 6, 55, 14, 3),
            Node("head", "全明細が引当済なら\n受注ヘッダを 30 へ", 6, 60, 17, 3),
            Node("end", "終了", 6, 65, 8, 2, shape="ellipse", fill="FFF2CC", line="BF8F00"),
        ],
        edges=[
            Edge("start", "read"),
            Edge("read", "sort"),
            Edge("sort", "loop"),
            Edge("loop", "find"),
            Edge("find", "j1"),
            Edge("j1", "short", "なし"),
            Edge("j1", "j2", "あり"),
            Edge("j2", "part", "一部"),
            Edge("j2", "full", "全数"),
            Edge("part", "upd"),
            Edge("full", "upd"),
            Edge("upd", "ins"),
            Edge("ins", "head"),
            Edge("head", "end"),
        ],
    )
    return _proc_spec(
        out,
        rel=spec.path_of("INV", "詳細設計", "処理仕様書_在庫引当.xlsx"),
        doc_name="処理仕様書（在庫引当）",
        sub="INV",
        class_name="StockAllocationService",
        overview=[
            ("対象", [
                ("モジュール名", "在庫引当サービス"),
                ("クラス名", "StockAllocationService"),
                ("関連する機能", "在庫引当（基本設計の機能一覧）"),
                ("関連する画面", "—（画面を持たない。出荷指示から呼ばれる）"),
            ]),
            ("呼出関係", [
                ("呼出元", "★ShipmentInstructionService#createInstruction（出荷指示の実行時）。"
                        "受注登録時には呼び出さない。"),
                ("呼出先", "AuditLogger"),
            ]),
            ("方式", [
                ("トランザクション", "1 受注ぶんの引当をまとめて 1 トランザクションとする。"
                            "1 明細でも引当に失敗した場合は当該受注ぶんをロールバックする。"),
                ("排他制御", "在庫行を SELECT FOR UPDATE で悲観的にロックする。"
                        "デッドロックを避けるため、商品コードの昇順にロックを取得する。"),
            ]),
            ("処理の要点", [
                ("処理概要", "受注明細に対して有効在庫から数量を確保し、在庫の引当済数量と"
                        "引当テーブルを更新する。有効在庫が不足する場合は確保できた数量までを"
                        "引き当て、明細ステータスを一部引当とする。"),
                ("引当の順序", "同一商品に複数のロットがある場合、入庫日の古いロットから順に"
                        "引き当てる（先入先出）。入庫日が同じ場合はロット番号の昇順とする。"),
                ("引当のタイミング", "★出荷指示の実行時に引き当てる。基本設計の「受注確定時」から"
                            "変更した（課題管理表の「在庫引当のタイミング」を参照）。"),
            ]),
        ],
        steps=[
            ["1", "対象の受注に紐づく受注明細のうち、ステータスが 10（未引当）または"
             "20（一部引当）のものを取得する",
             "T_ORDER_DETAIL", "—", "対象が 0 件の場合は正常終了する"],
            ["2", "明細を商品コードの昇順に並べ替える（デッドロック回避のため）",
             "—", "—", "—"],
            ["3", "明細ごとに、対象倉庫の在庫行を SELECT FOR UPDATE で取得する",
             "T_STOCK", "—", "ロック待ちが 10 秒を超えた場合はロールバックする"],
            ["4", "有効在庫数（実在庫数 − 引当済数量）を求め、引当可能な数量を決定する。"
             "有効在庫が受注数量以上であれば全数、不足していれば有効在庫の数量までとする",
             "T_STOCK", "—", "有効在庫が 0 の場合は不足として記録する"],
            ["5", "在庫の引当済数量に確保した数量を加算する。加算後の引当済数量が"
             "実在庫数を超える更新は行わない",
             "—", "T_STOCK", "超える場合は異常としてロールバックする"],
            ["6", "引当テーブルに、受注番号・明細番号・倉庫・商品・確保数量・引当日時を登録する",
             "—", "T_ALLOCATION", "—"],
            ["7", "受注明細の引当済数量を更新し、全数確保できた場合はステータスを 30（引当済）、"
             "一部の場合は 20（一部引当）とする",
             "—", "T_ORDER_DETAIL", "—"],
            ["8", "すべての明細が 30（引当済）になった場合、受注ヘッダのステータスを"
             "30（引当済）に更新する",
             "T_ORDER_DETAIL", "T_ORDER", "—"],
            ["9", "不足が発生した明細について、商品コードと不足数を戻り値に含めて呼出元へ返す",
             "—", "—", "—"],
        ],
        checks=[
            ["1", "受注番号", "必須", "未入力でないこと", "{0}を入力してください。"],
            ["2", "受注ステータス", "業務", "90（取消）でないこと", "取消可能期限を過ぎているため取り消せません。"],
            ["3", "倉庫コード", "存在", "倉庫マスタに存在すること", "{0}を入力してください。"],
            ["4", "引当数量", "範囲", "0 より大きく、有効在庫数以下であること",
             "有効在庫が不足しています。（商品:{0} 不足数:{1}）"],
            ["5", "引当済数量", "整合", "更新後の引当済数量が実在庫数を超えないこと", "（システムエラー画面）"],
        ],
        errors=[
            ["1", "有効在庫が不足している", "確保できた数量までを引き当て、明細ステータスを"
             "20（一部引当）とする。不足数を呼出元へ返す", "引当できたのは{0}のうち{1}です。"],
            ["2", "有効在庫が 0 である", "当該明細は引き当てず、ステータスを 10（未引当）の"
             "まま残す", "有効在庫が不足しています。（商品:{0} 不足数:{1}）"],
            ["3", "在庫行のロックが 10 秒以上取得できない", "当該受注ぶんをロールバックし、"
             "呼出元へ再実行を促す", "（システムエラー画面）"],
            ["4", "デッドロックが検知された", "1 回だけ自動で再試行する。再度失敗した場合は"
             "異常終了させる", "（システムエラー画面）"],
            ["5", "在庫行が存在しない（当該倉庫にその商品の在庫レコードが無い）",
             "引当済数量 0 の在庫行を作成せず、不足として扱う",
             "有効在庫が不足しています。（商品:{0} 不足数:{1}）"],
        ],
        diagram=diagram,
        revisions=[
            ("1.0", "2025/10/31", "初版作成", "鈴木"),
            ("1.1", "2025/11/14", "引当の呼出元を受注登録から出荷指示へ変更", "鈴木"),
        ],
    )


# ── 処理仕様書（請求締め）─────────────────────────────────────────
def _proc_billing_close(out: Path) -> Path:
    diagram = Diagram(
        sheet="3.処理フロー",
        nodes=[
            Node("start", "開始（21:00）", 6, 8, 12, 2, shape="ellipse", fill="FFF2CC", line="BF8F00"),
            Node("cal", "業務日付取得\n締め日か判定", 6, 12, 15, 3),
            Node("target", "締め対象得意先抽出\n（締日区分で判定）", 6, 17, 18, 3),
            Node("dup", "締め済を除外", 6, 22, 14, 3),
            Node("loop", "得意先ごとに繰返し\n（4 並列）", 6, 27, 16, 3, fill="E2EFDA", line="548235"),
            Node("agg", "売上集計\naggregateSales", 6, 32, 14, 3),
            Node("tax", "消費税計算\nTaxCalculator\n（請求単位・四捨五入）", 6, 37, 16, 4),
            Node("prev", "前回請求残高取得", 6, 43, 15, 3),
            Node("calc", "請求金額算出\n前残+売上+税-入金", 6, 48, 16, 3),
            Node("j1", "金額 = 0？", 6, 53, 12, 3, shape="diamond", fill="FCE4D6", line="C55A11"),
            Node("skip", "請求を作成しない\n（対象外として記録）", 26, 53, 16, 3, fill="EDEDED", line="808080"),
            Node("ins", "請求ヘッダ・明細登録\nT_INVOICE", 6, 59, 16, 3),
            Node("back", "受注明細へ請求番号を\n書き戻す（二重締め防止）", 6, 64, 18, 3),
            Node("commit", "得意先単位でコミット", 6, 69, 16, 3),
            Node("end", "終了（結果をログ出力）", 6, 74, 16, 2, shape="ellipse",
                 fill="FFF2CC", line="BF8F00"),
        ],
        edges=[
            Edge("start", "cal"),
            Edge("cal", "target"),
            Edge("target", "dup"),
            Edge("dup", "loop"),
            Edge("loop", "agg"),
            Edge("agg", "tax"),
            Edge("tax", "prev"),
            Edge("prev", "calc"),
            Edge("calc", "j1"),
            Edge("j1", "skip", "0"),
            Edge("j1", "ins", "0以外"),
            Edge("ins", "back"),
            Edge("back", "commit"),
            Edge("commit", "end"),
        ],
    )
    return _proc_spec(
        out,
        rel=spec.path_of("BIL", "詳細設計", "処理仕様書_請求締め.xlsx"),
        doc_name="処理仕様書（請求締め）",
        sub="BIL",
        class_name="BillingCloseBatch",
        overview=[
            ("対象", [
                ("モジュール名", "請求締めバッチ"),
                ("クラス名", "BillingCloseBatch"),
                ("関連する機能", "請求締め（基本設計の機能一覧）"),
                ("関連する画面", "請求締め処理"),
            ]),
            ("呼出関係", [
                ("呼出元", "ジョブ管理（請求締めジョブ）および請求締め処理画面"),
                ("呼出先", "TaxCalculator, NumberingService, BusinessDayCalendar, AuditLogger"),
            ]),
            ("方式", [
                ("トランザクション", "得意先単位でコミットする。1 得意先で異常が発生した場合は"
                            "当該得意先のみロールバックし、次の得意先の処理を続ける。"),
                ("並列度", "得意先を 4 分割して並列に処理する。"),
                ("処理時間", "得意先 3,000 件・明細 40 万件を想定し、2 時間以内の完了を見込む。"),
            ]),
            ("処理の要点", [
                ("処理概要", "締め対象の得意先ごとに締め期間の売上を集計し、消費税と前回請求残高を"
                        "加味して請求金額を確定する。"),
                ("締め日の判定", "★毎日 21:00 に起動し、得意先マスタの締日区分に従って"
                        "当日が締め日にあたる得意先だけを対象とする（1=20日締め、2=末日締め）。"
                        "20 日および月末が休日にあたる場合は前営業日に締める。"),
                ("消費税の計算単位", "★消費税は請求単位で「税抜合計 × 税率」により計算し、"
                            "円未満は四捨五入する。税率が混在する場合は税率ごとに集計してから計算する。"),
            ]),
        ],
        steps=[
            ["1", "業務日付を取得し、締日区分ごとに当日が締め日にあたるかを判定する。"
             "20 日・月末が休日の場合は前営業日を締め日とする",
             "M_CALENDAR", "—", "業務日付が取得できない場合は異常終了する"],
            ["2", "締め日にあたる得意先を抽出する。画面から得意先の範囲が指定された場合は"
             "その範囲に絞る",
             "M_CUSTOMER", "—", "対象が 0 件の場合は正常終了する"],
            ["3", "既に同一の請求年月で締め済みの得意先を対象から除外する",
             "T_INVOICE", "—", "—"],
            ["4", "得意先ごとに、締め期間内に売上計上された受注明細を集計する。"
             "締め期間は前回の締め日の翌日から今回の締め日までとする",
             "T_ORDER, T_ORDER_DETAIL, T_SHIPMENT", "—", "集計対象が 0 件でも処理を続ける"],
            ["5", "税率ごとに税抜金額を集計し、税率ごとに消費税額を計算して合算する"
             "（請求単位・円未満四捨五入）",
             "M_PRODUCT, M_TAX_RATE", "—", "税区分が不正な商品がある場合は当該得意先をエラーとする"],
            ["6", "前回の請求ヘッダから未回収の残高を取得する。初回の場合は 0 とする",
             "T_INVOICE, T_DEPOSIT", "—", "—"],
            ["7", "請求金額を算出する（前回請求残高 + 当月売上額 + 消費税額 − 当月入金額）",
             "—", "—", "—"],
            ["8", "請求金額および当月売上額がいずれも 0 の得意先は請求を作成せず、"
             "対象外として実行ログに記録する",
             "—", "—", "—"],
            ["9", "請求番号を採番し、請求ヘッダと請求明細を登録する。"
             "請求ステータスは 10（締め済）とする",
             "—", "T_NUMBERING, T_INVOICE, T_INVOICE_DETAIL", "一意制約違反時は"
             "当該得意先をロールバックしエラー件数に計上する"],
            ["10", "対象となった受注明細に請求番号を書き戻し、二重の締めを防ぐ",
             "—", "T_ORDER_DETAIL", "—"],
            ["11", "得意先単位でコミットする",
             "—", "—", "コミットに失敗した場合は当該得意先をエラー件数に計上する"],
            ["12", "確定件数・エラー件数・対象外件数を実行ログに出力し、"
             "エラーがある場合はジョブを警告終了とする",
             "—", "T_BATCH_LOG", "—"],
        ],
        checks=[
            ["1", "請求年月", "必須", "未入力でないこと", "{0}を入力してください。"],
            ["2", "請求年月", "書式", "YYYYMM 形式であること", "{0}は{1}以下で入力してください。"],
            ["3", "請求年月", "業務", "未来の年月でないこと", "{0}は{1}以下で入力してください。"],
            ["4", "締日区分", "存在", "コード定義書の締日区分に定める値であること", "{0}は{1}以下で入力してください。"],
            ["5", "得意先コード（自）（至）", "整合", "（自）が（至）以下であること", "{0}は{1}以下で入力してください。"],
        ],
        errors=[
            ["1", "税区分が不正な商品が売上明細に含まれる", "当該得意先をロールバックし、"
             "エラー件数に計上して次の得意先へ進む", "（実行ログに記録）"],
            ["2", "請求番号の採番で一意制約に違反した", "1 回だけ再採番して再試行する。"
             "再度失敗した場合は当該得意先をエラーとする", "他の利用者が更新しました。再度読み込んでください。"],
            ["3", "同一の請求年月で既に締め済みの得意先が指定された", "対象から除外し、"
             "実行ログに記録する", "（実行ログに記録）"],
            ["4", "バッチが多重に起動された", "実行管理テーブルで検知し、後から起動された"
             "ジョブを即時に異常終了させる", "（運用監視へ通知）"],
            ["5", "処理時間が 3 時間を超えた", "運用監視から警告を通知する。"
             "処理そのものは継続する", "（運用監視へ通知）"],
        ],
        diagram=diagram,
        revisions=[
            ("1.0", "2025/10/31", "初版作成", "高橋"),
            ("1.1", "2025/11/14", "締め日を得意先マスタの締日区分で判定する方式へ変更", "高橋"),
            ("1.2", "2025/11/28", "消費税を請求単位・四捨五入で計算する旨を明記", "高橋"),
        ],
        extra=[
            (
                "7.締め期間の例",
                ["締日区分", "締め実行日", "締め期間（自）", "締め期間（至）", "支払期日の例"],
                [
                    ["1（20日締め）", "2026/01/20", "2025/12/21", "2026/01/20", "2026/02/28"],
                    ["1（20日締め）", "2026/02/20", "2026/01/21", "2026/02/20", "2026/03/31"],
                    ["2（末日締め）", "2026/01/30", "2026/01/01", "2026/01/31", "2026/03/10"],
                    ["2（末日締め）", "2026/02/27", "2026/02/01", "2026/02/28", "2026/04/10"],
                ],
                [16, 16, 18, 18, 20],
                [("", 1), ("", 1), ("締め期間", 2), ("", 1)],
            )
        ],
    )


# ── バッチ処理仕様書 ──────────────────────────────────────────────
def _batch_spec(out: Path) -> Path:
    info = DocInfo(
        doc_name="バッチ処理仕様書",
        subsystem="全体",
        version="1.1",
        date="2025/11/28",
        revisions=REV,
    )

    def body(wb, info):
        ws = xk.add_sheet(wb, "1.ジョブ構成")
        r = xk.heading(ws, 2, 2, "1. ジョブの構成（Spring Batch）")
        r = xk.table(
            ws,
            top=r,
            left=2,
            caption="ジョブとステップの構成",
            groups=[("", 1), ("ステップ", 2), ("", 1), ("実行制御", 2)],
            header=["ジョブ名", "ステップ", "種別", "処理内容", "コミット間隔", "多重度"],
            rows=[
                ["EDI受注取込", "step1-read", "Chunk", "EDI受信ワークから未処理レコードを読む", "1,000 件", "1"],
                ["EDI受注取込", "step2-convert", "Chunk", "流通BMS のレコードを受注データへ変換する", "1,000 件", "1"],
                ["EDI受注取込", "step3-regist", "Chunk", "受注として登録し、結果をワークへ書き戻す", "100 件", "1"],
                ["出荷実績取込", "step1-fetch", "Tasklet", "WMS の REST API から出荷実績を取得する", "—", "1"],
                ["出荷実績取込", "step2-apply", "Chunk", "在庫を引き落とし売上を計上する", "500 件", "1"],
                ["入金データ取込", "step1-load", "Chunk", "全銀フォーマットのファイルを読み込む", "1,000 件", "1"],
                ["入金データ取込", "step2-match", "Chunk", "請求番号一致による自動消込を行う", "500 件", "1"],
                ["請求締め", "step1-target", "Tasklet", "締め対象の得意先を抽出する", "—", "1"],
                ["請求締め", "step2-close", "Partition", "得意先を 4 分割して並列に締める", "1 得意先", "4"],
                ["売上仕訳連携", "step1-export", "Chunk", "仕訳データを固定長ファイルへ出力する", "5,000 件", "1"],
                ["監査ログ退避", "step1-archive", "Chunk", "13 か月超の監査ログを退避する", "10,000 件", "1"],
            ],
            widths=[20, 20, 12, 60, 16, 10],
            merge_cols=(0,),
            center_cols=(2, 4, 5),
        )
        xk.set_print(ws, doc_name=info.doc_name)

        ws_r = xk.add_sheet(wb, "2.リスタート方式")
        r = xk.heading(ws_r, 2, 2, "2. リスタートの方式")
        xk.kv_group_table(
            ws_r,
            top=r,
            left=2,
            groups=[
                ("再実行", [
                    ("再実行の単位", "ジョブ単位で再実行する。Spring Batch のジョブリポジトリに"
                            "実行状態を保持し、失敗したステップから再開する。"),
                    ("冪等性", "処理済みのレコードには処理済フラグを立て、再実行時に読み飛ばす。"
                        "請求締めは請求年月と得意先の組で二重登録を防ぐ。"),
                ]),
                ("制御", [
                    ("多重起動の防止", "実行管理テーブルにジョブ名と開始日時を登録し、"
                                "実行中のジョブが存在する場合は即時に異常終了させる。"),
                    ("打ち切り", "1 ジョブの実行時間が 4 時間を超えた場合、運用監視から通知する。"
                        "自動での打ち切りは行わない。"),
                    ("エラーの許容", "取込系のジョブはエラー件数が全体の 10% を超えた時点で"
                            "処理を打ち切り、異常終了とする。"),
                ]),
            ],
            width=94,
        )
        xk.set_print(ws_r, doc_name=info.doc_name)

        ws2 = xk.add_sheet(wb, "3.入出力ファイル")
        r = xk.heading(ws2, 2, 2, "3. 入出力ファイル")
        r = xk.table(
            ws2,
            top=r,
            left=2,
            caption="入出力ファイル",
            groups=[("", 1), ("", 1), ("ファイル", 3), ("保管", 2)],
            header=["ジョブ名", "区分", "ファイル名", "形式", "文字コード", "配置先", "保管期間"],
            rows=[
                ["EDI受注取込", "入力", "BMS_ORDER_YYYYMMDDHH.xml", "流通BMS(XML)", "UTF-8", "/data/edi/in", "3 か月"],
                ["EDI受注取込", "出力", "BMS_ORDER_ERR_YYYYMMDD.csv", "CSV", "UTF-8", "/data/edi/err", "1 年"],
                ["入金データ取込", "入力", "ZENGIN_YYYYMMDD.txt", "全銀協フォーマット", "Shift_JIS", "/data/bank/in", "7 年"],
                ["売上仕訳連携", "出力", "JOURNAL_YYYYMMDD.dat", "固定長", "Shift_JIS", "/data/acct/out", "7 年"],
                ["マスタ連携取込", "入力", "STAFF_YYYYMMDD.csv", "CSV", "UTF-8", "/data/hr/in", "1 年"],
                ["監査ログ退避", "出力", "AUDIT_YYYYMM.csv.gz", "CSV(gzip)", "UTF-8", "/archive/audit", "5 年"],
            ],
            widths=[20, 8, 34, 22, 14, 22, 12],
            merge_cols=(0,),
            center_cols=(1, 4, 6),
        )
        r = xk.note(
            ws2, r, 2,
            "※ 入力ファイルの文字コードは相手システムの仕様に従う。"
            "取込時に UTF-8 へ変換する（全銀および会計は Shift_JIS のまま授受する）。",
        )
        xk.set_print(ws2, doc_name=info.doc_name)

    return xk.build(out, "00_全体/03_詳細設計/バッチ処理仕様書.xlsx", info, body)


# ── 共通部品仕様書 ────────────────────────────────────────────────
def _common_parts(out: Path) -> Path:
    info = DocInfo(
        doc_name="共通部品仕様書",
        subsystem="共通基盤",
        version="1.1",
        date="2025/11/28",
        revisions=REV,
    )

    def body(wb, info):
        ws = xk.add_sheet(wb, "共通部品一覧")
        r = xk.heading(ws, 2, 2, "共通部品一覧")
        r = xk.table(
            ws,
            top=r,
            left=2,
            groups=[("部品", 2), ("", 1), ("", 1)],
            header=["部品名", "クラス名", "提供する機能", "利用元"],
            rows=[
                ["監査ログ出力部品", "AuditLogger",
                 "更新操作の実施者・日時・変更前後の値を監査ログテーブルへ記録する",
                 "受注・在庫・請求の各サービス"],
                ["採番部品", "NumberingService",
                 "業務キーを採番テーブルから排他制御つきで発番し、書式を適用する",
                 "受注登録・請求締め・出荷指示・棚卸"],
                ["消費税計算部品", "TaxCalculator",
                 "税区分から税率を引き、消費税額を計算して端数処理を適用する",
                 "受注登録・請求締め・請求書発行"],
                ["営業日カレンダー部品", "BusinessDayCalendar",
                 "会社カレンダーから営業日を判定し、翌営業日・N営業日後を求める",
                 "受注登録・受注取消・請求締め"],
                ["認証サービス", "AuthService",
                 "社員コードとパスワードを照合し、セッションと権限情報を発行する",
                 "全画面（認証フィルタ）"],
            ],
            widths=[26, 26, 62, 40],
        )
        xk.set_print(ws, doc_name=info.doc_name)

        ws2 = xk.add_sheet(wb, "消費税計算部品")
        r = xk.heading(ws2, 2, 2, "消費税計算部品（TaxCalculator）")
        r = xk.kv_group_table(
            ws2,
            top=r,
            left=2,
            groups=[
                ("責務", [
                    ("責務", "税区分に対応する税率を取得し、税抜金額から消費税額を計算する。"),
                    ("計算の単位", "本部品は与えられた金額に対して計算するだけであり、"
                            "明細単位か請求単位かは呼出元が決める。"),
                ]),
                ("方式", [
                    ("税率の取得元", "税率マスタ（M_TAX_RATE）から適用日で有効な税率を取得する。"
                            "プログラムに税率を埋め込まない。"),
                    ("端数処理", "★呼出元が端数処理の方式を指定する。指定がない場合は四捨五入とする。"
                            "受注登録は切り捨て、請求締めは四捨五入を指定する。"),
                    ("スレッド安全性", "状態を持たないため、シングルトンで共有してよい。"),
                ]),
            ],
            width=94,
        )
        xk.set_print(ws2, doc_name=info.doc_name)

        # メソッド仕様と税率は B〜E の列幅を共有できるので 1 シートに積む。
        ws2b = xk.add_sheet(wb, "消費税計算部品 メソッド")
        r = xk.heading(ws2b, 2, 2, "メソッド仕様（TaxCalculator）")
        r = xk.table(
            ws2b,
            top=r,
            left=2,
            groups=[("", 1), ("入出力", 2), ("", 1)],
            header=["メソッド", "引数", "戻り値", "処理内容"],
            rows=[
                ["calcTax", "amount: 税抜金額, taxType: 税区分", "消費税額",
                 "税率マスタから税率を取得し、金額 × 税率を計算して既定の端数処理を適用する"],
                ["calcTax", "amount, taxType, rounding: 端数処理方式", "消費税額",
                 "端数処理方式（切り捨て／切り上げ／四捨五入）を指定して計算する"],
                ["getRate", "taxType: 税区分, date: 適用日", "税率",
                 "適用日に有効な税率を返す。該当がない場合は例外を送出する"],
            ],
            widths=[16, 40, 16, 76],
        )
        r = xk.heading(ws2b, r, 2, "税率")
        xk.table(
            ws2b,
            top=r,
            left=2,
            caption="税率（税率マスタの初期値）",
            groups=[("", 1), ("", 1), ("適用", 2)],
            header=["税区分", "名称", "税率", "適用開始日"],
            rows=[
                ["1", "標準税率", "10%", "2019/10/01"],
                ["2", "軽減税率", "8%", "2019/10/01"],
                ["9", "非課税", "0%", "—"],
            ],
            widths=[16, 40, 16, 76],
            center_cols=(0, 2, 3),
            header_fill=xk.FILL_HEAD2,
        )
        xk.set_print(ws2b, doc_name=info.doc_name)

        ws3 = xk.add_sheet(wb, "採番部品")
        r = xk.heading(ws3, 2, 2, "採番部品（NumberingService）")
        r = xk.kv_group_table(
            ws3,
            top=r,
            left=2,
            groups=[
                ("責務", [
                    ("責務", "採番区分ごとの連番を排他制御つきで取得し、書式を適用して返す。"),
                ]),
                ("方式", [
                    ("排他制御", "採番テーブルの該当行を SELECT FOR UPDATE でロックして加算する。"
                            "ロック待ちの上限は 10 秒とする。"),
                    ("トランザクション", "呼出元のトランザクションに参加する。"
                                "採番だけを別トランザクションにはしない（番号の欠番を許容する）。"),
                    ("リセット", "日次でリセットする採番区分は、日付が変わった時点で連番を 1 に戻す。"),
                ]),
            ],
            width=94,
        )
        xk.set_print(ws3, doc_name=info.doc_name)

        ws3b = xk.add_sheet(wb, "採番部品 採番区分")
        r = xk.heading(ws3b, 2, 2, "採番区分（NumberingService）")
        xk.table(
            ws3b,
            top=r,
            left=2,
            groups=[("", 1), ("", 1), ("採番規則", 2), ("", 1)],
            header=["採番区分", "対象", "書式", "リセット", "桁あふれ時の動作"],
            rows=[
                ["ORDER", "受注番号", "R + YYYYMMDD + 3 桁連番", "日次", "999 を超えたら例外を送出する"],
                ["SHIPMENT", "出荷指示番号", "S + YYYYMMDD + 3 桁連番", "日次", "999 を超えたら例外を送出する"],
                ["INVOICE", "請求番号", "B + YYYYMM + 5 桁連番", "月次", "99999 を超えたら例外を送出する"],
                ["COUNT", "棚卸番号", "C + YYYYMMDD + 3 桁連番", "日次", "999 を超えたら例外を送出する"],
                ["DEPOSIT", "入金番号", "D + YYYYMMDD + 4 桁連番", "日次", "9999 を超えたら例外を送出する"],
            ],
            widths=[14, 20, 30, 12, 44],
            center_cols=(0, 3),
            header_fill=xk.FILL_HEAD2,
        )
        xk.set_print(ws3b, doc_name=info.doc_name)

        ws4 = xk.add_sheet(wb, "営業日カレンダー部品")
        r = xk.heading(ws4, 2, 2, "営業日カレンダー部品（BusinessDayCalendar）")
        r = xk.kv_group_table(
            ws4,
            top=r,
            left=2,
            groups=[
                ("責務", [
                    ("責務", "会社カレンダー（M_CALENDAR）を参照し、営業日の判定と算出を行う。"),
                    ("営業日の定義", "土日祝日および会社が定める休日を除いた日を営業日とする。"
                            "カレンダーは年度単位で登録し、翌年度分は 2 月末までに登録する。"),
                ]),
                ("方式", [
                    ("キャッシュ", "起動時に当年度と翌年度のカレンダーを読み込み、"
                            "アプリケーション内にキャッシュする。更新時は再起動を要する。"),
                    ("未登録年度の扱い", "カレンダーが未登録の日付を指定された場合は例外を送出する。"
                                "土日の判定で代替しない（休日を取りこぼすため）。"),
                ]),
            ],
            width=94,
        )
        xk.set_print(ws4, doc_name=info.doc_name)

        ws4b = xk.add_sheet(wb, "営業日カレンダー部品 メソッド")
        r = xk.heading(ws4b, 2, 2, "メソッド仕様（BusinessDayCalendar）")
        xk.table(
            ws4b,
            top=r,
            left=2,
            groups=[("", 1), ("入出力", 2), ("", 1)],
            header=["メソッド", "引数", "戻り値", "処理内容"],
            rows=[
                ["isBusinessDay", "date: 判定する日付", "真偽値", "指定日が営業日かどうかを返す"],
                ["nextBusinessDay", "base: 基準日", "日付", "基準日の翌営業日を返す"],
                ["addBusinessDays", "base: 基準日, days: 日数", "日付", "基準日から N 営業日後を返す"],
                ["previousBusinessDay", "base: 基準日", "日付", "基準日の前営業日を返す。締め日が休日の場合に使う"],
            ],
            widths=[22, 34, 14, 76],
        )
        xk.set_print(ws4b, doc_name=info.doc_name)

    return xk.build(out, "00_全体/03_詳細設計/共通部品仕様書.xlsx", info, body)


# ── メッセージ定義書 ──────────────────────────────────────────────
def _message_def(out: Path) -> Path:
    info = DocInfo(
        doc_name="メッセージ定義書",
        subsystem="全体",
        version="1.0",
        date="2025/11/28",
        revisions=[("1.0", "2025/11/28", "初版作成", "山田")],
    )

    def body(wb, info):
        ws = xk.add_sheet(wb, "メッセージ一覧")
        r = xk.heading(ws, 2, 2, "メッセージ一覧")
        r = xk.table(
            ws,
            top=r,
            left=2,
            groups=[("", 1), ("", 1), ("出力条件", 1)],
            header=["種別", "メッセージ本文", "出力条件"],
            rows=[[kind, text, cond] for kind, text, cond in spec.MESSAGES],
            widths=[10, 60, 66],
            merge_cols=(0,),
            center_cols=(0,),
        )
        xk.note(ws, r, 2, "※ {0} {1} は実行時に値を埋め込む箇所を表す。")
        xk.set_print(ws, doc_name=info.doc_name)

        ws2 = xk.add_sheet(wb, "表示方法")
        r = xk.heading(ws2, 2, 2, "表示方法")
        xk.table(
            ws2,
            top=r,
            left=2,
            caption="種別ごとの表示方法",
            groups=[("", 1), ("表示", 2), ("", 1)],
            header=["種別", "表示位置", "色", "処理の継続"],
            rows=[
                ["エラー", "画面上部のメッセージ領域および該当項目の直下", "赤", "処理を中断する"],
                ["警告", "画面上部のメッセージ領域", "橙", "利用者の確認を経て処理を続ける"],
                ["情報", "画面上部のメッセージ領域", "青", "処理を続ける"],
            ],
            widths=[10, 48, 10, 30],
            center_cols=(0, 2),
            header_fill=xk.FILL_HEAD2,
        )
        xk.set_print(ws2, doc_name=info.doc_name)

    return xk.build(out, "00_全体/03_詳細設計/メッセージ定義書.xlsx", info, body)
