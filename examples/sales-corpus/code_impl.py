"""新販売管理システムの**実装コード**（Java）を生成する。

`資料/` の設計書と対になる成果物で、工程レイヤは詳細設計(3)のさらに下流
＝ソースコード(4)。

## ビルドできること

利用者が実際に `javac` でビルドし `java` で動作確認できる、**自己完結した
プロジェクト**として出す。そのための決めごとが 2 つある。

1. **外部依存を持たない。** 基本設計 2.1 のソフトウェア構成は Spring Boot だが、
   Spring に依存させると Maven/Gradle とネットワークが必要になり「配るだけで
   動かせる」が崩れる。そこで `@Service` `@Component` `@Transactional` は
   `framework` パッケージに**同名の注釈を自前で宣言**して使う。注釈の書き方は
   本番と同じ形で残るので、資材としての見た目も保てる。
2. **DB を要らなくする。** リポジトリはインターフェースにし、`demo` パッケージ
   でメモリ上の実装を与える。`demo.Main` が受注登録から請求締めまでを通し、
   仕込んだ食い違い（C1〜C5）を**実行結果として目で確認できる**。

このリポジトリは Java の環境構築を持たない（JDK も Maven も同梱しない）。
生成するのはソースとビルド手順だけで、JDK の用意は利用者に委ねる。

## 何の検体か

設計書どうしの食い違い（README の C1〜C3）は `資料/` 側で仕込んである。
ここで足すのは **コードと設計書の食い違い**という別の軸である。

- 詳細設計と一致する実装 —— C1（引当は出荷指示時）・C2（取消はステータス判定）
  はコードも詳細設計に従う。つまり**上流（基本設計・要件定義）だけが古い**
- 実装だけが違う ——  C4（明細上限）・C5（ロック待ち）は**コードが設計書と
  食い違う**。どちらが正かは人が決める（自動で寄せてはいけない）
- コードにしか無い記述 —— G6（得意先コードの前ゼロ埋め）は設計書に無い

## 名前で引く

設計項目に ID を振らないという `資料/` の決めごとはコード側にも効く。
javadoc に「関連する機能」「関連する業務ルール」を**名前で**書くだけなので、
設計書と突き合わせるには名寄せが要る。クラス名・メソッド名は `spec.MODULES` /
`spec.METHODS` と一致させてあり、これがトレース（realizes）の手がかりになる。

会社名・パッケージ名は架空のものを使う（`spec.ACCOUNTING_SYSTEM` の注記参照）。
"""

from __future__ import annotations

import re
from pathlib import Path

import code_auth
import code_billing
import code_check
import code_demo
import code_inventory
import code_kit as ck
import code_master
import code_order
import code_sources
import code_web
import xlsxkit as xk

PACKAGE_ROOT = ck.PACKAGE_ROOT
SRC_ROOT = "src/main/java/jp/co/contoso/sps"

_HEADER = f"""/*
 * {xk.PROJECT_NAME}
 * Copyright (c) 2026 {xk.VENDOR}
 */
"""


# ── 生成の下請け ──────────────────────────────────────────────────────────
# 値クラス・インターフェース・注釈を組む処理は code_kit に置いてある。
# サブシステムごとのモジュール（code_master / code_inventory / …）も同じ
# ビルダを使うので、ここから再輸出せず各モジュールが直接 code_kit を読む。
_package_of = ck.package_of
_head = ck.head
_bean = ck.bean
_iface = ck.iface
_annotation = ck.annotation


# ── framework（Spring の代わりに自前で宣言する注釈）──────────────────────────
_FRAMEWORK_NOTE = ("本番は Spring の同名アノテーションを使う。この資材を JDK だけで"
                   "ビルドできるようにするため、同じ名前の注釈を自前で宣言している。")

_FRAMEWORK = {
    "framework/Service.java": _annotation(
        "framework/Service.java",
        "業務ロジックを持つサービスであることを表す。\n *\n * " + _FRAMEWORK_NOTE, "TYPE"),
    "framework/Component.java": _annotation(
        "framework/Component.java",
        "共通部品であることを表す。\n *\n * " + _FRAMEWORK_NOTE, "TYPE"),
    "framework/Transactional.java": _annotation(
        "framework/Transactional.java",
        "メソッドの開始から終了までを 1 トランザクションとすることを表す。\n *\n * "
        + _FRAMEWORK_NOTE, "METHOD"),
}


# ── 値クラス・列挙・インターフェース ────────────────────────────────────────
_BD = "java.math.BigDecimal"
_LD = "java.time.LocalDate"

