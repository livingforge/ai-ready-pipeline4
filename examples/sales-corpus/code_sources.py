"""手書きの Java ソース（サービス・共通部品・実行サンプル）。

`code_impl.py` が表から機械的に組む値クラス・リポジトリ interface と違い、
こちらは**中身のあるコード**なので 1 本ずつ書く。`code_impl` からは
``HAND_WRITTEN`` を読むだけで、依存を一方向に保つため逆参照はしない。

import 文まで含めて丸ごと書いてある。``code_impl.verify()`` が
「import 先が存在しない」「import せずに使っている」を生成時に突き合わせる。
"""

from __future__ import annotations

# ── 受注登録（C1: 引当を呼ばない / C4: 明細上限が設計書と食い違う / G6: 前ゼロ埋め）──
_ORDER_REGIST = '''package jp.co.contoso.sps.order.service;

import java.math.BigDecimal;
import java.math.RoundingMode;
import java.time.LocalDate;
import java.util.ArrayList;
import java.util.List;

import jp.co.contoso.sps.common.AuditLogger;
import jp.co.contoso.sps.common.BusinessDayCalendar;
import jp.co.contoso.sps.common.CreditClient;
import jp.co.contoso.sps.common.CreditTimeoutException;
import jp.co.contoso.sps.common.Customer;
import jp.co.contoso.sps.common.NumberingService;
import jp.co.contoso.sps.common.TaxCalculator;
import jp.co.contoso.sps.common.repository.CustomerRepository;
import jp.co.contoso.sps.common.repository.PriceRepository;
import jp.co.contoso.sps.framework.Service;
import jp.co.contoso.sps.framework.Transactional;
import jp.co.contoso.sps.order.Amount;
import jp.co.contoso.sps.order.ErrorInfo;
import jp.co.contoso.sps.order.OrderDetailDto;
import jp.co.contoso.sps.order.OrderDto;
import jp.co.contoso.sps.order.OrderResult;
import jp.co.contoso.sps.order.OrderRoute;
import jp.co.contoso.sps.order.OrderStatus;
import jp.co.contoso.sps.order.repository.OrderRepository;

/**
 * 受注登録サービス。
 *
 * 受注入力画面からの登録要求を受け、検証・金額計算・与信確認を経て受注を登録する。
 *
 * 関連する機能: 受注登録
 * 関連する画面: 受注入力
 * 関連する業務ルール: 与信限度額の判定 / 販売単価の決定 / 受注数量の検証
 *
 * 在庫の引当は本サービスでは行わない。引当は出荷指示の実行時に在庫引当サービスが行う。
 */
@Service
public class OrderRegistService {

    /**
     * 受注明細の上限行数。
     *
     * 詳細設計では 1〜50 行と定義されているが、EDI 取込で 50 行を超える発注が
     * 実際に届いたため実装で 100 行へ引き上げた。設計書は未改訂。
     */
    private static final int MAX_DETAIL_ROWS = 100;

    /** 受注数量の整数部の桁数。 */
    private static final int QTY_INTEGER_DIGITS = 7;

    /** 受注数量の小数部の桁数。 */
    private static final int QTY_SCALE = 2;

    /** 得意先コードの桁数。現行システムの 6 桁を移行時に 8 桁へ変換した。 */
    private static final int CUSTOMER_CD_LENGTH = 8;

    private final OrderRepository orderRepository;
    private final CustomerRepository customerRepository;
    private final PriceRepository priceRepository;
    private final CreditClient creditClient;
    private final NumberingService numberingService;
    private final TaxCalculator taxCalculator;
    private final BusinessDayCalendar calendar;
    private final AuditLogger auditLogger;

    public OrderRegistService(OrderRepository orderRepository,
                              CustomerRepository customerRepository,
                              PriceRepository priceRepository,
                              CreditClient creditClient,
                              NumberingService numberingService,
                              TaxCalculator taxCalculator,
                              BusinessDayCalendar calendar,
                              AuditLogger auditLogger) {
        this.orderRepository = orderRepository;
        this.customerRepository = customerRepository;
        this.priceRepository = priceRepository;
        this.creditClient = creditClient;
        this.numberingService = numberingService;
        this.taxCalculator = taxCalculator;
        this.calendar = calendar;
        this.auditLogger = auditLogger;
    }

    /**
     * 受注 1 件を登録する。
     *
     * 検証・単価決定・金額計算・与信確認を順に行い、受注ヘッダと明細を登録する。
     * 与信限度額を超える場合はステータスを与信保留（20）として登録する。
     * 本メソッドの開始から終了までを 1 トランザクションとする。
     */
    @Transactional
    public OrderResult registerOrder(OrderDto dto) {
        List<ErrorInfo> errors = validateOrder(dto);
        if (!errors.isEmpty()) {
            return OrderResult.error(errors);
        }

        // 受注経路の指定が無ければ画面入力（1）とみなす。EDI 取込は取込バッチが
        // EDI（2）を立ててから呼ぶ。
        if (dto.getRoute() == null) {
            dto.setRoute(OrderRoute.SCREEN);
        }

        for (OrderDetailDto detail : dto.getDetails()) {
            BigDecimal unitPrice = resolveUnitPrice(
                    dto.getCustomerCd(), detail.getProductCd(), dto.getOrderDate());
            detail.setUnitPrice(unitPrice);
        }

        Amount amount = calcAmount(dto.getDetails());
        OrderStatus status = checkCredit(dto.getCustomerCd(), amount.getTotal())
                ? OrderStatus.ACCEPTED
                : OrderStatus.CREDIT_HOLD;

        String orderNo = numberingService.next("ORDER");
        orderRepository.insertHeader(orderNo, dto, amount, status);
        orderRepository.insertDetails(orderNo, dto.getDetails());
        auditLogger.record(dto.getEntryStaffCd(), "受注登録", orderNo, null,
                status.getLabel());

        // 引当はここでは行わない（出荷指示の実行時に行う）
        return OrderResult.of(orderNo, status);
    }

    /**
     * 得意先・商品・数量・納品希望日の入力値を検証し、エラー一覧を返す。
     *
     * 関連する業務ルール: 受注数量の検証 / 納品希望日の検証 / 取引停止得意先の受注拒否
     */
    public List<ErrorInfo> validateOrder(OrderDto dto) {
        List<ErrorInfo> errors = new ArrayList<>();

        // 得意先コードは前ゼロを詰めて 8 桁に正規化する。EDI 経由の発注データが
        // 前ゼロを落として届くため。設計書には記載が無い。
        dto.setCustomerCd(padCustomerCd(dto.getCustomerCd()));

        Customer customer = customerRepository.find(dto.getCustomerCd());
        if (customer == null) {
            errors.add(ErrorInfo.of("customerCd", "得意先コードを入力してください。"));
        } else if (customer.isSuspended()) {
            errors.add(ErrorInfo.of("customerCd",
                    "得意先「" + customer.getName() + "」は取引停止中のため受注できません。"));
        }

        if (dto.getDeliveryDate() == null
                || dto.getDeliveryDate().isBefore(dto.getOrderDate())
                || !calendar.isBusinessDay(dto.getDeliveryDate())) {
            errors.add(ErrorInfo.of("deliveryDate",
                    "納品希望日は受注日以降の営業日を指定してください。"));
        }

        if (dto.getDetails().isEmpty() || dto.getDetails().size() > MAX_DETAIL_ROWS) {
            errors.add(ErrorInfo.of("details", "明細行数が上限を超えています。"));
        }

        for (OrderDetailDto detail : dto.getDetails()) {
            BigDecimal qty = detail.getOrderQty();
            if (qty == null || qty.compareTo(BigDecimal.ZERO) <= 0) {
                errors.add(ErrorInfo.of("orderQty", "受注数量は0より大きい値を入力してください。"));
            } else if (qty.scale() > QTY_SCALE
                    || qty.precision() - qty.scale() > QTY_INTEGER_DIGITS) {
                errors.add(ErrorInfo.of("orderQty", "受注数量の桁数が不正です。"));
            }
        }
        return errors;
    }

    /**
     * 得意先別単価マスタを優先して販売単価を決定する。無ければ標準単価を返す。
     *
     * 関連する業務ルール: 販売単価の決定
     */
    public BigDecimal resolveUnitPrice(String customerCd, String productCd, LocalDate date) {
        BigDecimal contractPrice =
                priceRepository.findContractPrice(customerCd, productCd, date);
        if (contractPrice != null) {
            return contractPrice;
        }
        return priceRepository.findStandardPrice(productCd);
    }

    /**
     * 明細金額と消費税額を計算する。
     *
     * 明細金額は「受注数量 × 販売単価」で円未満を切り捨てる。消費税額は
     * 明細単位に計算した参考値であり、確定は請求締めで請求単位に行う。
     */
    public Amount calcAmount(List<OrderDetailDto> details) {
        BigDecimal total = BigDecimal.ZERO;
        BigDecimal tax = BigDecimal.ZERO;
        for (OrderDetailDto detail : details) {
            BigDecimal detailAmount = detail.getOrderQty()
                    .multiply(detail.getUnitPrice())
                    .setScale(0, RoundingMode.FLOOR);
            detail.setDetailAmount(detailAmount);
            total = total.add(detailAmount);
            tax = tax.add(taxCalculator.calcTax(detailAmount, detail.getTaxType(),
                    RoundingMode.FLOOR));
        }
        return new Amount(total, tax);
    }

    /**
     * 与信限度額を超えないかを判定する。
     *
     * 「売掛残高 + 未請求受注金額 + 今回受注金額」が与信限度額を超える場合は false。
     * 与信照会 API がタイムアウトした場合も false として与信保留に倒す。
     *
     * 関連する業務ルール: 与信限度額の判定
     */
    public boolean checkCredit(String customerCd, BigDecimal orderAmount) {
        Customer customer = customerRepository.find(customerCd);
        try {
            BigDecimal receivable = creditClient.fetchReceivable(customerCd);
            BigDecimal unbilled = orderRepository.sumUnbilled(customerCd);
            return receivable.add(unbilled).add(orderAmount)
                    .compareTo(customer.getCreditLimit()) <= 0;
        } catch (CreditTimeoutException e) {
            return false;
        }
    }

    private String padCustomerCd(String customerCd) {
        if (customerCd == null) {
            return null;
        }
        StringBuilder padded = new StringBuilder(customerCd);
        while (padded.length() < CUSTOMER_CD_LENGTH) {
            padded.insert(0, '0');
        }
        return padded.toString();
    }
}
'''

