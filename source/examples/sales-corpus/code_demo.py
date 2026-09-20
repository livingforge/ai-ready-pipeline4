"""配布サンプルを動かすための Java ソース（`demo` パッケージ）。

DB もアプリケーションサーバも無しに `java` コマンドだけで動かせるよう、
リポジトリのメモリ実装・サービスの配線・実行用の `Main` を置く。
**設計書に対応する成果物ではない**ので、`資料/` 側には対応する仕様書が無い。

`Main` は受注登録から請求締め・入金消込までを通し、`資料/` の設計書と実装の
食い違い（README の C1〜C5・G6）を**実行結果として目に見える形**にする。
"""

from __future__ import annotations

_IN_MEMORY = '''package jp.co.contoso.sps.demo;

import java.math.BigDecimal;
import java.time.DayOfWeek;
import java.time.LocalDate;
import java.time.LocalDateTime;
import java.util.ArrayList;
import java.util.Comparator;
import java.util.HashMap;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

import jp.co.contoso.sps.billing.Deposit;
import jp.co.contoso.sps.billing.Invoice;
import jp.co.contoso.sps.billing.InvoiceStatus;
import jp.co.contoso.sps.billing.JournalLine;
import jp.co.contoso.sps.billing.SalesLine;
import jp.co.contoso.sps.billing.repository.DepositRepository;
import jp.co.contoso.sps.billing.repository.InvoiceRepository;
import jp.co.contoso.sps.billing.repository.JournalRepository;
import jp.co.contoso.sps.billing.repository.SalesRepository;
import jp.co.contoso.sps.common.AuditLog;
import jp.co.contoso.sps.common.ClosingType;
import jp.co.contoso.sps.common.CreditClient;
import jp.co.contoso.sps.common.Customer;
import jp.co.contoso.sps.common.PasswordHasher;
import jp.co.contoso.sps.common.Product;
import jp.co.contoso.sps.common.Role;
import jp.co.contoso.sps.common.Staff;
import jp.co.contoso.sps.common.Warehouse;
import jp.co.contoso.sps.common.repository.AuditLogRepository;
import jp.co.contoso.sps.common.repository.CalendarRepository;
import jp.co.contoso.sps.common.repository.ClosingDayRepository;
import jp.co.contoso.sps.common.repository.CustomerRepository;
import jp.co.contoso.sps.common.repository.NumberingRepository;
import jp.co.contoso.sps.common.repository.PriceRepository;
import jp.co.contoso.sps.common.repository.ProductRepository;
import jp.co.contoso.sps.common.repository.StaffRepository;
import jp.co.contoso.sps.common.repository.TaxRateRepository;
import jp.co.contoso.sps.common.repository.WarehouseRepository;
import jp.co.contoso.sps.inventory.Allocation;
import jp.co.contoso.sps.inventory.CountLine;
import jp.co.contoso.sps.inventory.Stock;
import jp.co.contoso.sps.inventory.StockHistory;
import jp.co.contoso.sps.inventory.repository.AllocationRepository;
import jp.co.contoso.sps.inventory.repository.InventoryCountRepository;
import jp.co.contoso.sps.inventory.repository.StockHistoryRepository;
import jp.co.contoso.sps.inventory.repository.StockRepository;
import jp.co.contoso.sps.order.Amount;
import jp.co.contoso.sps.order.DetailStatus;
import jp.co.contoso.sps.order.EdiRecord;
import jp.co.contoso.sps.order.Order;
import jp.co.contoso.sps.order.OrderDetail;
import jp.co.contoso.sps.order.OrderDetailDto;
import jp.co.contoso.sps.order.OrderDto;
import jp.co.contoso.sps.order.OrderSearchCondition;
import jp.co.contoso.sps.order.OrderStatus;
import jp.co.contoso.sps.order.OrderSummary;
import jp.co.contoso.sps.order.repository.EdiRecvRepository;
import jp.co.contoso.sps.order.repository.OrderRepository;
import jp.co.contoso.sps.order.repository.ShipmentRepository;

/**
 * メモリ上のリポジトリ実装。
 *
 * DB を用意せずにサンプルを動かすための足場で、設計書に対応する成果物ではない。
 * 実際の実装は MyBatis のマッパーになる。
 */
public class InMemoryRepositories {

    private final Map<String, Customer> customers = new LinkedHashMap<>();
    private final Map<String, Product> products = new LinkedHashMap<>();
    private final Map<String, Warehouse> warehouses = new LinkedHashMap<>();
    private final Map<String, BigDecimal> receivables = new HashMap<>();
    private final Map<String, BigDecimal> contractPrices = new HashMap<>();
    private final Map<String, BigDecimal> taxRates = new HashMap<>();
    private final Map<String, Integer> closingDays = new HashMap<>();
    private final Map<String, Long> counters = new HashMap<>();
    private final Map<String, Order> orders = new LinkedHashMap<>();
    private final Map<String, List<OrderDetail>> orderDetails = new LinkedHashMap<>();
    private final List<Stock> stocks = new ArrayList<>();
    private final List<Allocation> allocations = new ArrayList<>();
    private final List<StockHistory> stockHistories = new ArrayList<>();
    private final List<CountLine> countLines = new ArrayList<>();
    private final List<String> confirmedCounts = new ArrayList<>();
    private final List<SalesLine> salesLines = new ArrayList<>();
    private final Map<String, Invoice> invoices = new LinkedHashMap<>();
    private final Map<String, Deposit> deposits = new LinkedHashMap<>();
    private final Map<String, EdiRecord> ediRecords = new LinkedHashMap<>();
    private final Map<String, Staff> staffs = new LinkedHashMap<>();
    private final List<AuditLog> auditLogs = new ArrayList<>();
    private final List<JournalLine> journals = new ArrayList<>();
    private final PasswordHasher passwordHasher = new PasswordHasher();

    public InMemoryRepositories() {
        customers.put("00010001", new Customer("00010001", "中央フードサービス",
                new BigDecimal("5000000"), ClosingType.TWENTIETH, false));
        customers.put("00010002", new Customer("00010002", "山下商店",
                new BigDecimal("100000"), ClosingType.MONTH_END, false));
        customers.put("00010003", new Customer("00010003", "取引停止中の得意先",
                new BigDecimal("1000000"), ClosingType.TWENTIETH, true));

        receivables.put("00010001", new BigDecimal("0"));
        receivables.put("00010002", new BigDecimal("80000"));
        receivables.put("00010003", new BigDecimal("0"));

        addProduct("4901234001", "こだわり醤油 1L", "4901234000015", "3400.00", "1");
        addProduct("4901234002", "food研 だし 500ml", "4901234000022", "980.25", "1");
        addProduct("4901234003", "冷凍うどん 5食", "4901234000039", "1500.00", "2");
        addProduct("4901234055", "有機味噌 750g", "4901234000558", "2001.00", "1");
        contractPrices.put("00010001/4901234001", new BigDecimal("3335.00"));

        warehouses.put("0102", new Warehouse("0102", "東京常温倉庫", "1", false));
        warehouses.put("0201", new Warehouse("0201", "大阪冷凍倉庫", "3", false));

        taxRates.put("1", new BigDecimal("0.10"));
        taxRates.put("2", new BigDecimal("0.08"));

        // 締め日マスタ（M_CLOSING_DAY）。非機能要件「保守性」により、締め日は
        // プログラムではなくここを直せば変えられる。末日締めは 31 を入れておくと
        // 締め日の算出側が月の末日へ丸める。
        closingDays.put("1", 20);
        closingDays.put("2", 31);

        stocks.add(new Stock("0102", "4901234001", "L001", LocalDate.of(2026, 1, 5),
                new BigDecimal("100"), BigDecimal.ZERO));
        stocks.add(new Stock("0102", "4901234001", "L002", LocalDate.of(2026, 1, 10),
                new BigDecimal("50"), BigDecimal.ZERO));
        stocks.add(new Stock("0102", "4901234055", "L003", LocalDate.of(2026, 1, 6),
                new BigDecimal("100"), BigDecimal.ZERO));
        stocks.add(new Stock("0102", "4901234002", "L004", LocalDate.of(2026, 1, 8),
                new BigDecimal("40"), BigDecimal.ZERO));
        // 4901234003 は在庫不足シナリオ専用の商品。合計 2 しか置かないので
        // 受注数量 3 は一部引当になる。2 ロットに分けてあるのは、入庫日の古い
        // L005 から先に引き当てること（先入先出）を確かめるため。
        stocks.add(new Stock("0102", "4901234003", "L005", LocalDate.of(2026, 1, 7),
                new BigDecimal("1"), BigDecimal.ZERO));
        stocks.add(new Stock("0102", "4901234003", "L006", LocalDate.of(2026, 1, 12),
                new BigDecimal("1"), BigDecimal.ZERO));

        // EDI受信ワーク（W_EDI_RECV）。1 件目は取り込めるもの、2 件目は
        // JAN コードが商品マスタに無くエラーになるもの。
        addEdiRecord("E20260115001", "010001", "4901234000015", "8");
        addEdiRecord("E20260115002", "010001", "9999999999999", "2");

        // 社員マスタ（M_STAFF）。パスワード変更日を実行日から数えているのは、
        // いつ動かしても「90 日ごとに変更」の判定が同じ結果になるようにするため。
        addStaff("100001", "山田太郎", Role.SALES, "Sales#2026", LocalDate.now());
        addStaff("100002", "佐藤花子", Role.WAREHOUSE, "Ware#2026a", LocalDate.now());
        addStaff("100003", "鈴木一郎", Role.ACCOUNTING, "Acct#2026b", LocalDate.now());
        addStaff("100099", "高橋三郎", Role.ADMIN, "Admin#2026", LocalDate.now());
        // 変更から 120 日たった利用者。ログインはできるが変更を求められる。
        addStaff("100004", "田中次郎", Role.SALES, "Old#20251",
                LocalDate.now().minusDays(120));
    }

    private void addProduct(String productCd, String productName, String janCd,
                            String stdPrice, String taxType) {
        products.put(productCd, new Product(productCd, productName, janCd, "本",
                new BigDecimal("12"), new BigDecimal(stdPrice), taxType, "1", false));
    }

    private void addEdiRecord(String recvNo, String ediCustomerCd, String janCd,
                              String qty) {
        ediRecords.put(recvNo, new EdiRecord(recvNo, ediCustomerCd, janCd,
                new BigDecimal(qty), LocalDate.of(2026, 1, 15),
                LocalDate.of(2026, 1, 19), "未処理", null, null));
    }

    /** 社員を 1 件積む。パスワードは平文では持たず、ソルト付きのハッシュにする。 */
    private void addStaff(String staffCd, String staffName, Role role, String password,
                          LocalDate passwordChangedOn) {
        String salt = passwordHasher.newSalt();
        staffs.put(staffCd, new Staff(staffCd, staffName, role,
                passwordHasher.hash(password, salt), salt, passwordChangedOn));
    }

    /** 締め対象の売上を積む（本来は出荷実績の取込で積まれる）。 */
    public void seedSales(String customerCd, LocalDate date, String productCd,
                          BigDecimal netAmount, String taxType) {
        salesLines.add(new SalesLine(customerCd, date, productCd, netAmount, taxType, null));
    }

    /** 入金を積む（本来は全銀ネットからの受信で積まれる）。 */
    public void seedDeposit(String depositNo, LocalDate date, String customerCd,
                            BigDecimal amount, String payerName, String invoiceNo) {
        deposits.put(depositNo, new Deposit(depositNo, date, customerCd, amount,
                BigDecimal.ZERO, payerName, invoiceNo));
    }

    /** 受注ヘッダを取り出す（実行結果の確認用）。 */
    public Order order(String orderNo) {
        return orders.get(orderNo);
    }

    /** 登録済みの受注をすべて取り出す（受注一覧照会の表示用）。 */
    public List<Order> allOrders() {
        return new ArrayList<>(orders.values());
    }

    /** 在庫の引当済数量を取り出す（実行結果の確認用）。 */
    public BigDecimal allocatedQty(String productCd) {
        BigDecimal sum = BigDecimal.ZERO;
        for (Stock stock : stocks) {
            if (stock.getProductCd().equals(productCd)) {
                sum = sum.add(stock.getAllocatedQty());
            }
        }
        return sum;
    }

    /** 引当をロット単位で取り出す（先入先出の確認用）。 */
    public List<Allocation> allocationsOf(String orderNo) {
        List<Allocation> found = new ArrayList<>();
        for (Allocation allocation : allocations) {
            if (allocation.getOrderNo().equals(orderNo)) {
                found.add(allocation);
            }
        }
        return found;
    }

    /** 在庫移動履歴を取り出す（実行結果の確認用）。 */
    public List<StockHistory> stockHistories() {
        return new ArrayList<>(stockHistories);
    }

    /** 出力した仕訳を取り出す（実行結果の確認用）。 */
    public List<JournalLine> journals() {
        return new ArrayList<>(journals);
    }

    /** 監査ログの 1 件を書き換える（改ざん検知の確認用）。 */
    public void tamperAuditLog(int index, String afterValue) {
        AuditLog log = auditLogs.get(index);
        auditLogs.set(index, new AuditLog(log.getRecordedAt(), log.getStaffCd(),
                log.getOperation(), log.getTargetKey(), log.getBeforeValue(),
                afterValue, log.getPreviousHash(), log.getHash()));
    }

    /** 棚卸の入力を積む。 */
    public void seedCountLine(String countNo, String warehouseCd, String productCd,
                              String lotNo, BigDecimal bookQty, BigDecimal actualQty) {
        countLines.add(new CountLine(countNo, warehouseCd, productCd, lotNo,
                bookQty, actualQty));
    }

    public CustomerRepository customerRepository() {
        return new CustomerRepository() {
            @Override
            public Customer find(String customerCd) {
                Customer customer = customers.get(customerCd);
                return customer == null || customer.isDeleted() ? null : customer;
            }

            @Override
            public List<Customer> findActive() {
                List<Customer> active = new ArrayList<>();
                for (Customer customer : customers.values()) {
                    if (!customer.isSuspended() && !customer.isDeleted()) {
                        active.add(customer);
                    }
                }
                return active;
            }

            @Override
            public void save(Customer customer) {
                customers.put(customer.getCustomerCd(), customer);
            }

            @Override
            public void logicalDelete(String customerCd) {
                Customer customer = customers.get(customerCd);
                if (customer != null) {
                    customer.setDeleted(true);
                }
            }
        };
    }

    public ProductRepository productRepository() {
        return new ProductRepository() {
            @Override
            public Product find(String productCd) {
                Product product = products.get(productCd);
                return product == null || product.isDeleted() ? null : product;
            }

            @Override
            public Product findByJan(String janCd) {
                for (Product product : products.values()) {
                    if (!product.isDeleted() && product.getJanCd().equals(janCd)) {
                        return product;
                    }
                }
                return null;
            }

            @Override
            public List<Product> findActive() {
                List<Product> active = new ArrayList<>();
                for (Product product : products.values()) {
                    if (!product.isDeleted()) {
                        active.add(product);
                    }
                }
                return active;
            }

            @Override
            public void save(Product product) {
                products.put(product.getProductCd(), product);
            }

            @Override
            public void logicalDelete(String productCd) {
                Product product = products.get(productCd);
                if (product != null) {
                    product.setDeleted(true);
                }
            }
        };
    }

    public WarehouseRepository warehouseRepository() {
        return new WarehouseRepository() {
            @Override
            public Warehouse find(String warehouseCd) {
                Warehouse warehouse = warehouses.get(warehouseCd);
                return warehouse == null || warehouse.isDeleted() ? null : warehouse;
            }

            @Override
            public List<Warehouse> findActive() {
                List<Warehouse> active = new ArrayList<>();
                for (Warehouse warehouse : warehouses.values()) {
                    if (!warehouse.isDeleted()) {
                        active.add(warehouse);
                    }
                }
                return active;
            }

            @Override
            public void save(Warehouse warehouse) {
                warehouses.put(warehouse.getWarehouseCd(), warehouse);
            }

            @Override
            public void logicalDelete(String warehouseCd) {
                Warehouse warehouse = warehouses.get(warehouseCd);
                if (warehouse != null) {
                    warehouse.setDeleted(true);
                }
            }
        };
    }

    public PriceRepository priceRepository() {
        return new PriceRepository() {
            @Override
            public BigDecimal findContractPrice(String customerCd, String productCd,
                                                LocalDate date) {
                return contractPrices.get(customerCd + "/" + productCd);
            }

            @Override
            public BigDecimal findStandardPrice(String productCd) {
                Product product = products.get(productCd);
                return product == null ? null : product.getStdPrice();
            }
        };
    }

    public TaxRateRepository taxRateRepository() {
        return (taxType, date) -> taxRates.get(taxType);
    }

    public CalendarRepository calendarRepository() {
        return date -> date.getDayOfWeek() != DayOfWeek.SATURDAY
                && date.getDayOfWeek() != DayOfWeek.SUNDAY;
    }

    public ClosingDayRepository closingDayRepository() {
        return closingTypeCd -> closingDays.get(closingTypeCd);
    }

    public NumberingRepository numberingRepository() {
        return (numberingType, date, lockTimeoutSeconds) ->
                counters.merge(numberingType, 1L, Long::sum);
    }

    public CreditClient creditClient() {
        return customerCd -> receivables.getOrDefault(customerCd, BigDecimal.ZERO);
    }

    public ShipmentRepository shipmentRepository() {
        return (shipmentNo, orderNo, shipDate, warehouseCd) ->
                System.out.printf("    出荷指示を登録: %s（受注 %s / 倉庫 %s）%n",
                        shipmentNo, orderNo, warehouseCd);
    }

    public AuditLogRepository auditLogRepository() {
        return new AuditLogRepository() {
            @Override
            public void insert(AuditLog log) {
                auditLogs.add(log);
            }

            @Override
            public String lastHash() {
                return auditLogs.isEmpty()
                        ? null
                        : auditLogs.get(auditLogs.size() - 1).getHash();
            }

            @Override
            public List<AuditLog> findAll() {
                return new ArrayList<>(auditLogs);
            }

            @Override
            public int deleteBefore(LocalDate limit) {
                int before = auditLogs.size();
                auditLogs.removeIf(log -> log.getRecordedAt().toLocalDate()
                        .isBefore(limit));
                return before - auditLogs.size();
            }
        };
    }

    public OrderRepository orderRepository() {
        return new OrderRepository() {
            @Override
            public Order find(String orderNo) {
                // 本物のリポジトリ（SQL でマッピングする実装）は取得時点のスナップショットを
                // 返すので、この後 updateHeaderCanceled などで行を更新しても、呼出元が
                // 掴んでいるインスタンスまでは変わらない。ここで実体をそのまま返すと
                // 監査ログの「変更前ステータス」が変更後の値に化けるため、複製して返す。
                Order order = orders.get(orderNo);
                if (order == null) {
                    return null;
                }
                return new Order(order.getOrderNo(), order.getOrderDate(),
                        order.getCustomerCd(), order.getDeliveryDate(),
                        order.getTotalAmount(), order.getTaxAmount(),
                        order.getStatus(), order.getRoute(), order.getEntryStaffCd(),
                        order.getUpdDatetime(), order.getCancelReason(),
                        order.getCancelNote());
            }

            @Override
            public List<OrderDetail> findDetails(String orderNo) {
                return new ArrayList<>(orderDetails.getOrDefault(orderNo, List.of()));
            }

            @Override
            public OrderDetail findDetail(String orderNo, int lineNo) {
                for (OrderDetail detail : orderDetails.getOrDefault(orderNo, List.of())) {
                    if (detail.getLineNo() == lineNo) {
                        return detail;
                    }
                }
                return null;
            }

            @Override
            public List<OrderDetail> findUnallocatedDetails(String orderNo) {
                List<OrderDetail> found = new ArrayList<>();
                for (OrderDetail detail : orderDetails.getOrDefault(orderNo, List.of())) {
                    if (detail.getStatus() == DetailStatus.UNALLOCATED
                            || detail.getStatus() == DetailStatus.PARTIAL) {
                        found.add(detail);
                    }
                }
                return found;
            }

            @Override
            public BigDecimal sumUnbilled(String customerCd) {
                BigDecimal sum = BigDecimal.ZERO;
                for (Order order : orders.values()) {
                    if (order.getCustomerCd().equals(customerCd)
                            && order.getStatus() != OrderStatus.CANCELED) {
                        sum = sum.add(order.getTotalAmount());
                    }
                }
                return sum;
            }

            @Override
            public List<OrderSummary> search(OrderSearchCondition condition, int offset,
                                             int limit) {
                List<OrderSummary> rows = new ArrayList<>();
                for (Order order : orders.values()) {
                    if (!matches(order, condition)) {
                        continue;
                    }
                    Customer customer = customers.get(order.getCustomerCd());
                    rows.add(new OrderSummary(order.getOrderNo(), order.getOrderDate(),
                            order.getCustomerCd(),
                            customer == null ? "" : customer.getName(),
                            order.getTotalAmount(), order.getStatus(),
                            order.getRoute()));
                }
                rows.sort(Comparator.comparing(OrderSummary::getOrderNo));
                int from = Math.min(offset, rows.size());
                int to = Math.min(from + limit, rows.size());
                return new ArrayList<>(rows.subList(from, to));
            }

            @Override
            public int count(OrderSearchCondition condition) {
                int total = 0;
                for (Order order : orders.values()) {
                    if (matches(order, condition)) {
                        total++;
                    }
                }
                return total;
            }

            @Override
            public void insertHeader(String orderNo, OrderDto dto, Amount amount,
                                     OrderStatus status) {
                orders.put(orderNo, new Order(orderNo, dto.getOrderDate(),
                        dto.getCustomerCd(), dto.getDeliveryDate(), amount.getTotal(),
                        amount.getTax(), status, dto.getRoute(), dto.getEntryStaffCd(),
                        LocalDateTime.now(), null, null));
            }

            @Override
            public void insertDetails(String orderNo, List<OrderDetailDto> details) {
                List<OrderDetail> rows = new ArrayList<>();
                int lineNo = 1;
                for (OrderDetailDto dto : details) {
                    OrderDetail detail = new OrderDetail(orderNo, lineNo++,
                            dto.getProductCd(), dto.getTaxType(), dto.getOrderQty(),
                            DetailStatus.UNALLOCATED, BigDecimal.ZERO);
                    detail.setUnitPrice(dto.getUnitPrice());
                    detail.setDetailAmount(dto.getDetailAmount());
                    rows.add(detail);
                }
                orderDetails.put(orderNo, rows);
            }

            @Override
            public void updateStatus(String orderNo, OrderStatus status) {
                orders.get(orderNo).setStatus(status);
            }

            @Override
            public void updateDetailStatus(String orderNo, OrderStatus status) {
                for (OrderDetail detail : orderDetails.getOrDefault(orderNo, List.of())) {
                    detail.setStatus(DetailStatus.CANCELED);
                }
            }

            @Override
            public void updateDetailStatus(OrderDetail detail, DetailStatus status) {
                detail.setStatus(status);
            }

            @Override
            public void updateDetailQty(String orderNo, int lineNo, BigDecimal qty,
                                        BigDecimal amount) {
                OrderDetail detail = findDetail(orderNo, lineNo);
                if (detail != null) {
                    detail.setOrderQty(qty);
                    detail.setDetailAmount(amount);
                }
            }

            @Override
            public void updateHeader(String orderNo, LocalDate deliveryDate,
                                     BigDecimal totalAmount, OrderStatus status) {
                Order order = orders.get(orderNo);
                order.setDeliveryDate(deliveryDate);
                order.setTotalAmount(totalAmount);
                order.setStatus(status);
                order.setUpdDatetime(LocalDateTime.now());
            }

            @Override
            public void updateHeaderCanceled(String orderNo, String reason, String note) {
                Order order = orders.get(orderNo);
                order.setStatus(OrderStatus.CANCELED);
                order.setCancelReason(reason);
                order.setCancelNote(note);
            }
        };
    }

    /** 検索条件に合う受注か。未入力の条件は絞り込みに使わない。 */
    private static boolean matches(Order order, OrderSearchCondition condition) {
        if (condition.getOrderDateFrom() != null
                && order.getOrderDate().isBefore(condition.getOrderDateFrom())) {
            return false;
        }
        if (condition.getOrderDateTo() != null
                && order.getOrderDate().isAfter(condition.getOrderDateTo())) {
            return false;
        }
        if (condition.getCustomerCd() != null && !condition.getCustomerCd().isBlank()
                && !order.getCustomerCd().equals(condition.getCustomerCd())) {
            return false;
        }
        return condition.getStatus() == null || order.getStatus() == condition.getStatus();
    }

    public EdiRecvRepository ediRecvRepository() {
        return new EdiRecvRepository() {
            @Override
            public List<EdiRecord> findUnprocessed(LocalDate targetDate) {
                List<EdiRecord> found = new ArrayList<>();
                for (EdiRecord record : ediRecords.values()) {
                    if ("未処理".equals(record.getStatus())
                            && record.getOrderDate().equals(targetDate)) {
                        found.add(record);
                    }
                }
                return found;
            }

            @Override
            public List<EdiRecord> findByDate(LocalDate targetDate) {
                List<EdiRecord> found = new ArrayList<>();
                for (EdiRecord record : ediRecords.values()) {
                    if (record.getOrderDate().equals(targetDate)) {
                        found.add(record);
                    }
                }
                return found;
            }

            @Override
            public void markImported(String recvNo, String orderNo) {
                EdiRecord record = ediRecords.get(recvNo);
                record.setStatus("取込済");
                record.setOrderNo(orderNo);
                record.setErrorMessage(null);
            }

            @Override
            public void markError(String recvNo, String message) {
                EdiRecord record = ediRecords.get(recvNo);
                record.setStatus("エラー");
                record.setErrorMessage(message);
            }
        };
    }

    public StaffRepository staffRepository() {
        return new StaffRepository() {
            @Override
            public Staff find(String staffCd) {
                return staffs.get(staffCd);
            }

            @Override
            public List<Staff> findAll() {
                return new ArrayList<>(staffs.values());
            }

            @Override
            public void save(Staff staff) {
                staffs.put(staff.getStaffCd(), staff);
            }

            @Override
            public void updatePassword(String staffCd, String passwordHash,
                                       String passwordSalt, LocalDate changedOn) {
                Staff staff = staffs.get(staffCd);
                staff.setPasswordHash(passwordHash);
                staff.setPasswordSalt(passwordSalt);
                staff.setPasswordChangedOn(changedOn);
            }
        };
    }

    public StockRepository stockRepository() {
        return new StockRepository() {
            @Override
            public Stock find(String warehouseCd, String productCd, String lotNo) {
                for (Stock stock : stocks) {
                    if (stock.getWarehouseCd().equals(warehouseCd)
                            && stock.getProductCd().equals(productCd)
                            && stock.getLotNo().equals(lotNo)) {
                        return stock;
                    }
                }
                return null;
            }

            @Override
            public List<Stock> findByWarehouse(String warehouseCd) {
                List<Stock> found = new ArrayList<>();
                for (Stock stock : stocks) {
                    if (stock.getWarehouseCd().equals(warehouseCd)) {
                        found.add(stock);
                    }
                }
                return found;
            }

            @Override
            public List<Stock> findLotsForUpdate(String warehouseCd, String productCd,
                                                 int lockWaitSeconds) {
                List<Stock> lots = new ArrayList<>();
                for (Stock stock : stocks) {
                    if (stock.getWarehouseCd().equals(warehouseCd)
                            && stock.getProductCd().equals(productCd)) {
                        lots.add(stock);
                    }
                }
                return lots;
            }

            @Override
            public void insert(Stock stock) {
                stocks.add(stock);
            }

            @Override
            public void addStockQty(String warehouseCd, String productCd, String lotNo,
                                    BigDecimal qty) {
                Stock lot = find(warehouseCd, productCd, lotNo);
                lot.setStockQty(lot.getStockQty().add(qty));
            }

            @Override
            public void subtractStockQty(String warehouseCd, String productCd,
                                         String lotNo, BigDecimal qty) {
                Stock lot = find(warehouseCd, productCd, lotNo);
                lot.setStockQty(lot.getStockQty().subtract(qty));
            }

            @Override
            public void addAllocatedQty(String warehouseCd, String productCd,
                                        String lotNo, BigDecimal qty) {
                Stock lot = find(warehouseCd, productCd, lotNo);
                lot.setAllocatedQty(lot.getAllocatedQty().add(qty));
            }

            @Override
            public void subtractAllocatedQty(String warehouseCd, String productCd,
                                             String lotNo, BigDecimal qty) {
                Stock lot = find(warehouseCd, productCd, lotNo);
                lot.setAllocatedQty(lot.getAllocatedQty().subtract(qty));
            }
        };
    }

    public StockHistoryRepository stockHistoryRepository() {
        return new StockHistoryRepository() {
            @Override
            public void insert(StockHistory history) {
                stockHistories.add(history);
            }

            @Override
            public List<StockHistory> findByProduct(String warehouseCd,
                                                    String productCd) {
                List<StockHistory> found = new ArrayList<>();
                for (StockHistory history : stockHistories) {
                    if (history.getWarehouseCd().equals(warehouseCd)
                            && history.getProductCd().equals(productCd)) {
                        found.add(history);
                    }
                }
                return found;
            }
        };
    }

    public InventoryCountRepository inventoryCountRepository() {
        return new InventoryCountRepository() {
            @Override
            public void insert(List<CountLine> lines) {
                countLines.addAll(lines);
            }

            @Override
            public List<CountLine> findByCountNo(String countNo) {
                List<CountLine> found = new ArrayList<>();
                for (CountLine line : countLines) {
                    if (line.getCountNo().equals(countNo)) {
                        found.add(line);
                    }
                }
                return found;
            }

            @Override
            public List<String> findOpenCountNos() {
                List<String> open = new ArrayList<>();
                for (CountLine line : countLines) {
                    if (!confirmedCounts.contains(line.getCountNo())
                            && !open.contains(line.getCountNo())) {
                        open.add(line.getCountNo());
                    }
                }
                return open;
            }

            @Override
            public void markConfirmed(String countNo) {
                confirmedCounts.add(countNo);
            }
        };
    }

    public AllocationRepository allocationRepository() {
        return new AllocationRepository() {
            @Override
            public void insert(String orderNo, int lineNo, String warehouseCd,
                               String productCd, String lotNo, BigDecimal qty) {
                allocations.add(new Allocation(orderNo, lineNo, warehouseCd, productCd,
                        lotNo, qty));
            }

            @Override
            public List<Allocation> findByOrder(String orderNo) {
                List<Allocation> found = new ArrayList<>();
                for (Allocation allocation : allocations) {
                    if (allocation.getOrderNo().equals(orderNo)) {
                        found.add(allocation);
                    }
                }
                return found;
            }

            @Override
            public void deleteByOrder(String orderNo) {
                allocations.removeIf(a -> a.getOrderNo().equals(orderNo));
            }
        };
    }

    public InvoiceRepository invoiceRepository() {
        return new InvoiceRepository() {
            @Override
            public boolean exists(String customerCd, String closingYm) {
                for (Invoice invoice : invoices.values()) {
                    if (invoice.getCustomerCd().equals(customerCd)
                            && invoice.getClosingYm().equals(closingYm)) {
                        return true;
                    }
                }
                return false;
            }

            @Override
            public BigDecimal previousBalance(String customerCd) {
                BigDecimal sum = BigDecimal.ZERO;
                for (Invoice invoice : invoices.values()) {
                    if (invoice.getCustomerCd().equals(customerCd)
                            && invoice.getStatus() != InvoiceStatus.SETTLED) {
                        sum = sum.add(invoice.getUnpaidAmount());
                    }
                }
                return sum;
            }

            @Override
            public BigDecimal depositAmount(String customerCd, LocalDate from,
                                            LocalDate to) {
                BigDecimal sum = BigDecimal.ZERO;
                for (Deposit deposit : deposits.values()) {
                    if (customerCd.equals(deposit.getCustomerCd())
                            && !deposit.getDepositDate().isBefore(from)
                            && !deposit.getDepositDate().isAfter(to)) {
                        sum = sum.add(deposit.getAppliedAmount());
                    }
                }
                return sum;
            }

            @Override
            public void insert(Invoice invoice) {
                invoices.put(invoice.getInvoiceNo(), invoice);
                System.out.printf("    請求を登録: %s（%s / 税抜 %s / 消費税 %s / 請求 %s）%n",
                        invoice.getInvoiceNo(), invoice.getCustomerCd(),
                        invoice.getSalesAmount(), invoice.getTaxAmount(),
                        invoice.getBillingAmount());
            }

            @Override
            public Invoice find(String invoiceNo) {
                return invoices.get(invoiceNo);
            }

            @Override
            public List<Invoice> findAll() {
                return new ArrayList<>(invoices.values());
            }

            @Override
            public List<Invoice> findByClosingYm(String closingYm) {
                List<Invoice> found = new ArrayList<>();
                for (Invoice invoice : invoices.values()) {
                    if (invoice.getClosingYm().equals(closingYm)) {
                        found.add(invoice);
                    }
                }
                return found;
            }

            @Override
            public List<Invoice> findUnpaid(String customerCd) {
                List<Invoice> found = new ArrayList<>();
                for (Invoice invoice : invoices.values()) {
                    if (invoice.getCustomerCd().equals(customerCd)
                            && invoice.getUnpaidAmount().signum() > 0) {
                        found.add(invoice);
                    }
                }
                return found;
            }

            @Override
            public void updateStatus(String invoiceNo, InvoiceStatus status) {
                invoices.get(invoiceNo).setStatus(status);
            }

            @Override
            public void updateDeposit(String invoiceNo, BigDecimal appliedAmount,
                                      InvoiceStatus status) {
                Invoice invoice = invoices.get(invoiceNo);
                invoice.setDepositAmount(appliedAmount);
                invoice.setStatus(status);
            }
        };
    }

    public DepositRepository depositRepository() {
        return new DepositRepository() {
            @Override
            public List<Deposit> findByDate(LocalDate depositDate) {
                List<Deposit> found = new ArrayList<>();
                for (Deposit deposit : deposits.values()) {
                    if (deposit.getDepositDate().equals(depositDate)) {
                        found.add(deposit);
                    }
                }
                return found;
            }

            @Override
            public Deposit find(String depositNo) {
                return deposits.get(depositNo);
            }

            @Override
            public void updateApplied(String depositNo, String invoiceNo,
                                      BigDecimal amount) {
                Deposit deposit = deposits.get(depositNo);
                deposit.setAppliedAmount(deposit.getAppliedAmount().add(amount));
                deposit.setInvoiceNo(invoiceNo);
            }
        };
    }

    public JournalRepository journalRepository() {
        return (targetDate, lines) -> {
            journals.addAll(lines);
            System.out.printf("    仕訳ファイルを出力: %s（%d 行）%n", targetDate, lines.size());
        };
    }

    public SalesRepository salesRepository() {
        return new SalesRepository() {
            @Override
            public void insert(SalesLine line) {
                salesLines.add(line);
            }

            @Override
            public List<SalesLine> aggregate(String customerCd, LocalDate from,
                                             LocalDate to) {
                List<SalesLine> found = new ArrayList<>();
                for (SalesLine line : salesLines) {
                    if (line.getCustomerCd().equals(customerCd)
                            && line.getInvoiceNo() == null
                            && !line.getSalesDate().isBefore(from)
                            && !line.getSalesDate().isAfter(to)) {
                        found.add(line);
                    }
                }
                return found;
            }

            @Override
            public List<SalesLine> findByDate(LocalDate salesDate) {
                List<SalesLine> found = new ArrayList<>();
                for (SalesLine line : salesLines) {
                    if (line.getSalesDate().equals(salesDate)) {
                        found.add(line);
                    }
                }
                return found;
            }

            @Override
            public List<SalesLine> findByInvoiceNo(String invoiceNo) {
                List<SalesLine> found = new ArrayList<>();
                for (SalesLine line : salesLines) {
                    if (invoiceNo.equals(line.getInvoiceNo())) {
                        found.add(line);
                    }
                }
                return found;
            }

            @Override
            public void writeBackInvoiceNo(List<SalesLine> lines, String invoiceNo) {
                for (SalesLine line : lines) {
                    line.setInvoiceNo(invoiceNo);
                }
            }
        };
    }
}
'''