_BEANS: dict[str, str] = {
    "order/Amount.java": _bean(
        "order/Amount.java", "受注の金額。明細金額の合計と消費税額の参考値を持つ。",
        "BigDecimal total; BigDecimal tax", [_BD]),
    "order/ErrorInfo.java": _bean(
        "order/ErrorInfo.java", "入力チェックのエラー 1 件。項目名とメッセージを持つ。",
        "String field; String message", [],
        extra="""    /** 項目名とメッセージからエラーを作る。 */
    public static ErrorInfo of(String field, String message) {
        return new ErrorInfo(field, message);
    }"""),
    "order/OrderResult.java": _bean(
        "order/OrderResult.java", "受注登録の結果。採番した受注番号かエラー一覧を返す。",
        "String orderNo; OrderStatus status; List<ErrorInfo> errors",
        ["java.util.List", "java.util.Collections"],
        extra="""    /** 検証エラーで登録できなかった結果。 */
    public static OrderResult error(List<ErrorInfo> errors) {
        return new OrderResult(null, null, errors);
    }

    /** 登録できた結果。 */
    public static OrderResult of(String orderNo, OrderStatus status) {
        return new OrderResult(orderNo, status, Collections.emptyList());
    }

    /** エラーが無ければ真。 */
    public boolean isOk() {
        return errors == null || errors.isEmpty();
    }"""),
    "order/CancelResult.java": _bean(
        "order/CancelResult.java", "受注取消の結果。",
        "boolean ok; String message", [],
        extra="""    /** 取り消せた結果。 */
    public static CancelResult ok() {
        return new CancelResult(true, null);
    }

    /** 取り消せなかった結果。理由のメッセージを持つ。 */
    public static CancelResult error(String message) {
        return new CancelResult(false, message);
    }"""),
    "order/ShipmentResult.java": _bean(
        "order/ShipmentResult.java",
        "出荷指示の結果。作成できた指示番号か、在庫の不足か、出せなかった理由を返す。",
        "String orderNo; String shipmentNo; List<Shortage> shortages; !String message",
        ["java.util.Collections", "java.util.List",
         f"{PACKAGE_ROOT}.inventory.Shortage"],
        extra="""    /** 出荷指示を作成できた結果。 */
    public static ShipmentResult ok(String orderNo, String shipmentNo) {
        return new ShipmentResult(orderNo, shipmentNo, Collections.emptyList());
    }

    /** 在庫が不足して出荷指示を作成しなかった結果。 */
    public static ShipmentResult shortage(String orderNo, List<Shortage> shortages) {
        return new ShipmentResult(orderNo, null, shortages);
    }

    /** ステータスが対象外で出荷指示を作成しなかった結果。 */
    public static ShipmentResult rejected(String orderNo, String message) {
        ShipmentResult result =
                new ShipmentResult(orderNo, null, Collections.emptyList());
        result.setMessage(message);
        return result;
    }

    /** 出荷指示を作成できたか。 */
    public boolean isOk() {
        return shipmentNo != null;
    }"""),
    "order/OrderDto.java": _bean(
        "order/OrderDto.java",
        "受注入力画面から受け取る受注 1 件。\n *\n"
        " * 受注経路と入力者コードは呼出元が決める。未設定なら受注登録サービスが\n"
        " * 画面入力（1）とみなす。",
        "*String customerCd; LocalDate orderDate; LocalDate deliveryDate;"
        " List<OrderDetailDto> details; !OrderRoute route; !String entryStaffCd",
        [_LD, "java.util.List"]),
    "order/OrderDetailDto.java": _bean(
        "order/OrderDetailDto.java",
        "受注明細 1 行。販売単価と明細金額はサービスが決めて書き戻す。",
        "String productCd; BigDecimal orderQty; String taxType;"
        " !BigDecimal unitPrice; !BigDecimal detailAmount", [_BD]),
    "order/Order.java": _bean(
        "order/Order.java", "受注ヘッダ（T_ORDER）。",
        "String orderNo; LocalDate orderDate; String customerCd;"
        " *LocalDate deliveryDate; *BigDecimal totalAmount; BigDecimal taxAmount;"
        " *OrderStatus status; OrderRoute route; String entryStaffCd;"
        " *LocalDateTime updDatetime; *String cancelReason; *String cancelNote",
        [_BD, _LD, "java.time.LocalDateTime"]),
    "order/OrderDetail.java": _bean(
        "order/OrderDetail.java", "受注明細（T_ORDER_DETAIL）。",
        "String orderNo; int lineNo; String productCd; String taxType;"
        " *BigDecimal orderQty; *DetailStatus status; *BigDecimal allocatedQty;"
        " !BigDecimal unitPrice; !BigDecimal detailAmount", [_BD]),
    "inventory/Shortage.java": _bean(
        "inventory/Shortage.java", "引当で確保できなかった数量。商品ごとに 1 件。",
        "String productCd; BigDecimal shortQty", [_BD]),
    "inventory/AllocationResult.java": _bean(
        "inventory/AllocationResult.java", "引当の結果。不足があれば商品ごとに返す。",
        "List<Shortage> shortages", ["java.util.List"],
        extra="""    /** 不足が 1 件でもあれば真。 */
    public boolean hasShortage() {
        return shortages != null && !shortages.isEmpty();
    }"""),
    "inventory/Stock.java": _bean(
        "inventory/Stock.java", "在庫（T_STOCK）。ロット単位に持ち、先入先出で引き当てる。",
        "String warehouseCd; String productCd; String lotNo; LocalDate receiveDate;"
        " *BigDecimal stockQty; *BigDecimal allocatedQty", [_BD, _LD],
        extra="""    /** 有効在庫数（実在庫数 − 引当済数量）。 */
    public BigDecimal availableQty() {
        return stockQty.subtract(allocatedQty);
    }"""),
    "inventory/Allocation.java": _bean(
        "inventory/Allocation.java",
        "引当（T_ALLOCATION）。受注明細と在庫の対応を持つ。\n *\n"
        " * 引当は在庫のロット単位に作る。業務ルール「先入先出」で入庫日の古い\n"
        " * ロットから確保するため、どのロットから何個取ったかを残す必要がある。",
        "String orderNo; int lineNo; String warehouseCd; String productCd;"
        " String lotNo; BigDecimal allocatedQty", [_BD]),
    "billing/SalesLine.java": _bean(
        "billing/SalesLine.java", "締めの対象になる売上 1 行。税抜金額と税区分を持つ。",
        "String customerCd; LocalDate salesDate; String productCd;"
        " BigDecimal netAmount; String taxType; *String invoiceNo", [_BD, _LD]),
    "billing/CloseResult.java": _bean(
        "billing/CloseResult.java", "得意先 1 件の締めの結果。",
        "String customerCd; String invoiceNo; BigDecimal billingAmount; boolean skipped",
        [_BD],
        extra="""    /** 請求を作成した結果。 */
    public static CloseResult closed(String invoiceNo, BigDecimal billingAmount) {
        return new CloseResult(null, invoiceNo, billingAmount, false);
    }

    /** 金額が 0 で請求を作成しなかった結果。 */
    public static CloseResult skipped(String customerCd) {
        return new CloseResult(customerCd, null, BigDecimal.ZERO, true);
    }"""),
    "common/Customer.java": _bean(
        "common/Customer.java", "得意先マスタ（M_CUSTOMER）。",
        "String customerCd; String name; BigDecimal creditLimit;"
        " ClosingType closingType; boolean suspended; !boolean deleted", [_BD]),
}