# ── 受注取消（C2: 期限ではなくステータスで判定）────────────────────────────
_ORDER_CANCEL = '''package jp.co.contoso.sps.order.service;

import java.time.LocalDateTime;

import jp.co.contoso.sps.common.AuditLogger;
import jp.co.contoso.sps.framework.Service;
import jp.co.contoso.sps.framework.Transactional;
import jp.co.contoso.sps.inventory.service.StockAllocationService;
import jp.co.contoso.sps.order.CancelResult;
import jp.co.contoso.sps.order.Order;
import jp.co.contoso.sps.order.OrderStatus;
import jp.co.contoso.sps.order.repository.OrderRepository;

/**
 * 受注取消サービス。
 *
 * 受注の取消可否を判定し、受注と明細を取消状態にして引当を解放する。
 *
 * 関連する機能: 受注取消
 * 関連する画面: 受注取消
 * 関連する業務ルール: 受注取消の期限 / 出荷指示済み受注の変更禁止 / 引当の解放
 *
 * 取消可能な条件は「受注ステータスが出荷指示済（40）より前であること」とする。
 * 要件定義の「受注日の翌営業日まで」は運用の実態と合わないため、日付では判定しない。
 */
@Service
public class OrderCancelService {

    private final OrderRepository orderRepository;
    private final StockAllocationService allocationService;
    private final AuditLogger auditLogger;

    public OrderCancelService(OrderRepository orderRepository,
                              StockAllocationService allocationService,
                              AuditLogger auditLogger) {
        this.orderRepository = orderRepository;
        this.allocationService = allocationService;
        this.auditLogger = auditLogger;
    }

    /**
     * 受注を取り消す。
     *
     * 受注・明細の更新と引当の解放を 1 トランザクションで行う。
     */
    @Transactional
    public CancelResult cancelOrder(String orderNo, String staffCd, String reason,
                                    String note, LocalDateTime updDatetime) {
        Order order = orderRepository.find(orderNo);
        if (order == null) {
            return CancelResult.error("受注番号を入力してください。");
        }
        if (!order.getUpdDatetime().equals(updDatetime)) {
            return CancelResult.error("他の利用者が更新しました。再度読み込んでください。");
        }
        if (!isCancelable(order.getStatus())) {
            return CancelResult.error("取消可能期限を過ぎているため取り消せません。");
        }
        if ("99".equals(reason) && (note == null || note.isBlank())) {
            return CancelResult.error("取消理由備考を入力してください。");
        }

        allocationService.releaseAllocation(orderNo);
        orderRepository.updateDetailStatus(orderNo, OrderStatus.CANCELED);
        orderRepository.updateHeaderCanceled(orderNo, reason, note);
        auditLogger.record(staffCd, "受注取消", orderNo, order.getStatus().getLabel(),
                OrderStatus.CANCELED.getLabel());
        return CancelResult.ok();
    }

    /**
     * 取消可否をステータスで判定する。
     *
     * 出荷指示済（40）以降と取消済（90）は取り消せない。受注日からの経過日数は
     * 判定に用いない。
     */
    public boolean isCancelable(OrderStatus status) {
        return status.getCode() < OrderStatus.SHIP_INSTRUCTED.getCode();
    }
}
'''

