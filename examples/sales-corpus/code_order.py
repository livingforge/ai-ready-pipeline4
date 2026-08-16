"""受注管理（`order`）のうち、登録・取消・出荷指示以外のモジュールの Java ソース。

設計書のモジュール一覧にある

- EDI受注取込バッチ（`EdiOrderImportBatch`）
- 受注検索サービス（`OrderSearchService`）

と、機能要件「受注内容の変更」に対応する `OrderUpdateService` を置く。
受注登録・受注取消・出荷指示は C1・C2・C4・G6 を仕込んである関係で
`code_sources.py` に残してある。

`OrderUpdateService` は `spec.MODULES` にモジュール定義が無い。機能要件
「受注内容の変更」に対応する実装が要るのに設計書がモジュールを起こして
いないため、G6 と同じ**コードにしか無い**ものとして javadoc に書いてある。
"""

from __future__ import annotations

import code_kit as ck

_BD = "java.math.BigDecimal"
_LD = "java.time.LocalDate"
_LDT = "java.time.LocalDateTime"
_ROOT = ck.PACKAGE_ROOT


# ── 受注経路（コード定義書の受注経路と一致させる）──────────────────────────
_ORDER_ROUTE = ck.code_enum(
    "order/OrderRoute.java",
    "受注経路（T_ORDER.ORDER_ROUTE）。\n *\n"
    " * コード値はコード定義書の受注経路と一致させる。EDI 取込で登録した受注は\n"
    " * 2 になるので、画面入力ぶんと取込ぶんを後から区別できる。",
    [
        ("SCREEN", "1", "画面入力"),
        ("EDI", "2", "EDI"),
        ("FAX", "3", "FAX代行入力"),
    ])


# ── 値クラス ──────────────────────────────────────────────────────────────
_EDI_RECORD = ck.bean(
    "order/EdiRecord.java",
    "EDI受信ワーク（W_EDI_RECV）の 1 行。\n *\n"
    " * 流通BMS で受信した発注データを取込前に一時保持したもの。得意先コードは\n"
    " * 相手先が採番した 6 桁、商品コードは JAN コードで届く。",
    "String recvNo; String ediCustomerCd; String janCd; BigDecimal orderQty;"
    " LocalDate orderDate; LocalDate deliveryDate; *String status;"
    " *String errorMessage; *String orderNo", [_BD, _LD])

_IMPORT_ERROR = ck.bean(
    "order/ImportError.java",
    "EDI受注取込で弾かれた 1 件。受信番号と理由を持つ。",
    "String recvNo; String message", [])

_IMPORT_SUMMARY = ck.bean(
    "order/ImportSummary.java",
    "EDI受注取込の結果。成功件数・エラー件数とエラーの明細を返す。",
    "int successCount; int errorCount; List<ImportError> errors",
    ["java.util.List"])

_ORDER_SEARCH_CONDITION = ck.bean(
    "order/OrderSearchCondition.java",
    "受注一覧照会の検索条件。\n *\n"
    " * 未入力の項目は絞り込みに使わない。ページは 1 起点で数える。画面から\n"
    " * 少しずつ埋めるので、引数の無いコンストラクタで作って setter で詰める。",
    "!LocalDate orderDateFrom; !LocalDate orderDateTo; !String customerCd;"
    " !OrderStatus status; !int page; !int pageSize", [_LD])

_ORDER_SUMMARY = ck.bean(
    "order/OrderSummary.java",
    "受注一覧照会の 1 行。ヘッダの項目に得意先名を添えたもの。",
    "String orderNo; LocalDate orderDate; String customerCd; String customerName;"
    " BigDecimal totalAmount; OrderStatus status; OrderRoute route", [_BD, _LD])

_ORDER_PAGE = ck.bean(
    "order/OrderPage.java",
    "受注一覧照会の 1 ページ。表示する行と、ページ送りに要る件数を持つ。",
    "List<OrderSummary> rows; int page; int pageSize; int totalCount",
    ["java.util.List"],
    extra="""    /** 最後のページか。 */
    public boolean isLast() {
        return page * pageSize >= totalCount;
    }

    /** 総ページ数。 */
    public int getTotalPages() {
        return totalCount == 0 ? 1 : (totalCount + pageSize - 1) / pageSize;
    }""")