_ENUMS = {
    "order/DetailStatus.java": _head("order/DetailStatus.java", []) + '''/**
 * 受注明細のステータス。
 *
 * コード値はコード定義書の受注明細ステータスと一致させる。
 */
public enum DetailStatus {

    /** 未引当。 */
    UNALLOCATED(10),

    /** 一部引当。有効在庫が不足し、確保できた分だけ引き当てた状態。 */
    PARTIAL(20),

    /** 引当済。 */
    ALLOCATED(30),

    /** 取消。 */
    CANCELED(90);

    private final int code;

    DetailStatus(int code) {
        this.code = code;
    }

    public int getCode() {
        return code;
    }
}
''',
}

_EXCEPTIONS = {
    "common/BusinessException.java": _head("common/BusinessException.java", []) + '''/**
 * 業務エラー。画面にメッセージを表示して処理を中断する種類の例外。
 */
public class BusinessException extends RuntimeException {

    public BusinessException(String message) {
        super(message);
    }
}
''',
    "common/CreditTimeoutException.java": _head(
        "common/CreditTimeoutException.java", []) + '''/**
 * 与信照会 API のタイムアウト。
 *
 * 外部インターフェース仕様書の「与信情報照会」に定める異常。発生時は与信
 * チェックを保留とし、受注ステータスを与信保留（20）にする。
 */
public class CreditTimeoutException extends RuntimeException {

    public CreditTimeoutException(String message) {
        super(message);
    }
}
''',
}