# ── 出荷指示（C1: ここで引当を呼ぶ）───────────────────────────────────────
_SHIPMENT = '''package jp.co.contoso.sps.order.service;

import java.time.LocalDate;
import java.util.ArrayList;
import java.util.List;

import jp.co.contoso.sps.common.AuditLogger;
import jp.co.contoso.sps.common.NumberingService;
import jp.co.contoso.sps.framework.Service;
import jp.co.contoso.sps.framework.Transactional;
import jp.co.contoso.sps.inventory.AllocationResult;
import jp.co.contoso.sps.inventory.service.StockAllocationService;
import jp.co.contoso.sps.order.Order;
import jp.co.contoso.sps.order.OrderStatus;
import jp.co.contoso.sps.order.ShipmentResult;
import jp.co.contoso.sps.order.repository.OrderRepository;
import jp.co.contoso.sps.order.repository.ShipmentRepository;

/**
 * 出荷指示サービス。
 *
 * 引当済みの受注から出荷指示を作成し、倉庫管理システムへ連携する。
 *
 * 関連する機能: 出荷指示
 * 関連する画面: 出荷指示
 * 関連する帳票: 出荷指示書
 * 関連する業務ルール: 与信限度額の判定
 *
 * 在庫の引当は本サービスから呼び出す。受注登録の時点では引当を行わない。
 * 与信保留（20）の受注は営業部長の承認があるまで引き当てないので、出荷指示の
 * 対象にも入れない。
 */
@Service
public class ShipmentInstructionService {

    private final ShipmentRepository shipmentRepository;
    private final OrderRepository orderRepository;
    private final StockAllocationService allocationService;
    private final NumberingService numberingService;
    private final AuditLogger auditLogger;

    public ShipmentInstructionService(ShipmentRepository shipmentRepository,
                                      OrderRepository orderRepository,
                                      StockAllocationService allocationService,
                                      NumberingService numberingService,
                                      AuditLogger auditLogger) {
        this.shipmentRepository = shipmentRepository;
        this.orderRepository = orderRepository;
        this.allocationService = allocationService;
        this.numberingService = numberingService;
        this.auditLogger = auditLogger;
    }

    /**
     * 選択された受注から出荷指示を作成する。
     *
     * 受注ごとに在庫を引き当て、全数を確保できた受注だけ出荷指示済（40）へ進める。
     * 引当が一部にとどまった受注は出荷指示を作成せず、不足を呼出元へ返す。
     * 出荷指示を出せる状態にない受注は引当も行わず、理由を返す。
     */
    @Transactional
    public List<ShipmentResult> createInstruction(List<String> orderNos,
                                                  LocalDate shipDate,
                                                  String warehouseCd) {
        List<ShipmentResult> results = new ArrayList<>();
        for (String orderNo : orderNos) {
            Order order = orderRepository.find(orderNo);
            if (order == null) {
                results.add(ShipmentResult.rejected(orderNo, "受注が存在しません。"));
                continue;
            }
            if (!isInstructable(order.getStatus())) {
                results.add(ShipmentResult.rejected(orderNo,
                        order.getStatus().getLabel() + "の受注は出荷指示できません。"));
                continue;
            }
            AllocationResult allocation = allocationService.allocate(orderNo, warehouseCd);
            if (allocation.hasShortage()) {
                results.add(ShipmentResult.shortage(orderNo, allocation.getShortages()));
                continue;
            }
            String shipmentNo = numberingService.next("SHIPMENT");
            shipmentRepository.insert(shipmentNo, orderNo, shipDate, warehouseCd);
            orderRepository.updateStatus(orderNo, OrderStatus.SHIP_INSTRUCTED);
            auditLogger.record("出荷指示", orderNo, order.getStatus().getLabel(),
                    OrderStatus.SHIP_INSTRUCTED.getLabel());
            results.add(ShipmentResult.ok(orderNo, shipmentNo));
        }
        return results;
    }

    /**
     * 出荷指示を出せる状態か。
     *
     * 受付（10）と引当済（30）だけを対象とする。与信保留（20）は承認待ちなので
     * 引き当てず、出荷指示済（40）と取消（90）は対象にならない。
     *
     * 関連する業務ルール: 与信限度額の判定
     */
    public boolean isInstructable(OrderStatus status) {
        return status == OrderStatus.ACCEPTED || status == OrderStatus.ALLOCATED;
    }
}
'''