_DETAIL_CHANGE = ck.bean(
    "order/DetailChange.java",
    "受注内容の変更で受け取る明細 1 行の変更後の数量。",
    "int lineNo; BigDecimal orderQty", [_BD])

_UPDATE_RESULT = ck.bean(
    "order/UpdateResult.java",
    "受注内容の変更の結果。",
    "boolean ok; String message; List<ErrorInfo> errors",
    ["java.util.Collections", "java.util.List"],
    extra="""    /** 変更できた結果。 */
    public static UpdateResult ok(String message) {
        return new UpdateResult(true, message, Collections.emptyList());
    }

    /** 変更できなかった結果。 */
    public static UpdateResult error(String message) {
        return new UpdateResult(false, message, Collections.emptyList());
    }

    /** 入力チェックで弾かれた結果。 */
    public static UpdateResult invalid(List<ErrorInfo> errors) {
        return new UpdateResult(false, "入力内容に誤りがあります。", errors);
    }""")


# ── リポジトリ ────────────────────────────────────────────────────────────
_EDI_RECV_REPOSITORY = ck.iface(
    "order/repository/EdiRecvRepository.java",
    "EDI受信ワーク（W_EDI_RECV）への参照と更新。",
    [
        "List<EdiRecord> findUnprocessed(LocalDate targetDate)",
        "List<EdiRecord> findByDate(LocalDate targetDate)",
        "void markImported(String recvNo, String orderNo)",
        "void markError(String recvNo, String message)",
    ],
    [_LD, "java.util.List", f"{_ROOT}.order.EdiRecord"])