_INTERFACES = {
    "order/repository/OrderRepository.java": _iface(
        "order/repository/OrderRepository.java", "受注ヘッダ・受注明細への参照と更新。",
        [
            "Order find(String orderNo)",
            "List<OrderDetail> findDetails(String orderNo)",
            "OrderDetail findDetail(String orderNo, int lineNo)",
            "List<OrderDetail> findUnallocatedDetails(String orderNo)",
            "BigDecimal sumUnbilled(String customerCd)",
            "List<OrderSummary> search(OrderSearchCondition condition, int offset,"
            " int limit)",
            "int count(OrderSearchCondition condition)",
            "void insertHeader(String orderNo, OrderDto dto, Amount amount, OrderStatus status)",
            "void insertDetails(String orderNo, List<OrderDetailDto> details)",
            "void updateStatus(String orderNo, OrderStatus status)",
            "void updateDetailStatus(String orderNo, OrderStatus status)",
            "void updateDetailStatus(OrderDetail detail, DetailStatus status)",
            "void updateDetailQty(String orderNo, int lineNo, BigDecimal qty,"
            " BigDecimal amount)",
            "void updateHeader(String orderNo, LocalDate deliveryDate,"
            " BigDecimal totalAmount, OrderStatus status)",
            "void updateHeaderCanceled(String orderNo, String reason, String note)",
        ],
        [_BD, _LD, "java.util.List",
         f"{PACKAGE_ROOT}.order.Amount", f"{PACKAGE_ROOT}.order.DetailStatus",
         f"{PACKAGE_ROOT}.order.Order", f"{PACKAGE_ROOT}.order.OrderDetail",
         f"{PACKAGE_ROOT}.order.OrderDetailDto", f"{PACKAGE_ROOT}.order.OrderDto",
         f"{PACKAGE_ROOT}.order.OrderSearchCondition",
         f"{PACKAGE_ROOT}.order.OrderStatus", f"{PACKAGE_ROOT}.order.OrderSummary"]),
    "order/repository/ShipmentRepository.java": _iface(
        "order/repository/ShipmentRepository.java", "出荷指示（T_SHIPMENT）への登録。",
        ["void insert(String shipmentNo, String orderNo, LocalDate shipDate,"
         " String warehouseCd)"], [_LD]),
    "inventory/repository/StockRepository.java": _iface(
        "inventory/repository/StockRepository.java",
        "在庫への参照と、実在庫数・引当済数量の更新。\n *\n"
        " * 更新はすべてロット単位に受ける。業務ルール「先入先出」により、どの\n"
        " * ロットを動かすかは呼出元（在庫引当サービス・在庫更新サービス）が決める。",
        [
            "Stock find(String warehouseCd, String productCd, String lotNo)",
            "List<Stock> findByWarehouse(String warehouseCd)",
            "List<Stock> findLotsForUpdate(String warehouseCd, String productCd,"
            " int lockWaitSeconds)",
            "void insert(Stock stock)",
            "void addStockQty(String warehouseCd, String productCd, String lotNo,"
            " BigDecimal qty)",
            "void subtractStockQty(String warehouseCd, String productCd, String lotNo,"
            " BigDecimal qty)",
            "void addAllocatedQty(String warehouseCd, String productCd, String lotNo,"
            " BigDecimal qty)",
            "void subtractAllocatedQty(String warehouseCd, String productCd,"
            " String lotNo, BigDecimal qty)",
        ],
        [_BD, "java.util.List", f"{PACKAGE_ROOT}.inventory.Stock"]),
    "inventory/repository/AllocationRepository.java": _iface(
        "inventory/repository/AllocationRepository.java", "引当への登録・参照・削除。",
        [
            "void insert(String orderNo, int lineNo, String warehouseCd,"
            " String productCd, String lotNo, BigDecimal qty)",
            "List<Allocation> findByOrder(String orderNo)",
            "void deleteByOrder(String orderNo)",
        ],
        [_BD, "java.util.List", f"{PACKAGE_ROOT}.inventory.Allocation"]),
    "billing/repository/InvoiceRepository.java": _iface(
        "billing/repository/InvoiceRepository.java", "請求ヘッダへの参照と登録・更新。",
        [
            "boolean exists(String customerCd, String closingYm)",
            "BigDecimal previousBalance(String customerCd)",
            "BigDecimal depositAmount(String customerCd, LocalDate from, LocalDate to)",
            "void insert(Invoice invoice)",
            "Invoice find(String invoiceNo)",
            "List<Invoice> findAll()",
            "List<Invoice> findByClosingYm(String closingYm)",
            "List<Invoice> findUnpaid(String customerCd)",
            "void updateStatus(String invoiceNo, InvoiceStatus status)",
            "void updateDeposit(String invoiceNo, BigDecimal appliedAmount,"
            " InvoiceStatus status)",
        ],
        [_BD, _LD, "java.util.List", f"{PACKAGE_ROOT}.billing.Invoice",
         f"{PACKAGE_ROOT}.billing.InvoiceStatus"]),
    "billing/repository/SalesRepository.java": _iface(
        "billing/repository/SalesRepository.java",
        "売上の登録・集計と請求番号の書き戻し。",
        [
            "void insert(SalesLine line)",
            "List<SalesLine> aggregate(String customerCd, LocalDate from, LocalDate to)",
            "List<SalesLine> findByDate(LocalDate salesDate)",
            "List<SalesLine> findByInvoiceNo(String invoiceNo)",
            "void writeBackInvoiceNo(List<SalesLine> lines, String invoiceNo)",
        ],
        ["java.util.List", _LD, f"{PACKAGE_ROOT}.billing.SalesLine"]),
    "common/repository/CustomerRepository.java": _iface(
        "common/repository/CustomerRepository.java", "得意先マスタへの参照と更新。",
        [
            "Customer find(String customerCd)",
            "List<Customer> findActive()",
            "void save(Customer customer)",
            "void logicalDelete(String customerCd)",
        ],
        ["java.util.List", f"{PACKAGE_ROOT}.common.Customer"]),
    "common/repository/PriceRepository.java": _iface(
        "common/repository/PriceRepository.java",
        "得意先別単価マスタ・商品マスタの単価への参照。",
        [
            "BigDecimal findContractPrice(String customerCd, String productCd,"
            " LocalDate date)",
            "BigDecimal findStandardPrice(String productCd)",
        ], [_BD, _LD]),
    "common/repository/TaxRateRepository.java": _iface(
        "common/repository/TaxRateRepository.java", "税率マスタ（M_TAX_RATE）への参照。",
        ["BigDecimal find(String taxType, LocalDate date)"], [_BD, _LD]),
    "common/repository/NumberingRepository.java": _iface(
        "common/repository/NumberingRepository.java", "採番テーブルへの排他つき加算。",
        ["long incrementForUpdate(String numberingType, LocalDate date,"
         " int lockTimeoutSeconds)"], [_LD]),
    "common/repository/CalendarRepository.java": _iface(
        "common/repository/CalendarRepository.java", "会社カレンダー（M_CALENDAR）への参照。",
        ["Boolean isBusinessDay(LocalDate date)"], [_LD]),
}