# ── 在庫引当（C1: 呼出元は出荷指示 / 先入先出）──────────────────────────────
_ALLOCATION = '''package jp.co.contoso.sps.inventory.service;

import java.math.BigDecimal;
import java.util.ArrayList;
import java.util.Comparator;
import java.util.List;

import jp.co.contoso.sps.framework.Service;
import jp.co.contoso.sps.framework.Transactional;
import jp.co.contoso.sps.inventory.Allocation;
import jp.co.contoso.sps.inventory.AllocationResult;
import jp.co.contoso.sps.inventory.Shortage;
import jp.co.contoso.sps.inventory.Stock;
import jp.co.contoso.sps.inventory.repository.AllocationRepository;
import jp.co.contoso.sps.inventory.repository.StockRepository;
import jp.co.contoso.sps.order.DetailStatus;
import jp.co.contoso.sps.order.OrderDetail;
import jp.co.contoso.sps.order.OrderStatus;
import jp.co.contoso.sps.order.repository.OrderRepository;

/**
 * 在庫引当サービス。
 *
 * 受注明細に対して有効在庫から数量を確保し、在庫の引当済数量と引当テーブルを
 * 更新する。有効在庫が不足する場合は確保できた数量までを引き当て、明細
 * ステータスを一部引当とする。
 *
 * 関連する機能: 在庫引当
 * 関連する業務ルール: 先入先出 / 引当の解放 / 在庫マイナスの禁止
 *
 * 引当は在庫のロット単位に作る。業務ルール「先入先出」により入庫日の古い
 * ロットから順に確保し、どのロットから何個取ったかを引当テーブルに残す。
 *
 * 呼出元は出荷指示サービスである。受注登録時には呼び出されない。
 */
@Service
public class StockAllocationService {

    /** 在庫行のロック待ちの上限（秒）。 */
    private static final int LOCK_WAIT_SECONDS = 10;

    /** デッドロック検知時に自動で再試行する回数。 */
    private static final int DEADLOCK_RETRY = 1;

    private final StockRepository stockRepository;
    private final AllocationRepository allocationRepository;
    private final OrderRepository orderRepository;

    public StockAllocationService(StockRepository stockRepository,
                                  AllocationRepository allocationRepository,
                                  OrderRepository orderRepository) {
        this.stockRepository = stockRepository;
        this.allocationRepository = allocationRepository;
        this.orderRepository = orderRepository;
    }

    /**
     * 受注 1 件ぶんの引当を行う。
     *
     * 明細を商品コードの昇順に並べ替えてからロックを取得する（デッドロック回避）。
     * 明細ごとに、入庫日の古いロットから足りるところまで確保していく。
     * 1 明細でも異常が起きた場合は当該受注ぶんをロールバックする。
     */
    @Transactional
    public AllocationResult allocate(String orderNo, String warehouseCd) {
        List<OrderDetail> details = orderRepository.findUnallocatedDetails(orderNo);
        details.sort(Comparator.comparing(OrderDetail::getProductCd));

        List<Shortage> shortages = new ArrayList<>();
        for (OrderDetail detail : details) {
            BigDecimal required = detail.getOrderQty().subtract(detail.getAllocatedQty());
            BigDecimal allocated = allocateFromLots(orderNo, warehouseCd, detail, required);
            detail.setAllocatedQty(detail.getAllocatedQty().add(allocated));

            BigDecimal shortQty = detail.getOrderQty().subtract(detail.getAllocatedQty());
            if (shortQty.compareTo(BigDecimal.ZERO) > 0) {
                orderRepository.updateDetailStatus(detail,
                        detail.getAllocatedQty().compareTo(BigDecimal.ZERO) > 0
                                ? DetailStatus.PARTIAL
                                : DetailStatus.UNALLOCATED);
                shortages.add(new Shortage(detail.getProductCd(), shortQty));
            } else {
                orderRepository.updateDetailStatus(detail, DetailStatus.ALLOCATED);
            }
        }
        if (shortages.isEmpty()) {
            orderRepository.updateStatus(orderNo, OrderStatus.ALLOCATED);
        }
        return new AllocationResult(shortages);
    }

    /**
     * 引当可能な在庫を入庫日の古い順に取得する。
     *
     * 有効在庫は「実在庫数 − 引当済数量」。入庫日が同じ場合はロット番号の
     * 昇順とする。必要数に届いた時点で打ち切るので、返るのは実際に引き当てる
     * ぶんのロットだけである。
     *
     * 関連する業務ルール: 先入先出
     */
    public List<Stock> findAllocatableStock(String warehouseCd, String productCd,
                                            BigDecimal requiredQty) {
        List<Stock> lots = stockRepository.findLotsForUpdate(
                warehouseCd, productCd, LOCK_WAIT_SECONDS);
        lots.sort(Comparator.comparing(Stock::getReceiveDate)
                .thenComparing(Stock::getLotNo));

        List<Stock> allocatable = new ArrayList<>();
        BigDecimal collected = BigDecimal.ZERO;
        for (Stock lot : lots) {
            if (collected.compareTo(requiredQty) >= 0) {
                break;
            }
            if (lot.availableQty().compareTo(BigDecimal.ZERO) <= 0) {
                continue;
            }
            allocatable.add(lot);
            collected = collected.add(lot.availableQty());
        }
        return allocatable;
    }

    /**
     * 引当を解放する。ロットごとの引当済数量を在庫から減算し、引当テーブルの
     * 行を削除する。
     *
     * 関連する業務ルール: 引当の解放
     */
    @Transactional
    public void releaseAllocation(String orderNo) {
        for (Allocation allocation : allocationRepository.findByOrder(orderNo)) {
            stockRepository.subtractAllocatedQty(allocation.getWarehouseCd(),
                    allocation.getProductCd(), allocation.getLotNo(),
                    allocation.getAllocatedQty());
        }
        allocationRepository.deleteByOrder(orderNo);
    }

    /** 明細 1 行ぶんを古いロットから確保し、確保できた数量を返す。 */
    private BigDecimal allocateFromLots(String orderNo, String warehouseCd,
                                        OrderDetail detail, BigDecimal required) {
        BigDecimal allocated = BigDecimal.ZERO;
        for (Stock lot : findAllocatableStock(warehouseCd, detail.getProductCd(),
                required)) {
            BigDecimal rest = required.subtract(allocated);
            if (rest.compareTo(BigDecimal.ZERO) <= 0) {
                break;
            }
            BigDecimal take = rest.min(lot.availableQty());
            if (take.compareTo(BigDecimal.ZERO) <= 0) {
                continue;
            }
            stockRepository.addAllocatedQty(warehouseCd, detail.getProductCd(),
                    lot.getLotNo(), take);
            allocationRepository.insert(orderNo, detail.getLineNo(), warehouseCd,
                    detail.getProductCd(), lot.getLotNo(), take);
            allocated = allocated.add(take);
        }
        return allocated;
    }
}
'''