_SERVICES = '''package jp.co.contoso.sps.demo;

import jp.co.contoso.sps.billing.batch.BillingCloseBatch;
import jp.co.contoso.sps.billing.batch.SalesJournalExportBatch;
import jp.co.contoso.sps.billing.service.DepositMatchingService;
import jp.co.contoso.sps.billing.service.InvoicePrintService;
import jp.co.contoso.sps.billing.service.ReceivableInquiryService;
import jp.co.contoso.sps.common.AuditLogger;
import jp.co.contoso.sps.common.AuthService;
import jp.co.contoso.sps.common.BusinessDayCalendar;
import jp.co.contoso.sps.common.NumberingService;
import jp.co.contoso.sps.common.PasswordHasher;
import jp.co.contoso.sps.common.PasswordPolicy;
import jp.co.contoso.sps.common.TaxCalculator;
import jp.co.contoso.sps.common.service.MasterMaintenanceService;
import jp.co.contoso.sps.inventory.service.InventoryCountService;
import jp.co.contoso.sps.inventory.service.ShipmentResultImportService;
import jp.co.contoso.sps.inventory.service.StockAllocationService;
import jp.co.contoso.sps.inventory.service.StockInquiryService;
import jp.co.contoso.sps.inventory.service.StockUpdateService;
import jp.co.contoso.sps.order.batch.EdiOrderImportBatch;
import jp.co.contoso.sps.order.service.OrderCancelService;
import jp.co.contoso.sps.order.service.OrderRegistService;
import jp.co.contoso.sps.order.service.OrderSearchService;
import jp.co.contoso.sps.order.service.OrderUpdateService;
import jp.co.contoso.sps.order.service.ShipmentInstructionService;

/**
 * サービスの配線。
 *
 * 本番は Spring が組み立てるところを、配布サンプルでは手で組む。`Main` と
 * `WebMain` の両方が同じ配線を使えるようにここへまとめてある。
 * 設計書に対応する成果物ではない。
 */
public class Services {

    public final InMemoryRepositories repos;

    public final AuditLogger auditLogger;
    public final BusinessDayCalendar calendar;
    public final TaxCalculator taxCalculator;
    public final NumberingService numbering;

    public final AuthService auth;
    public final MasterMaintenanceService master;

    public final OrderRegistService regist;
    public final OrderCancelService cancel;
    public final OrderUpdateService update;
    public final OrderSearchService search;
    public final ShipmentInstructionService shipment;
    public final EdiOrderImportBatch ediImport;

    public final StockAllocationService allocation;
    public final StockUpdateService stockUpdate;
    public final InventoryCountService count;
    public final ShipmentResultImportService shipmentResult;
    public final StockInquiryService stockInquiry;

    public final BillingCloseBatch billing;
    public final InvoicePrintService invoicePrint;
    public final DepositMatchingService deposit;
    public final SalesJournalExportBatch journal;
    public final ReceivableInquiryService receivable;

    public Services(InMemoryRepositories repos) {
        this.repos = repos;
        this.auditLogger = new AuditLogger(repos.auditLogRepository());
        this.calendar = new BusinessDayCalendar(repos.calendarRepository());
        this.taxCalculator = new TaxCalculator(repos.taxRateRepository());
        this.numbering = new NumberingService(repos.numberingRepository());

        this.auth = new AuthService(repos.staffRepository(), new PasswordHasher(),
                new PasswordPolicy(), auditLogger);
        this.master = new MasterMaintenanceService(repos.customerRepository(),
                repos.productRepository(), repos.warehouseRepository(),
                repos.staffRepository(), auditLogger);

        this.allocation = new StockAllocationService(repos.stockRepository(),
                repos.allocationRepository(), repos.orderRepository());
        this.stockUpdate = new StockUpdateService(repos.stockRepository(),
                repos.stockHistoryRepository(), auditLogger);
        this.count = new InventoryCountService(repos.inventoryCountRepository(),
                stockUpdate, auditLogger);
        this.stockInquiry = new StockInquiryService(repos.stockRepository(),
                repos.productRepository());

        this.regist = new OrderRegistService(repos.orderRepository(),
                repos.customerRepository(), repos.priceRepository(),
                repos.creditClient(), numbering, taxCalculator, calendar, auditLogger);
        this.cancel = new OrderCancelService(repos.orderRepository(), allocation,
                auditLogger);
        this.update = new OrderUpdateService(repos.orderRepository(), calendar, regist,
                auditLogger);
        this.search = new OrderSearchService(repos.orderRepository());
        this.shipment = new ShipmentInstructionService(repos.shipmentRepository(),
                repos.orderRepository(), allocation, numbering, auditLogger);
        this.ediImport = new EdiOrderImportBatch(repos.ediRecvRepository(),
                repos.productRepository(), regist, auditLogger);

        this.shipmentResult = new ShipmentResultImportService(stockUpdate,
                repos.orderRepository(), repos.salesRepository(), auditLogger);

        this.billing = new BillingCloseBatch(repos.customerRepository(),
                repos.invoiceRepository(), repos.salesRepository(), taxCalculator,
                numbering, calendar, repos.closingDayRepository(), auditLogger);
        this.invoicePrint = new InvoicePrintService(repos.invoiceRepository(),
                repos.salesRepository(), repos.customerRepository(), auditLogger);
        this.deposit = new DepositMatchingService(repos.depositRepository(),
                repos.invoiceRepository(), auditLogger);
        this.journal = new SalesJournalExportBatch(repos.salesRepository(),
                repos.journalRepository(), taxCalculator, auditLogger);
        this.receivable = new ReceivableInquiryService(repos.invoiceRepository(),
                repos.customerRepository());
    }
}
'''