_CLIENTS = {
    "common/CreditClient.java": _head("common/CreditClient.java", [_BD]) + '''/**
 * 与信情報照会のクライアント。
 *
 * 外部インターフェース仕様書の「与信情報照会」に対応する。タイムアウト時は
 * {@link CreditTimeoutException} を送出し、呼出元は与信保留に倒す。
 */
public interface CreditClient {

    /** 得意先の売掛残高を照会する。 */
    BigDecimal fetchReceivable(String customerCd);
}
''',
}


def _files() -> dict[str, str]:
    """出力する Java ファイル（パッケージ配下の相対パス → 中身）。"""
    files: dict[str, str] = {}
    files.update(_FRAMEWORK)
    files.update(_BEANS)
    files.update(_ENUMS)
    files.update(_EXCEPTIONS)
    files.update(_INTERFACES)
    files.update(_CLIENTS)
    files.update(_HAND_WRITTEN)
    return files


# ── 生成時の突き合わせ ────────────────────────────────────────────────────
_IMPORT_RE = re.compile(rf"^import ({re.escape(PACKAGE_ROOT)}[\w.]+);", re.MULTILINE)
_PUBLIC_TYPE_RE = re.compile(r"public (?:final )?(?:class|interface|enum|@interface) (\w+)")


def verify(files: dict[str, str]) -> list[str]:
    """コンパイルの前に気づける食い違いを洗い出す。

    javac の代わりにはならないが、**参照している自前クラスが存在しない**
    「必ずコンパイルが通らない」型の間違いはここで止まる。
    """
    problems: list[str] = []
    declared = {f"{_package_of(rel)}.{rel.split('/')[-1].removesuffix('.java')}"
               for rel in files}

    for rel, source in sorted(files.items()):
        name = rel.split("/")[-1].removesuffix(".java")
        public_types = _PUBLIC_TYPE_RE.findall(source)
        if name not in public_types:
            problems.append(f"{rel}: ファイル名と public 型名が一致しない {public_types}")
        if "org.springframework" in source:
            problems.append(f"{rel}: 外部依存（Spring）が残っている")
        for imported in _IMPORT_RE.findall(source):
            if imported not in declared:
                problems.append(f"{rel}: import 先が存在しない {imported}")
        # 同じパッケージ以外の自前クラスを import 無しで使っていないか。
        # javadoc の {@link ...} で名前を挙げるだけの箇所を拾わないよう、
        # コメントを落としてから調べる。
        code = _strip_comments(source)
        same_package = _package_of(rel) + "."
        for other in declared:
            package, simple = other.rsplit(".", 1)
            if package + "." == same_package or simple == name:
                continue
            if re.search(rf"\b{simple}\b", code) and f"import {other};" not in source:
                problems.append(f"{rel}: {simple} を import せずに使っている")
    return problems