# ── 請求締め（C3: 請求単位・四捨五入 / G1: 締日区分）─────────────────────────
_BILLING_CLOSE = '''package jp.co.contoso.sps.billing.batch;

import java.math.BigDecimal;
import java.math.RoundingMode;
import java.time.LocalDate;
import java.util.ArrayList;
import java.util.List;

import jp.co.contoso.sps.billing.BatchResult;
import jp.co.contoso.sps.billing.CloseResult;
import jp.co.contoso.sps.billing.Invoice;
import jp.co.contoso.sps.billing.InvoiceStatus;
import jp.co.contoso.sps.billing.SalesLine;
import jp.co.contoso.sps.billing.SalesSummary;
import jp.co.contoso.sps.billing.TaxGroup;
import jp.co.contoso.sps.billing.repository.InvoiceRepository;
import jp.co.contoso.sps.billing.repository.SalesRepository;
import jp.co.contoso.sps.common.AuditLogger;
import jp.co.contoso.sps.common.BusinessDayCalendar;
import jp.co.contoso.sps.common.ClosingType;
import jp.co.contoso.sps.common.Customer;
import jp.co.contoso.sps.common.NumberingService;
import jp.co.contoso.sps.common.TaxCalculator;
import jp.co.contoso.sps.common.repository.ClosingDayRepository;
import jp.co.contoso.sps.common.repository.CustomerRepository;
import jp.co.contoso.sps.framework.Component;
import jp.co.contoso.sps.framework.Transactional;

/**
 * 請求締めバッチ。
 *
 * 締め対象の得意先ごとに締め期間の売上を集計し、消費税と前回請求残高を加味して
 * 請求金額を確定する。
 *
 * 関連する機能: 請求締め
 * 関連する画面: 請求締め処理
 * 関連する業務ルール: 請求の締め日 / 請求金額の算出 / 消費税の計算
 *
 * 毎日 21:00 に起動し、得意先マスタの締日区分に従って当日が締め日にあたる
 * 得意先だけを対象とする。得意先単位でコミットし、異常時は当該得意先のみ
 * ロールバックして次の得意先へ進む。
 */
@Component
public class BillingCloseBatch {

    /** 並列度。得意先を 4 分割して処理する。 */
    private static final int PARTITION_COUNT = 4;

    /** 消費税の端数処理。請求単位で計算し円未満は四捨五入する。 */
    private static final RoundingMode TAX_ROUNDING = RoundingMode.HALF_UP;

    private final CustomerRepository customerRepository;
    private final InvoiceRepository invoiceRepository;
    private final SalesRepository salesRepository;
    private final TaxCalculator taxCalculator;
    private final NumberingService numberingService;
    private final BusinessDayCalendar calendar;
    private final ClosingDayRepository closingDayRepository;
    private final AuditLogger auditLogger;

    public BillingCloseBatch(CustomerRepository customerRepository,
                             InvoiceRepository invoiceRepository,
                             SalesRepository salesRepository,
                             TaxCalculator taxCalculator,
                             NumberingService numberingService,
                             BusinessDayCalendar calendar,
                             ClosingDayRepository closingDayRepository,
                             AuditLogger auditLogger) {
        this.customerRepository = customerRepository;
        this.invoiceRepository = invoiceRepository;
        this.salesRepository = salesRepository;
        this.taxCalculator = taxCalculator;
        this.numberingService = numberingService;
        this.calendar = calendar;
        this.closingDayRepository = closingDayRepository;
        this.auditLogger = auditLogger;
    }

    /**
     * 締め対象の得意先を順に処理し、請求ヘッダと請求明細を作成する。
     *
     * 実行日を当日として起動する入口。得意先コードを渡した場合はその得意先
     * だけを対象にする（画面「請求締め処理」からの再実行に使う）。
     */
    public BatchResult execute(String closingYm, List<String> customerCds) {
        return execute(closingYm, customerCds, LocalDate.now());
    }

    /**
     * 業務日付を指定して締める。
     *
     * 得意先単位でコミットし、異常時は当該得意先のみロールバックして次の
     * 得意先へ進む。締め期間は「前回の締め日の翌日」から「今回の締め日」まで。
     */
    public BatchResult execute(String closingYm, List<String> customerCds,
                               LocalDate businessDate) {
        int closed = 0;
        int skipped = 0;
        int failed = 0;
        for (Customer customer : selectTargets(businessDate, closingYm)) {
            if (customerCds != null && !customerCds.isEmpty()
                    && !customerCds.contains(customer.getCustomerCd())) {
                continue;
            }
            LocalDate to = closingDate(customer.getClosingType(), businessDate);
            LocalDate from = closingDate(customer.getClosingType(),
                    businessDate.minusMonths(1)).plusDays(1);
            try {
                CloseResult result = closeCustomer(customer, closingYm, from, to);
                if (result.isSkipped()) {
                    skipped++;
                } else {
                    closed++;
                }
            } catch (RuntimeException e) {
                failed++;
            }
        }
        auditLogger.record("請求締め", closingYm, null,
                closed + " 件確定 / " + skipped + " 件対象外 / " + failed + " 件異常");
        return new BatchResult(closed, skipped, failed);
    }

    /**
     * 締め対象の得意先を抽出する。
     *
     * 締日区分ごとに当日が締め日にあたるかを判定する。20 日および月末が休日に
     * あたる場合は前営業日を締め日とする。既に同一の請求年月で締め済みの得意先は
     * 対象から除外する。
     */
    public List<Customer> selectTargets(LocalDate businessDate, String closingYm) {
        List<Customer> targets = new ArrayList<>();
        for (Customer customer : customerRepository.findActive()) {
            if (!isClosingDay(customer.getClosingType(), businessDate)) {
                continue;
            }
            if (invoiceRepository.exists(customer.getCustomerCd(), closingYm)) {
                continue;
            }
            targets.add(customer);
        }
        return targets;
    }

    /**
     * 当日が締め日にあたるかを判定する。休日の場合は前営業日に締める。
     *
     * 関連する業務ルール: 請求の締め日
     */
    public boolean isClosingDay(ClosingType closingType, LocalDate businessDate) {
        return businessDate.equals(closingDate(closingType, businessDate));
    }

    /**
     * 基準日の属する月の締め日を返す。休日にあたる場合は前営業日とする。
     *
     * 関連する業務ルール: 請求の締め日
     */
    public LocalDate closingDate(ClosingType closingType, LocalDate businessDate) {
        LocalDate nominal = closingType.nominalClosingDate(businessDate,
                closingDay(closingType));
        return calendar.isBusinessDay(nominal)
                ? nominal
                : calendar.previousBusinessDay(nominal);
    }

    /**
     * 締日区分に対応する締め日（何日で締めるか）を締め日マスタから引く。
     *
     * 非機能要件「保守性」は締め日をプログラム修正なしに変更できることを
     * 求めるので、20 日・末日をプログラムに埋め込まない。
     *
     * 関連する非機能要件: 保守性
     */
    public int closingDay(ClosingType closingType) {
        Integer day = closingDayRepository.findClosingDay(closingType.getCode());
        if (day == null) {
            throw new IllegalStateException(
                    "締め日マスタが未登録です: 締日区分=" + closingType.getCode());
        }
        return day;
    }

    /**
     * 締め期間内の売上明細を集計し、売上額と消費税額を返す。
     *
     * 税抜金額を税率ごとに集計し、税率ごとに消費税額を計算して合算する
     * （請求単位・円未満四捨五入）。
     *
     * 関連する業務ルール: 消費税の計算
     */
    public SalesSummary aggregateSales(String customerCd, LocalDate from, LocalDate to) {
        List<SalesLine> lines = salesRepository.aggregate(customerCd, from, to);
        BigDecimal salesAmount = BigDecimal.ZERO;
        BigDecimal taxAmount = BigDecimal.ZERO;
        for (TaxGroup group : TaxGroup.groupBy(lines)) {
            salesAmount = salesAmount.add(group.getNetAmount());
            taxAmount = taxAmount.add(taxCalculator.calcTax(
                    group.getNetAmount(), group.getTaxType(), TAX_ROUNDING));
        }
        return new SalesSummary(salesAmount, taxAmount, lines);
    }

    /**
     * 得意先 1 件を締める。
     *
     * 請求金額は「前回請求残高 + 当月売上額 + 消費税額 − 当月入金額」で求める。
     * 請求金額と当月売上額がいずれも 0 の得意先は請求を作成しない。
     *
     * 作成した請求の入金額は 0 とする。ここでいう当月入金額は前回請求に対する
     * 入金であり、今回の請求へ充当された額ではないためである（今回ぶんの充当は
     * 入金消込サービスが積む）。
     *
     * 関連する業務ルール: 請求金額の算出 / 消費税の計算
     */
    @Transactional
    public CloseResult closeCustomer(Customer customer, String closingYm,
                                     LocalDate from, LocalDate to) {
        SalesSummary summary = aggregateSales(customer.getCustomerCd(), from, to);

        BigDecimal previous = invoiceRepository.previousBalance(customer.getCustomerCd());
        BigDecimal deposit =
                invoiceRepository.depositAmount(customer.getCustomerCd(), from, to);
        BigDecimal billing = previous.add(summary.getSalesAmount())
                .add(summary.getTaxAmount()).subtract(deposit);

        if (billing.compareTo(BigDecimal.ZERO) == 0
                && summary.getSalesAmount().compareTo(BigDecimal.ZERO) == 0) {
            return CloseResult.skipped(customer.getCustomerCd());
        }

        String invoiceNo = numberingService.next("INVOICE");
        invoiceRepository.insert(new Invoice(invoiceNo, closingYm,
                customer.getCustomerCd(), previous, summary.getSalesAmount(),
                summary.getTaxAmount(), BigDecimal.ZERO, billing,
                InvoiceStatus.CLOSED));
        salesRepository.writeBackInvoiceNo(summary.getLines(), invoiceNo);
        auditLogger.record("請求締め", invoiceNo, null, billing);
        return CloseResult.closed(invoiceNo, billing);
    }

    /** 並列度を返す。得意先を 4 分割して並列に処理する。 */
    public int partitionCount() {
        return PARTITION_COUNT;
    }
}
'''