# ── EDI受注取込バッチ ─────────────────────────────────────────────────────
_EDI_IMPORT = '''package jp.co.contoso.sps.order.batch;

import java.time.LocalDate;
import java.util.ArrayList;
import java.util.List;

import jp.co.contoso.sps.common.AuditLogger;
import jp.co.contoso.sps.common.Product;
import jp.co.contoso.sps.common.repository.ProductRepository;
import jp.co.contoso.sps.framework.Component;
import jp.co.contoso.sps.framework.Transactional;
import jp.co.contoso.sps.order.EdiRecord;
import jp.co.contoso.sps.order.ErrorInfo;
import jp.co.contoso.sps.order.ImportError;
import jp.co.contoso.sps.order.ImportSummary;
import jp.co.contoso.sps.order.OrderDetailDto;
import jp.co.contoso.sps.order.OrderDto;
import jp.co.contoso.sps.order.OrderResult;
import jp.co.contoso.sps.order.OrderRoute;
import jp.co.contoso.sps.order.repository.EdiRecvRepository;
import jp.co.contoso.sps.order.service.OrderRegistService;

/**
 * EDI受注取込バッチ。
 *
 * EDI受信ワークのデータを検証して受注として登録し、エラーを取込結果に記録する。
 *
 * 関連する機能: EDI受注の取込
 * 関連する画面: EDI受注取込結果照会
 * 関連する外部インターフェース: EDI受注データ受信
 *
 * 日次 6 回、受信のたびに起動する。受信 1 件ごとにトランザクションを切り、
 * 弾かれた 1 件が他の受信を巻き込まないようにする。登録した受注の受注経路は
 * EDI（2）とする。
 */
@Component
public class EdiOrderImportBatch {

    private final EdiRecvRepository ediRecvRepository;
    private final ProductRepository productRepository;
    private final OrderRegistService orderRegistService;
    private final AuditLogger auditLogger;

    public EdiOrderImportBatch(EdiRecvRepository ediRecvRepository,
                               ProductRepository productRepository,
                               OrderRegistService orderRegistService,
                               AuditLogger auditLogger) {
        this.ediRecvRepository = ediRecvRepository;
        this.productRepository = productRepository;
        this.orderRegistService = orderRegistService;
        this.auditLogger = auditLogger;
    }

    /**
     * EDI受信ワークを 1 件ずつ受注として登録し、成功件数とエラー件数を返す。
     *
     * 受信レコードの変換に失敗した場合と、受注登録の入力チェックで弾かれた
     * 場合はエラーとして受信ワークに記録し、次の 1 件へ進む。
     */
    public ImportSummary importOrders(LocalDate targetDate) {
        List<ImportError> errors = new ArrayList<>();
        int success = 0;
        for (EdiRecord record : ediRecvRepository.findUnprocessed(targetDate)) {
            String failed = importOne(record);
            if (failed == null) {
                success++;
            } else {
                errors.add(new ImportError(record.getRecvNo(), failed));
            }
        }
        auditLogger.record("EDI受注取込", targetDate.toString(), null,
                "成功 " + success + " 件 / エラー " + errors.size() + " 件");
        return new ImportSummary(success, errors.size(), errors);
    }

    /** 受信 1 件を取り込む。取り込めた場合は null、弾かれた場合は理由を返す。 */
    @Transactional
    public String importOne(EdiRecord record) {
        OrderDto dto;
        try {
            dto = convertToOrder(record);
        } catch (IllegalArgumentException e) {
            ediRecvRepository.markError(record.getRecvNo(), e.getMessage());
            return e.getMessage();
        }

        OrderResult result = orderRegistService.registerOrder(dto);
        if (!result.isOk()) {
            String message = message(result.getErrors());
            ediRecvRepository.markError(record.getRecvNo(), message);
            return message;
        }
        ediRecvRepository.markImported(record.getRecvNo(), result.getOrderNo());
        return null;
    }

    /**
     * 流通BMS の受信レコードを受注データへ変換する。
     *
     * 得意先コードは相手先の 6 桁で届くので、前ゼロを詰めた 8 桁に読み替える
     * （読み替えの実体は受注登録サービスの入力チェックにある）。商品コードは
     * JAN コードで届くので、商品マスタの JAN コードから商品コードを引く。
     */
    public OrderDto convertToOrder(EdiRecord record) {
        Product product = productRepository.findByJan(record.getJanCd());
        if (product == null) {
            throw new IllegalArgumentException(
                    "JANコード「" + record.getJanCd() + "」に対応する商品がありません。");
        }
        if (record.getOrderQty() == null) {
            throw new IllegalArgumentException("受注数量を入力してください。");
        }
        OrderDetailDto detail = new OrderDetailDto(product.getProductCd(),
                record.getOrderQty(), product.getTaxType());
        List<OrderDetailDto> details = new ArrayList<>();
        details.add(detail);

        OrderDto dto = new OrderDto(record.getEdiCustomerCd(), record.getOrderDate(),
                record.getDeliveryDate(), details);
        dto.setRoute(OrderRoute.EDI);
        dto.setEntryStaffCd(AuditLogger.SYSTEM);
        return dto;
    }

    /** 受信の取込結果を返す（EDI受注取込結果照会の表示用）。 */
    public List<EdiRecord> findResults(LocalDate targetDate) {
        return ediRecvRepository.findByDate(targetDate);
    }

    private static String message(List<ErrorInfo> errors) {
        StringBuilder text = new StringBuilder();
        for (ErrorInfo error : errors) {
            if (text.length() > 0) {
                text.append(" / ");
            }
            text.append(error.getMessage());
        }
        return text.toString();
    }
}
'''