def _strip_comments(source: str) -> str:
    source = re.sub(r"/\*.*?\*/", " ", source, flags=re.DOTALL)
    return re.sub(r"//[^\n]*", " ", source)


def build(out: Path) -> list[Path]:
    """`out`（実装/）へ Java ソースとビルド手順を書き出し、書いたパスを返す。"""
    files = _files()
    problems = verify(files)
    if problems:
        raise SystemExit("実装コードの生成に矛盾があります:\n  " + "\n  ".join(problems))

    written: list[Path] = []
    for rel, source in files.items():
        path = out / SRC_ROOT / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(_HEADER + source, encoding="utf-8")
        written.append(path)

    # javac へ渡すソース一覧（シェルの違いを気にせず ``javac @sources.txt`` で通る）
    listing = out / "sources.txt"
    listing.write_text(
        "\n".join(sorted(f"{SRC_ROOT}/{rel}" for rel in files)) + "\n", encoding="utf-8")
    written.append(listing)

    readme = out / "README.md"
    readme.write_text(_BUILD_README, encoding="utf-8")
    written.append(readme)

    # javac の代わりの構造検査。ここで落としておかないと、利用者が
    # 「ビルドできない資材」を受け取ることになる。
    broken = code_check.check(out / SRC_ROOT)
    if broken:
        raise SystemExit("生成した Java に構造的な誤りがあります:\n  "
                         + "\n  ".join(broken))
    return written