# ── 共通部品 ───────────────────────────────────────────────────────────────
_TAX_CALCULATOR = '''package jp.co.contoso.sps.common;

import java.math.BigDecimal;
import java.math.RoundingMode;
import java.time.LocalDate;

import jp.co.contoso.sps.common.repository.TaxRateRepository;
import jp.co.contoso.sps.framework.Component;

/**
 * 消費税計算部品。
 *
 * 税区分に対応する税率を取得し、税抜金額から消費税額を計算する。
 * 計算の単位（明細単位か請求単位か）は呼出元が決める。本部品は与えられた
 * 金額に対して計算するだけである。状態を持たないのでシングルトンで共有できる。
 *
 * 関連する業務ルール: 消費税の計算
 */
@Component
public class TaxCalculator {

    /** 端数処理を指定しない場合の既定。 */
    private static final RoundingMode DEFAULT_ROUNDING = RoundingMode.HALF_UP;

    private final TaxRateRepository taxRateRepository;

    public TaxCalculator(TaxRateRepository taxRateRepository) {
        this.taxRateRepository = taxRateRepository;
    }

    /** 既定の端数処理で消費税額を計算する。 */
    public BigDecimal calcTax(BigDecimal amount, String taxType) {
        return calcTax(amount, taxType, DEFAULT_ROUNDING);
    }

    /**
     * 端数処理方式を指定して消費税額を計算する。
     *
     * 受注登録は切り捨て、請求締めは四捨五入を指定する。
     */
    public BigDecimal calcTax(BigDecimal amount, String taxType, RoundingMode rounding) {
        BigDecimal rate = getRate(taxType, LocalDate.now());
        return amount.multiply(rate).setScale(0, rounding);
    }

    /**
     * 適用日に有効な税率を返す。
     *
     * 税率はプログラムに埋め込まず税率マスタから取得する。該当が無ければ
     * 例外を送出する。
     */
    public BigDecimal getRate(String taxType, LocalDate date) {
        BigDecimal rate = taxRateRepository.find(taxType, date);
        if (rate == null) {
            throw new IllegalArgumentException("税率が取得できません: 税区分=" + taxType);
        }
        return rate;
    }
}
'''