# ── 受注検索サービス ──────────────────────────────────────────────────────
_ORDER_SEARCH = '''package jp.co.contoso.sps.order.service;

import java.util.List;

import jp.co.contoso.sps.framework.Service;
import jp.co.contoso.sps.order.OrderDetail;
import jp.co.contoso.sps.order.OrderPage;
import jp.co.contoso.sps.order.OrderSearchCondition;
import jp.co.contoso.sps.order.OrderSummary;
import jp.co.contoso.sps.order.repository.OrderRepository;

/**
 * 受注検索サービス。
 *
 * 検索条件から受注ヘッダと明細を取得し、ページ単位で返す。
 *
 * 関連する機能: 受注状況の照会
 * 関連する画面: 受注一覧照会 / 受注取消 / 出荷指示
 * 関連する非機能要件: 一覧検索の応答時間
 *
 * 非機能要件「一覧検索の応答時間」が 100 件表示で 5 秒以内を求めるため、
 * 全件を読んでから捨てる作りにはせず、ページぶんだけを取得する。
 */
@Service
public class OrderSearchService {

    /** 1 ページの既定の表示件数。 */
    private static final int DEFAULT_PAGE_SIZE = 100;

    /** 1 ページの上限。これを超える指定は上限に丸める。 */
    private static final int MAX_PAGE_SIZE = 500;

    private final OrderRepository orderRepository;

    public OrderSearchService(OrderRepository orderRepository) {
        this.orderRepository = orderRepository;
    }

    /**
     * 検索条件に合う受注をページ単位で返す。
     *
     * ページ・表示件数が未設定の場合は 1 ページ目・100 件とする。
     */
    public OrderPage search(OrderSearchCondition condition) {
        int pageSize = condition.getPageSize() <= 0
                ? DEFAULT_PAGE_SIZE
                : Math.min(condition.getPageSize(), MAX_PAGE_SIZE);
        int page = Math.max(condition.getPage(), 1);
        int offset = (page - 1) * pageSize;

        List<OrderSummary> rows = orderRepository.search(condition, offset, pageSize);
        int total = orderRepository.count(condition);
        return new OrderPage(rows, page, pageSize, total);
    }

    /** 受注 1 件の明細を返す。 */
    public List<OrderDetail> findDetails(String orderNo) {
        return orderRepository.findDetails(orderNo);
    }
}
'''