_BUILD_README = f"""# 新販売管理システム 実装コード（架空）

`examples/sales-corpus/build.py` が生成した Java ソース。設計書（`../資料/`）と
対になる成果物で、**外部ライブラリに依存しない**ので JDK だけでビルドできる。

## 必要なもの

JDK 17 以上だけ。Maven / Gradle は要らない。このリポジトリは JDK を同梱しないので
用意は各自で行う（`java -version` が動けばよい）。

## ビルドと実行

```bash
cd examples/sales-corpus/実装
javac -encoding UTF-8 -d out @sources.txt
java -cp out {PACKAGE_ROOT}.demo.Main
```

PowerShell でも通るが、`-D` から始まる引数はクォートしないと分割されるので
`java '-Dfile.encoding=UTF-8' -cp out {PACKAGE_ROOT}.demo.Main` の形にする
（文字化けする場合に使う）。

## ブラウザでログインして触る

画面「ログイン」をブラウザから触れる形でも動かせる。

```bash
java -cp out {PACKAGE_ROOT}.demo.WebMain 8080
```

`http://127.0.0.1:8080/login` を開くと、社員コードとパスワードで認証してから
権限に応じたメニューが出る。試せる社員コードとパスワードはログイン画面に
表示してある。待ち受けはループバックのみで、止めるのは Ctrl+C。

設計書の**画面一覧 16 画面すべて**をメニューに並べ、いずれもブラウザから
操作できる。いまの権限で使えない画面は「権限なし」と出る（管理者
`100099 / Admin#2026` で入ると全画面を触れる）。

| サブシステム | 画面 |
| --- | --- |
| 受注管理 | 受注入力・受注一覧照会・受注取消・出荷指示・EDI受注取込結果照会 |
| 在庫管理 | 在庫照会・入庫登録・棚卸入力・在庫調整 |
| 請求管理 | 請求締め処理・請求書発行・入金消込・売掛残高照会 |
| 共通基盤 | ログイン・得意先マスタ保守・商品マスタ保守 |

請求締め処理 → 請求書発行 → 入金消込 → 売掛残高照会は、初期データのまま
順に触れば一通り流れる（`WebMain` が売上と入金を積んである）。

## 実行すると何が分かるか

`demo.Main` は受注登録から請求締め・入金消込・仕訳連携までを 20 シナリオで
通し、**設計書との食い違いを実行結果として見せる**。詳しくは `../README.md` の
「コードと設計書の食い違い」を参照。

| シナリオ | 確かめられること |
| --- | --- |
| 与信枠内の受注を登録 | 明細金額は円未満切り捨て・ステータスは受付(10) |
| 与信枠を超える受注を登録 | ステータスが与信保留(20)になる |
| 明細 60 行の受注を登録 | **C4** 設計書は 50 行までだが実装は通る |
| 出荷指示を実行 | **C1** ここで初めて引当が走り、引当済(30)→出荷指示済(40) |
| 出荷指示済の受注を取消 | **C2** 日付ではなくステータスで不可と判定される |
| 在庫不足の受注を出荷指示 | 一部引当となり不足数が返る（ロット単位・先入先出） |
| 与信保留の受注を出荷指示 | 承認待ちのため引当も出荷指示も行われない |
| 受注内容を変更 | 出荷指示前なら変えられる／出荷指示済は変えられない |
| EDI受注を取込 | JAN コードから商品を引き、弾かれた 1 件は理由が残る |
| 入庫登録・在庫照会 | 未登録ロットは行を起こす・有効在庫が引当済ぶん減る |
| 出荷実績を受信 | 在庫を引き落とし、売上を計上する |
| 棚卸を確定 | 差異のある行だけが在庫調整になる |
| 実在庫を超える出庫 | 業務ルール「在庫マイナスの禁止」で弾かれる |
| 請求締めを実行 | **C3** 明細単位の切り捨てと請求単位の四捨五入で税額が違う |
| 請求書を発行 | 帳票基盤へ渡す出力データを組み立て、二重発行を弾く |
| 入金を消し込む | 請求番号一致は自動、不一致は候補を出して人が選ぶ |
| 売上仕訳を連携 | 売上高と仮受消費税に分けた仕訳を出力する |
| 売掛残高を照会 | 支払期日を過ぎた請求が滞留として出る |
| 監査ログを検証 | 1 件書き換えるとハッシュの連鎖が切れて検知できる |
| 受注一覧を照会 | 検索条件とページ送りが効く |

## 構成

```
src/main/java/{SRC_ROOT.split('java/')[1]}/
├── framework/    3 本  Spring の代わりに自前で宣言した注釈（@Service など）
├── order/       29 本  受注管理（登録・変更・取消・検索・出荷指示・EDI取込）
├── inventory/   20 本  在庫管理（引当・入出庫・棚卸・出荷実績取込・照会）
├── billing/     23 本  請求管理（締め・請求書発行・入金消込・仕訳連携・売掛照会）
├── common/      30 本  共通基盤（認証・マスタ保守・採番・消費税・カレンダー・監査ログ）
└── demo/         9 本  メモリ上のリポジトリ実装と Main / WebMain（配布サンプル用）
```

`demo` は動かすための足場で、設計書に対応する成果物ではない。認証そのもの
（`AuthService` ・ 社員マスタ ・ パスワード方針）は設計書に対応する成果物として
`common` にある。

## 設計書との既知の食い違い

### 通信の暗号化

非機能要件「通信の暗号化」は画面の通信を TLS1.2 以上で暗号化することを求めるが、
`WebMain` は平文 HTTP で待ち受ける。配布物に証明書を同梱できないためで、
C1〜C5 と同じく**コードと設計書の食い違い**として残る。

### 設計書にモジュール定義が無いサービス

機能要件にはあるのに詳細設計がモジュールを起こしていないものが 4 つある。
実装しないと機能要件を満たせないので、コード側だけに置いてある。G6（得意先
コードの前ゼロ埋め）と同じ性質のもので、javadoc にその旨を書いてある。

| クラス | 対応する機能要件 |
| --- | --- |
| `OrderUpdateService` | 受注内容の変更 |
| `StockInquiryService` | 在庫照会 |
| `ReceivableInquiryService` | 売掛残高管理 |
| `MasterMaintenanceService` | マスタ保守 |
"""