_MAIN = '''package jp.co.contoso.sps.demo;

import java.math.BigDecimal;
import java.math.RoundingMode;
import java.time.LocalDate;
import java.util.ArrayList;
import java.util.List;

import jp.co.contoso.sps.billing.BatchResult;
import jp.co.contoso.sps.billing.DepositCandidate;
import jp.co.contoso.sps.billing.Invoice;
import jp.co.contoso.sps.billing.InvoicePrintData;
import jp.co.contoso.sps.billing.MatchSummary;
import jp.co.contoso.sps.billing.Receivable;
import jp.co.contoso.sps.billing.SalesLine;
import jp.co.contoso.sps.billing.SalesSummary;
import jp.co.contoso.sps.common.BusinessException;
import jp.co.contoso.sps.common.ClosingType;
import jp.co.contoso.sps.inventory.Allocation;
import jp.co.contoso.sps.inventory.CountResult;
import jp.co.contoso.sps.inventory.MoveType;
import jp.co.contoso.sps.inventory.ShipmentResultDto;
import jp.co.contoso.sps.inventory.Shortage;
import jp.co.contoso.sps.inventory.StockMoveDto;
import jp.co.contoso.sps.inventory.StockView;
import jp.co.contoso.sps.order.CancelResult;
import jp.co.contoso.sps.order.DetailChange;
import jp.co.contoso.sps.order.EdiRecord;
import jp.co.contoso.sps.order.ImportSummary;
import jp.co.contoso.sps.order.OrderDetailDto;
import jp.co.contoso.sps.order.OrderDto;
import jp.co.contoso.sps.order.OrderPage;
import jp.co.contoso.sps.order.OrderResult;
import jp.co.contoso.sps.order.OrderSearchCondition;
import jp.co.contoso.sps.order.OrderSummary;
import jp.co.contoso.sps.order.ShipmentResult;
import jp.co.contoso.sps.order.UpdateResult;

/**
 * 新販売管理システムの動作確認サンプル。
 *
 * 受注登録から請求締め・入金消込・仕訳連携までを通し、設計書（../資料/）と
 * 実装の食い違いを実行結果として示す。DB は使わず、すべてメモリ上で動く。
 */
public class Main {

    private static final String WAREHOUSE = "0102";
    private static final LocalDate ORDER_DATE = LocalDate.of(2026, 1, 15);
    private static final LocalDate DELIVERY_DATE = LocalDate.of(2026, 1, 19);
    private static final String SALES_STAFF = "100001";
    private static final String WAREHOUSE_STAFF = "100002";
    private static final String ACCOUNTING_STAFF = "100003";

    public static void main(String[] args) {
        InMemoryRepositories repos = new InMemoryRepositories();
        Services services = new Services(repos);

        String orderA = scenario1(services, repos);
        scenario2(services);
        scenario3(services);
        scenario4(services, repos, orderA);
        scenario5(services, repos, orderA);
        scenario6(services, repos);
        scenario7(services);
        scenario8(services);
        scenario9(services);
        scenario10(services, repos);
        scenario11(services, repos);
        scenario12(services);
        scenario13(services);
        String invoiceNo = scenario14(services, repos);
        scenario15(services, invoiceNo);
        scenario16(services, invoiceNo);
        scenario17(services);
        scenario18(services);
        scenario19(services, repos);
        scenario20(services);
    }

    /** 与信枠内の受注を登録する。前ゼロ埋め・契約単価・円未満切り捨てを確認する。 */
    private static String scenario1(Services services, InMemoryRepositories repos) {
        title("1. 受注登録（与信枠内）");
        OrderDto dto = order("10001",
                detail("4901234001", "3", "1"),
                detail("4901234055", "5", "1"),
                detail("4901234002", "3", "1"));
        OrderResult result = services.regist.registerOrder(dto);
        System.out.println("    得意先コード: 入力 10001 → " + dto.getCustomerCd()
                + "（G6 前ゼロ埋め。設計書には記載が無い）");
        for (OrderDetailDto detail : dto.getDetails()) {
            System.out.printf("    %s  単価 %s × 数量 %s → 明細金額 %s%n",
                    detail.getProductCd(), detail.getUnitPrice(),
                    detail.getOrderQty(), detail.getDetailAmount());
        }
        System.out.println("    4901234002 は 980.25 × 3 = 2940.75 → 円未満切り捨てで 2940");
        System.out.println("    受注番号 " + result.getOrderNo()
                + " / ステータス " + result.getStatus().getCode()
                + "（" + result.getStatus().getLabel() + "）");
        return result.getOrderNo();
    }

    /** 与信限度額を超える受注は与信保留（20）になる。 */
    private static void scenario2(Services services) {
        title("2. 受注登録（与信限度額を超える）");
        OrderDto dto = order("00010002", detail("4901234001", "10", "1"));
        OrderResult result = services.regist.registerOrder(dto);
        System.out.println("    売掛残高 80,000 + 今回 34,000 > 与信限度額 100,000");
        System.out.println("    受注番号 " + result.getOrderNo()
                + " / ステータス " + result.getStatus().getCode()
                + "（" + result.getStatus().getLabel() + "）");
    }

    /** C4: 詳細設計は明細 50 行までだが、実装は 100 行まで通る。 */
    private static void scenario3(Services services) {
        title("3. 明細 60 行の受注登録（C4 コードと設計書の食い違い）");
        List<OrderDetailDto> details = new ArrayList<>();
        for (int i = 0; i < 60; i++) {
            details.add(detail("4901234001", "1", "1"));
        }
        OrderDto dto = new OrderDto("00010001", ORDER_DATE, DELIVERY_DATE, details);
        OrderResult result = services.regist.registerOrder(dto);
        System.out.println("    詳細設計「明細 1〜50 行」に対し実装の上限は 100 行");
        System.out.println("    登録できたか: " + result.isOk()
                + "（受注番号 " + result.getOrderNo() + "）");
    }

    /** C1: 引当は受注登録ではなく出荷指示の実行時に走る。 */
    private static void scenario4(Services services, InMemoryRepositories repos,
                                  String orderNo) {
        title("4. 出荷指示（C1 引当のタイミング）");
        System.out.println("    受注登録の直後の引当済数量（4901234001）: "
                + repos.allocatedQty("4901234001") + " ← 受注時は引き当てていない");
        List<ShipmentResult> results = services.shipment.createInstruction(
                List.of(orderNo), LocalDate.of(2026, 1, 16), WAREHOUSE);
        System.out.println("    出荷指示の後の引当済数量（4901234001）: "
                + repos.allocatedQty("4901234001") + " ← ここで初めて引き当てた");
        System.out.println("    受注ステータス: "
                + repos.order(orderNo).getStatus().getCode()
                + "（" + repos.order(orderNo).getStatus().getLabel() + "）");
        System.out.println("    出荷指示番号: " + results.get(0).getShipmentNo());
        for (Allocation allocation : repos.allocationsOf(orderNo)) {
            System.out.printf("    引当: 商品 %s ロット %s 数量 %s（入庫日の古い順）%n",
                    allocation.getProductCd(), allocation.getLotNo(),
                    allocation.getAllocatedQty());
        }
    }

    /** C2: 取消可否は日付ではなくステータスで判定する。 */
    private static void scenario5(Services services, InMemoryRepositories repos,
                                  String orderNo) {
        title("5. 出荷指示済の受注を取り消す（C2 取消の期限）");
        CancelResult result = services.cancel.cancelOrder(orderNo, SALES_STAFF, "01",
                null, repos.order(orderNo).getUpdDatetime());
        System.out.println("    要件定義は「受注日の翌営業日まで」だが、実装はステータスで判定する");
        System.out.println("    取り消せたか: " + result.isOk()
                + " / " + result.getMessage());
    }

    /** 有効在庫が足りない受注は一部引当となり、不足数が返る。 */
    private static void scenario6(Services services, InMemoryRepositories repos) {
        title("6. 在庫が足りない受注の出荷指示（先入先出と一部引当）");
        OrderDto dto = order("00010001", detail("4901234003", "3", "2"));
        OrderResult registered = services.regist.registerOrder(dto);
        List<ShipmentResult> results = services.shipment.createInstruction(
                List.of(registered.getOrderNo()), LocalDate.of(2026, 1, 16), WAREHOUSE);
        System.out.println("    在庫 2（L005 が 1・L006 が 1）に対し受注数量 3");
        System.out.println("    入庫日の古い L005 から引き当て、2 まで確保して 1 が不足する");
        for (Allocation allocation : repos.allocationsOf(registered.getOrderNo())) {
            System.out.printf("    引当: ロット %s 数量 %s%n",
                    allocation.getLotNo(), allocation.getAllocatedQty());
        }
        for (Shortage shortage : results.get(0).getShortages()) {
            System.out.println("    不足: 商品 " + shortage.getProductCd()
                    + " 数量 " + shortage.getShortQty());
        }
        System.out.println("    出荷指示は作成しない（指示番号 "
                + results.get(0).getShipmentNo() + "）");
    }

    /** 与信保留（20）の受注は出荷指示の対象にならない。 */
    private static void scenario7(Services services) {
        title("7. 与信保留の受注を出荷指示する（承認待ちのため対象外）");
        OrderDto dto = order("00010002", detail("4901234001", "20", "1"));
        OrderResult held = services.regist.registerOrder(dto);
        System.out.println("    受注 " + held.getOrderNo() + " のステータス: "
                + held.getStatus().getCode() + "（" + held.getStatus().getLabel() + "）");
        List<ShipmentResult> results = services.shipment.createInstruction(
                List.of(held.getOrderNo()), LocalDate.of(2026, 1, 16), WAREHOUSE);
        System.out.println("    出荷指示できたか: " + results.get(0).isOk()
                + " / " + results.get(0).getMessage());
    }

    /** 受注内容の変更。出荷指示前なら数量と納品希望日を変えられる。 */
    private static void scenario8(Services services) {
        title("8. 受注内容の変更（数量と納品希望日）");
        OrderDto dto = order("00010001", detail("4901234002", "4", "1"));
        OrderResult registered = services.regist.registerOrder(dto);
        String orderNo = registered.getOrderNo();

        List<DetailChange> changes = new ArrayList<>();
        changes.add(new DetailChange(1, new BigDecimal("6")));
        UpdateResult changed = services.update.changeOrder(orderNo, SALES_STAFF,
                LocalDate.of(2026, 1, 20), changes,
                services.repos.order(orderNo).getUpdDatetime());
        System.out.println("    変更できたか: " + changed.isOk() + " / " + changed.getMessage());
        System.out.println("    受注金額: " + services.repos.order(orderNo).getTotalAmount()
                + "（980.25 × 6 = 5881.5 → 円未満切り捨てで 5881）");

        List<ShipmentResult> shipped = services.shipment.createInstruction(
                List.of(orderNo), LocalDate.of(2026, 1, 20), WAREHOUSE);
        UpdateResult afterShip = services.update.changeOrder(orderNo, SALES_STAFF,
                LocalDate.of(2026, 1, 21), changes,
                services.repos.order(orderNo).getUpdDatetime());
        System.out.println("    出荷指示（" + shipped.get(0).getShipmentNo()
                + "）の後に変更: " + afterShip.isOk() + " / " + afterShip.getMessage());
    }

    /** EDI受注の取込。JAN コードから商品を引き当て、弾かれた 1 件は理由が残る。 */
    private static void scenario9(Services services) {
        title("9. EDI受注の取込");
        ImportSummary summary = services.ediImport.importOrders(ORDER_DATE);
        System.out.println("    成功 " + summary.getSuccessCount()
                + " 件 / エラー " + summary.getErrorCount() + " 件");
        for (EdiRecord record : services.ediImport.findResults(ORDER_DATE)) {
            System.out.printf("    %s  得意先 %s  JAN %s  → %s%s%n",
                    record.getRecvNo(), record.getEdiCustomerCd(), record.getJanCd(),
                    record.getStatus(),
                    record.getOrderNo() == null
                            ? "（" + record.getErrorMessage() + "）"
                            : "（受注 " + record.getOrderNo() + "）");
        }
    }

    /** 入庫登録と在庫照会。新しいロットは行を起こす。 */
    private static void scenario10(Services services, InMemoryRepositories repos) {
        title("10. 入庫登録と在庫照会");
        services.stockUpdate.receive(new StockMoveDto(WAREHOUSE, "4901234003", "L007",
                MoveType.PURCHASE_IN, new BigDecimal("30"), LocalDate.of(2026, 1, 16),
                "仕入入庫", WAREHOUSE_STAFF));
        for (StockView view : services.stockInquiry.search(WAREHOUSE, "4901234003")) {
            System.out.printf("    %s  実在庫 %s / 引当済 %s / 有効在庫 %s%n",
                    view.getProductName(), view.getStockQty(), view.getAllocatedQty(),
                    view.getAvailableQty());
        }
        System.out.println("    在庫移動履歴: " + repos.stockHistories().size() + " 件");
    }

    /** 出荷実績の受信で在庫を引き落とし、売上を計上する。 */
    private static void scenario11(Services services, InMemoryRepositories repos) {
        title("11. 出荷実績の受信（出庫処理と売上計上）");
        OrderDto dto = order("00010001", detail("4901234055", "4", "1"));
        OrderResult registered = services.regist.registerOrder(dto);
        services.shipment.createInstruction(List.of(registered.getOrderNo()),
                LocalDate.of(2026, 1, 16), WAREHOUSE);

        List<ShipmentResultDto> results = new ArrayList<>();
        results.add(new ShipmentResultDto("S001", registered.getOrderNo(), 1, WAREHOUSE,
                "4901234055", "L003", new BigDecimal("4"), LocalDate.of(2026, 1, 16)));
        int imported = services.shipmentResult.importResults(results);
        System.out.println("    取り込んだ出荷実績: " + imported + " 件");
        for (StockView view : services.stockInquiry.search(WAREHOUSE, "4901234055")) {
            System.out.printf("    引落後の在庫: 実在庫 %s / 引当済 %s / 有効在庫 %s%n",
                    view.getStockQty(), view.getAllocatedQty(), view.getAvailableQty());
        }
    }

    /** 棚卸の確定。差異のある行だけが在庫調整になる。 */
    private static void scenario12(Services services) {
        title("12. 棚卸の確定（差異を在庫調整として計上）");
        services.repos.seedCountLine("C20260116001", WAREHOUSE, "4901234002", "L004",
                new BigDecimal("40"), new BigDecimal("38"));
        services.repos.seedCountLine("C20260116001", WAREHOUSE, "4901234001", "L001",
                new BigDecimal("100"), new BigDecimal("100"));
        CountResult result = services.count.confirmCount("C20260116001",
                LocalDate.of(2026, 1, 16), WAREHOUSE_STAFF);
        System.out.println("    棚卸番号 " + result.getCountNo()
                + " / 調整した行 " + result.getAdjustedLines()
                + " 行 / 差異合計 " + result.getTotalDiffQty());
        System.out.println("    差異が 0 の行は在庫を動かさない");
    }

    /** 業務ルール「在庫マイナスの禁止」。実在庫を超える出庫はできない。 */
    private static void scenario13(Services services) {
        title("13. 在庫マイナスの禁止");
        try {
            services.stockUpdate.issue(new StockMoveDto(WAREHOUSE, "4901234002", "L004",
                    MoveType.SHIPMENT_OUT, new BigDecimal("9999"),
                    LocalDate.of(2026, 1, 16), "過大な出庫", WAREHOUSE_STAFF));
            System.out.println("    出庫できてしまった（業務ルール違反）");
        } catch (BusinessException e) {
            System.out.println("    弾かれた: " + e.getMessage());
        }
    }

    /** C3: 明細単位の切り捨てと請求単位の四捨五入で消費税額が変わる。 */
    private static String scenario14(Services services, InMemoryRepositories repos) {
        title("14. 請求締め（C3 消費税の計算単位）");
        LocalDate to = LocalDate.of(2026, 1, 20);
        repos.seedSales("00010001", LocalDate.of(2026, 1, 6), "4901234001",
                new BigDecimal("10005"), "1");
        repos.seedSales("00010001", LocalDate.of(2026, 1, 13), "4901234055",
                new BigDecimal("10005"), "1");
        repos.seedSales("00010001", LocalDate.of(2026, 1, 20), "4901234002",
                new BigDecimal("2940"), "1");

        // 同じ売上に 2 つの方式を当てて、税額が食い違うことを見せる。締めが走ると
        // 売上に請求番号が入って集計対象から外れるので、先に集計しておく。
        SalesSummary summary = services.billing.aggregateSales("00010001",
                LocalDate.of(2025, 12, 21), to);
        BigDecimal perLine = BigDecimal.ZERO;
        for (SalesLine line : summary.getLines()) {
            perLine = perLine.add(services.taxCalculator.calcTax(
                    line.getNetAmount(), line.getTaxType(), RoundingMode.FLOOR));
        }

        System.out.println("    締め日の判定（1/20 は 20日締めの締め日か）: "
                + services.billing.isClosingDay(ClosingType.TWENTIETH, to));
        System.out.println("    締め日は締め日マスタから引く（20日締め = "
                + services.billing.closingDay(ClosingType.TWENTIETH) + " 日）");
        BatchResult batch = services.billing.execute("202601", List.of("00010001"), to);
        System.out.println("    確定 " + batch.getClosedCount()
                + " 件 / 対象外 " + batch.getSkippedCount()
                + " 件 / 異常 " + batch.getFailedCount() + " 件");
        System.out.println("    明細単位・切り捨て（受注登録の方式）: 消費税 " + perLine);
        System.out.println("    請求単位・四捨五入（請求締めの方式）: 消費税 "
                + summary.getTaxAmount());
        System.out.println("    → 同じ売上（税抜 " + summary.getSalesAmount()
                + "）でも税額が " + perLine + " と " + summary.getTaxAmount()
                + " で食い違う");

        for (Invoice invoice : services.repos.invoiceRepository().findByClosingYm("202601")) {
            return invoice.getInvoiceNo();
        }
        return null;
    }

    /** 請求書の発行。帳票基盤へ渡す出力データを組み立てる。 */
    private static void scenario15(Services services, String invoiceNo) {
        title("15. 請求書発行（帳票基盤へ渡す出力データ）");
        if (invoiceNo == null) {
            System.out.println("    発行対象の請求がない");
            return;
        }
        InvoicePrintData data = services.invoicePrint.print(invoiceNo,
                LocalDate.of(2026, 1, 21), ACCOUNTING_STAFF);
        System.out.println("    請求番号 " + data.getInvoiceNo()
                + " / " + data.getCustomerName()
                + " / 請求金額 " + data.getBillingAmount());
        System.out.println("    明細 " + data.getLines().size()
                + " 行（PDF の組版は帳票基盤（SVF）が行う）");
        try {
            services.invoicePrint.print(invoiceNo, LocalDate.of(2026, 1, 21),
                    ACCOUNTING_STAFF);
        } catch (BusinessException e) {
            System.out.println("    二重発行は弾かれる: " + e.getMessage());
        }
    }

    /** 入金消込。請求番号が一致しないものは候補として担当者へ回す。 */
    private static void scenario16(Services services, String invoiceNo) {
        title("16. 入金消込（突合順序と候補提示）");
        if (invoiceNo == null) {
            System.out.println("    消込対象の請求がない");
            return;
        }
        LocalDate depositDate = LocalDate.of(2026, 2, 25);
        services.repos.seedDeposit("D001", depositDate, "00010001",
                new BigDecimal("5000"), "チユウオウフードサービス", null);
        MatchSummary summary = services.deposit.matchDeposits(depositDate);
        System.out.println("    自動消込 " + summary.getMatchedCount()
                + " 件 / 候補 " + summary.getUnmatchedCount() + " 件");
        for (DepositCandidate candidate : summary.getCandidates()) {
            System.out.printf("    候補: 入金 %s → 請求 %s（%s / 未回収 %s）%n",
                    candidate.getDepositNo(), candidate.getInvoiceNo(),
                    candidate.getReason(), candidate.getUnpaidAmount());
        }
        services.deposit.applyDeposit("D001", invoiceNo, new BigDecimal("5000"),
                ACCOUNTING_STAFF);
        Invoice invoice = services.repos.invoiceRepository().find(invoiceNo);
        System.out.println("    充当後: 入金額 " + invoice.getDepositAmount()
                + " / 未回収 " + invoice.getUnpaidAmount()
                + " / ステータス " + invoice.getStatus().getLabel());
    }

    /** 売上仕訳の連携。会計システムへ渡す固定長 CSV の元になる仕訳を作る。 */
    private static void scenario17(Services services) {
        title("17. 売上仕訳連携（会計システム連携）");
        int lines = services.journal.execute(LocalDate.of(2026, 1, 20));
        System.out.println("    出力した仕訳: " + lines + " 行（売上高と仮受消費税に分ける）");
        for (int i = 0; i < services.repos.journals().size(); i++) {
            System.out.printf("    借方 %s / 貸方 %s / 金額 %s / %s%n",
                    services.repos.journals().get(i).getDebitAccount(),
                    services.repos.journals().get(i).getCreditAccount(),
                    services.repos.journals().get(i).getAmount(),
                    services.repos.journals().get(i).getSummary());
        }
    }

    /** 売掛残高の照会。支払期日を過ぎた請求は滞留として出る。 */
    private static void scenario18(Services services) {
        title("18. 売掛残高照会（滞留の判定）");
        LocalDate baseDate = LocalDate.of(2026, 4, 10);
        for (Receivable receivable : services.receivable.search(null, baseDate)) {
            System.out.printf("    %s %s  請求 %s / 入金 %s / 未回収 %s / 滞留 %d 日%s%n",
                    receivable.getCustomerCd(), receivable.getCustomerName(),
                    receivable.getBillingAmount(), receivable.getDepositAmount(),
                    receivable.getUnpaidAmount(), receivable.getOverdueDays(),
                    receivable.isOverdue() ? " ← 滞留" : "");
        }
    }

    /** 監査ログ。ハッシュの連鎖で書き換えを検知できる。 */
    private static void scenario19(Services services, InMemoryRepositories repos) {
        title("19. 監査ログ（保存期間と改ざん検知）");
        System.out.println("    記録した監査ログ: "
                + repos.auditLogRepository().findAll().size() + " 件");
        System.out.println("    保存年数: " + services.auditLogger.getRetentionYears() + " 年");
        System.out.println("    連鎖の検証（書き換え前）: " + services.auditLogger.verifyChain());
        repos.tamperAuditLog(0, "書き換えた値");
        System.out.println("    連鎖の検証（1 件書き換えた後）: "
                + services.auditLogger.verifyChain());
        System.out.println("    5 年より前の監査ログを削除: "
                + services.auditLogger.purgeExpired(LocalDate.now()) + " 件");
    }

    /** 受注状況の照会。検索条件に合う受注をページ単位で返す。 */
    private static void scenario20(Services services) {
        title("20. 受注一覧照会（検索条件とページ送り）");
        OrderSearchCondition condition = new OrderSearchCondition();
        condition.setCustomerCd("00010001");
        condition.setPageSize(3);
        OrderPage page = services.search.search(condition);
        System.out.println("    得意先 00010001 の受注: " + page.getTotalCount()
                + " 件 / " + page.getTotalPages() + " ページ（1 ページ "
                + page.getPageSize() + " 件）");
        for (OrderSummary row : page.getRows()) {
            System.out.printf("    %s  %s  %s  %s  経路 %s%n",
                    row.getOrderNo(), row.getOrderDate(), row.getCustomerName(),
                    row.getStatus().getLabel(), row.getRoute().getLabel());
        }
        System.out.println("    最後のページか: " + page.isLast());
    }

    private static OrderDto order(String customerCd, OrderDetailDto... details) {
        List<OrderDetailDto> list = new ArrayList<>();
        for (OrderDetailDto detail : details) {
            list.add(detail);
        }
        OrderDto dto = new OrderDto(customerCd, ORDER_DATE, DELIVERY_DATE, list);
        dto.setEntryStaffCd(SALES_STAFF);
        return dto;
    }

    private static OrderDetailDto detail(String productCd, String qty, String taxType) {
        return new OrderDetailDto(productCd, new BigDecimal(qty), taxType);
    }

    private static void title(String text) {
        System.out.println();
        System.out.println("=== " + text + " ===");
    }
}
'''

DEMO: dict[str, str] = {
    "demo/InMemoryRepositories.java": _IN_MEMORY,
    "demo/Services.java": _SERVICES,
    "demo/Main.java": _MAIN,
}