# ── 受注変更サービス ──────────────────────────────────────────────────────
_ORDER_UPDATE = '''package jp.co.contoso.sps.order.service;

import java.math.BigDecimal;
import java.math.RoundingMode;
import java.time.LocalDate;
import java.time.LocalDateTime;
import java.util.ArrayList;
import java.util.List;

import jp.co.contoso.sps.common.AuditLogger;
import jp.co.contoso.sps.common.BusinessDayCalendar;
import jp.co.contoso.sps.framework.Service;
import jp.co.contoso.sps.framework.Transactional;
import jp.co.contoso.sps.order.DetailChange;
import jp.co.contoso.sps.order.ErrorInfo;
import jp.co.contoso.sps.order.Order;
import jp.co.contoso.sps.order.OrderDetail;
import jp.co.contoso.sps.order.OrderStatus;
import jp.co.contoso.sps.order.UpdateResult;
import jp.co.contoso.sps.order.repository.OrderRepository;

/**
 * 受注変更サービス。
 *
 * 出荷指示を出す前の受注について、数量・納品希望日を変更する。
 *
 * 関連する機能: 受注内容の変更
 * 関連する画面: 受注入力
 * 関連する業務ルール: 出荷指示済み受注の変更禁止 / 受注数量の検証 / 納品希望日の検証
 *
 * 詳細設計にモジュールの定義が無い。機能要件「受注内容の変更」に対応する実装が
 * 要るのに設計書がモジュールを起こしていないため、コード側だけに存在する。
 */
@Service
public class OrderUpdateService {

    private final OrderRepository orderRepository;
    private final BusinessDayCalendar calendar;
    private final OrderRegistService orderRegistService;
    private final AuditLogger auditLogger;

    public OrderUpdateService(OrderRepository orderRepository,
                              BusinessDayCalendar calendar,
                              OrderRegistService orderRegistService,
                              AuditLogger auditLogger) {
        this.orderRepository = orderRepository;
        this.calendar = calendar;
        this.orderRegistService = orderRegistService;
        this.auditLogger = auditLogger;
    }

    /**
     * 受注の数量と納品希望日を変更する。
     *
     * 出荷指示済（40）以降と取消済（90）の受注は変更できない。数量を変えた
     * 場合は明細金額と受注金額合計を計算し直し、与信を再判定する。
     * 変更後に与信限度額を超える場合は与信保留（20）へ落とす。
     */
    @Transactional
    public UpdateResult changeOrder(String orderNo, String staffCd,
                                    LocalDate deliveryDate,
                                    List<DetailChange> changes,
                                    LocalDateTime updDatetime) {
        Order order = orderRepository.find(orderNo);
        if (order == null) {
            return UpdateResult.error("受注番号を入力してください。");
        }
        if (!order.getUpdDatetime().equals(updDatetime)) {
            return UpdateResult.error("他の利用者が更新しました。再度読み込んでください。");
        }
        if (!isChangeable(order.getStatus())) {
            return UpdateResult.error("出荷指示済みの受注は変更できません。");
        }

        List<OrderDetail> details = orderRepository.findDetails(orderNo);
        List<ErrorInfo> errors = validate(deliveryDate, order.getOrderDate(), changes,
                details);
        if (!errors.isEmpty()) {
            return UpdateResult.invalid(errors);
        }

        for (DetailChange change : changes) {
            OrderDetail detail = find(details, change.getLineNo());
            BigDecimal amount = change.getOrderQty()
                    .multiply(detail.getUnitPrice())
                    .setScale(0, RoundingMode.FLOOR);
            orderRepository.updateDetailQty(orderNo, change.getLineNo(),
                    change.getOrderQty(), amount);
        }

        BigDecimal total = BigDecimal.ZERO;
        for (OrderDetail detail : orderRepository.findDetails(orderNo)) {
            total = total.add(detail.getDetailAmount());
        }
        OrderStatus status = orderRegistService.checkCredit(order.getCustomerCd(), total)
                ? OrderStatus.ACCEPTED
                : OrderStatus.CREDIT_HOLD;
        orderRepository.updateHeader(orderNo, deliveryDate, total, status);
        auditLogger.record(staffCd, "受注変更", orderNo,
                order.getTotalAmount(), total);
        return UpdateResult.ok("受注番号" + orderNo + "を変更しました。");
    }

    /**
     * 変更可否をステータスで判定する。
     *
     * 関連する業務ルール: 出荷指示済み受注の変更禁止
     */
    public boolean isChangeable(OrderStatus status) {
        return status.getCode() < OrderStatus.SHIP_INSTRUCTED.getCode();
    }

    private List<ErrorInfo> validate(LocalDate deliveryDate, LocalDate orderDate,
                                     List<DetailChange> changes,
                                     List<OrderDetail> details) {
        List<ErrorInfo> errors = new ArrayList<>();
        if (deliveryDate == null || deliveryDate.isBefore(orderDate)
                || !calendar.isBusinessDay(deliveryDate)) {
            errors.add(ErrorInfo.of("deliveryDate",
                    "納品希望日は受注日以降の営業日を指定してください。"));
        }
        for (DetailChange change : changes) {
            if (find(details, change.getLineNo()) == null) {
                errors.add(ErrorInfo.of("lineNo",
                        "受注明細が存在しません。（明細" + change.getLineNo() + "）"));
                continue;
            }
            BigDecimal qty = change.getOrderQty();
            if (qty == null || qty.compareTo(BigDecimal.ZERO) <= 0) {
                errors.add(ErrorInfo.of("orderQty",
                        "受注数量は0より大きい値を入力してください。"));
            }
        }
        return errors;
    }

    private static OrderDetail find(List<OrderDetail> details, int lineNo) {
        for (OrderDetail detail : details) {
            if (detail.getLineNo() == lineNo) {
                return detail;
            }
        }
        return null;
    }
}
'''


ORDER: dict[str, str] = {
    "order/OrderRoute.java": _ORDER_ROUTE,
    "order/EdiRecord.java": _EDI_RECORD,
    "order/ImportError.java": _IMPORT_ERROR,
    "order/ImportSummary.java": _IMPORT_SUMMARY,
    "order/OrderSearchCondition.java": _ORDER_SEARCH_CONDITION,
    "order/OrderSummary.java": _ORDER_SUMMARY,
    "order/OrderPage.java": _ORDER_PAGE,
    "order/DetailChange.java": _DETAIL_CHANGE,
    "order/UpdateResult.java": _UPDATE_RESULT,
    "order/repository/EdiRecvRepository.java": _EDI_RECV_REPOSITORY,
    "order/batch/EdiOrderImportBatch.java": _EDI_IMPORT,
    "order/service/OrderSearchService.java": _ORDER_SEARCH,
    "order/service/OrderUpdateService.java": _ORDER_UPDATE,
}