# ── 採番部品（C5: ロック待ちが設計書と食い違う）──────────────────────────────
_NUMBERING = '''package jp.co.contoso.sps.common;

import java.time.LocalDate;
import java.time.format.DateTimeFormatter;

import jp.co.contoso.sps.common.repository.NumberingRepository;
import jp.co.contoso.sps.framework.Component;

/**
 * 採番部品。
 *
 * 採番区分ごとの連番を排他制御つきで取得し、書式を適用して返す。
 * 採番テーブルの該当行を SELECT FOR UPDATE でロックして加算する。
 * 呼出元のトランザクションに参加する（番号の欠番は許容する）。
 *
 * 関連する業務ルール: 業務キーの採番
 */
@Component
public class NumberingService {

    /**
     * 採番テーブルのロック待ちの上限（秒）。
     *
     * 詳細設計では 10 秒だが、月末の請求締めでロック待ちのタイムアウトが
     * 頻発したため実装で 30 秒へ延ばした。設計書は未改訂。
     */
    private static final int LOCK_TIMEOUT_SECONDS = 30;

    private static final DateTimeFormatter YMD = DateTimeFormatter.ofPattern("yyyyMMdd");
    private static final DateTimeFormatter YM = DateTimeFormatter.ofPattern("yyyyMM");

    private final NumberingRepository numberingRepository;

    public NumberingService(NumberingRepository numberingRepository) {
        this.numberingRepository = numberingRepository;
    }

    /**
     * 採番区分に応じた業務キーを発番する。
     *
     * 受注番号は「R + YYYYMMDD + 3 桁連番」、請求番号は「B + YYYYMM + 5 桁連番」。
     * 連番が桁あふれした場合は例外を送出する。
     */
    public String next(String numberingType) {
        LocalDate today = LocalDate.now();
        long seq = numberingRepository.incrementForUpdate(
                numberingType, today, LOCK_TIMEOUT_SECONDS);
        switch (numberingType) {
            case "ORDER":
                return "R" + today.format(YMD) + pad(seq, 3);
            case "SHIPMENT":
                return "S" + today.format(YMD) + pad(seq, 3);
            case "INVOICE":
                return "B" + today.format(YM) + pad(seq, 5);
            case "COUNT":
                return "C" + today.format(YMD) + pad(seq, 3);
            case "DEPOSIT":
                return "D" + today.format(YMD) + pad(seq, 4);
            default:
                throw new IllegalArgumentException("未知の採番区分: " + numberingType);
        }
    }

    private String pad(long seq, int digits) {
        String text = String.valueOf(seq);
        if (text.length() > digits) {
            throw new IllegalStateException("採番の連番が桁あふれしました: " + seq);
        }
        StringBuilder padded = new StringBuilder(text);
        while (padded.length() < digits) {
            padded.insert(0, '0');
        }
        return padded.toString();
    }
}
'''