# ── 手書きのソース（サービス・共通部品・デモ）─────────────────────────────
_ORDER_STATUS = _head("order/OrderStatus.java", []) + '''/**
 * 受注ステータス。
 *
 * コード値はコード定義書の受注ステータスと一致させる。状態遷移は
 * 基本設計書の状態遷移表に従う。
 */
public enum OrderStatus {

    /** 受付。与信の枠内で登録された状態。 */
    ACCEPTED(10, "受付"),

    /** 与信保留。与信限度額を超えたため営業部長の承認を待つ状態。 */
    CREDIT_HOLD(20, "与信保留"),

    /** 引当済。出荷指示の実行時に在庫を確保できた状態。 */
    ALLOCATED(30, "引当済"),

    /** 出荷指示済。これ以降は取消できない。 */
    SHIP_INSTRUCTED(40, "出荷指示済"),

    /** 取消。 */
    CANCELED(90, "取消");

    private final int code;
    private final String label;

    OrderStatus(int code, String label) {
        this.code = code;
        this.label = label;
    }

    public int getCode() {
        return code;
    }

    public String getLabel() {
        return label;
    }

    /** コード値から列挙子を引く。該当が無ければ例外を送出する。 */
    public static OrderStatus of(int code) {
        for (OrderStatus status : values()) {
            if (status.code == code) {
                return status;
            }
        }
        throw new IllegalArgumentException("未知の受注ステータス: " + code);
    }
}
'''

_CLOSING_TYPE = _head("common/ClosingType.java", ["java.time.LocalDate"]) + '''/**
 * 締日区分。
 *
 * 得意先マスタの締日区分に対応する。要件定義では「締めは毎月20日」と
 * されているが、得意先には 20 日締めと末日締めの 2 種類が存在する。
 *
 * 関連する業務ルール: 請求の締め日
 * 関連する非機能要件: 保守性
 *
 * 何日で締めるかはここに持たない。非機能要件「保守性」が締め日をプログラム
 * 修正なしに変更できることを求めるので、締め日マスタから引いて渡す。
 */
public enum ClosingType {

    /** 20日締め。 */
    TWENTIETH("1", "20日締め"),

    /** 末日締め。 */
    MONTH_END("2", "末日締め");

    private final String code;
    private final String label;

    ClosingType(String code, String label) {
        this.code = code;
        this.label = label;
    }

    public String getCode() {
        return code;
    }

    public String getLabel() {
        return label;
    }

    /**
     * 当月の締め日（休日を考慮しない暦上の日付）を返す。
     *
     * 締め日が月の日数を超える指定（末日締めの 31 など）は月の末日に丸める。
     * 休日にあたる場合の前営業日への繰り上げは営業日カレンダー部品が行う。
     */
    public LocalDate nominalClosingDate(LocalDate base, int closingDay) {
        return base.withDayOfMonth(Math.min(closingDay, base.lengthOfMonth()));
    }

    /** コード値から列挙子を引く。 */
    public static ClosingType of(String code) {
        for (ClosingType type : values()) {
            if (type.code.equals(code)) {
                return type;
            }
        }
        throw new IllegalArgumentException("未知の締日区分: " + code);
    }
}
'''

_TAX_GROUP = _head("billing/TaxGroup.java",
                   ["java.math.BigDecimal", "java.util.ArrayList",
                    "java.util.LinkedHashMap", "java.util.List", "java.util.Map"]) + '''/**
 * 税率ごとにまとめた税抜金額。
 *
 * 消費税は税率が混在する場合、税率ごとに集計してから計算する。
 */
public class TaxGroup {

    private final String taxType;
    private final BigDecimal netAmount;

    public TaxGroup(String taxType, BigDecimal netAmount) {
        this.taxType = taxType;
        this.netAmount = netAmount;
    }

    public String getTaxType() {
        return taxType;
    }

    public BigDecimal getNetAmount() {
        return netAmount;
    }

    /** 売上明細を税区分ごとに集計する。 */
    public static List<TaxGroup> groupBy(List<SalesLine> lines) {
        Map<String, BigDecimal> sums = new LinkedHashMap<>();
        for (SalesLine line : lines) {
            sums.merge(line.getTaxType(), line.getNetAmount(), BigDecimal::add);
        }
        List<TaxGroup> groups = new ArrayList<>();
        for (Map.Entry<String, BigDecimal> entry : sums.entrySet()) {
            groups.add(new TaxGroup(entry.getKey(), entry.getValue()));
        }
        return groups;
    }
}
'''


#: 手書きのソース。列挙・集計クラスはここ、中身のあるコードは別モジュールに置く。
_HAND_WRITTEN: dict[str, str] = {
    "order/OrderStatus.java": _ORDER_STATUS,
    "common/ClosingType.java": _CLOSING_TYPE,
    "billing/TaxGroup.java": _TAX_GROUP,
    **code_sources.HAND_WRITTEN,
    **code_auth.AUTH,
    **code_master.MASTER,
    **code_order.ORDER,
    **code_inventory.INVENTORY,
    **code_billing.BILLING,
    **code_demo.DEMO,
    **code_web.WEB,
}