_CALENDAR = '''package jp.co.contoso.sps.common;

import java.time.LocalDate;

import jp.co.contoso.sps.common.repository.CalendarRepository;
import jp.co.contoso.sps.framework.Component;

/**
 * 営業日カレンダー部品。
 *
 * 会社カレンダー（M_CALENDAR）を参照し、営業日の判定と算出を行う。
 * 土日祝日および会社が定める休日を除いた日を営業日とする。
 * 起動時に当年度と翌年度のカレンダーを読み込みキャッシュする。
 */
@Component
public class BusinessDayCalendar {

    private final CalendarRepository calendarRepository;

    public BusinessDayCalendar(CalendarRepository calendarRepository) {
        this.calendarRepository = calendarRepository;
    }

    /**
     * 指定日が営業日かどうかを返す。
     *
     * カレンダーが未登録の日付を指定された場合は例外を送出する。土日の判定で
     * 代替しない（休日を取りこぼすため）。
     */
    public boolean isBusinessDay(LocalDate date) {
        Boolean registered = calendarRepository.isBusinessDay(date);
        if (registered == null) {
            throw new IllegalStateException("カレンダーが未登録です: " + date);
        }
        return registered;
    }

    /** 基準日の翌営業日を返す。 */
    public LocalDate nextBusinessDay(LocalDate base) {
        LocalDate date = base.plusDays(1);
        while (!isBusinessDay(date)) {
            date = date.plusDays(1);
        }
        return date;
    }

    /** 基準日から N 営業日後を返す。 */
    public LocalDate addBusinessDays(LocalDate base, int days) {
        LocalDate date = base;
        for (int i = 0; i < days; i++) {
            date = nextBusinessDay(date);
        }
        return date;
    }

    /** 基準日の前営業日を返す。締め日が休日の場合に使う。 */
    public LocalDate previousBusinessDay(LocalDate base) {
        LocalDate date = base.minusDays(1);
        while (!isBusinessDay(date)) {
            date = date.minusDays(1);
        }
        return date;
    }
}
'''

HAND_WRITTEN: dict[str, str] = {
    "order/service/OrderRegistService.java": _ORDER_REGIST,
    "order/service/OrderCancelService.java": _ORDER_CANCEL,
    "order/service/ShipmentInstructionService.java": _SHIPMENT,
    "inventory/service/StockAllocationService.java": _ALLOCATION,
    "billing/batch/BillingCloseBatch.java": _BILLING_CLOSE,
    "common/TaxCalculator.java": _TAX_CALCULATOR,
    "common/NumberingService.java": _NUMBERING,
    "common/BusinessDayCalendar.java": _CALENDAR,
}
