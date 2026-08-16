"""在庫管理（`inventory`）のうち、引当以外のモジュールの Java ソース。

設計書のモジュール一覧にある

- 在庫更新サービス（`StockUpdateService`）—— 入庫・出庫・在庫調整
- 棚卸サービス（`InventoryCountService`）
- 出荷実績取込サービス（`ShipmentResultImportService`）

と、機能要件「在庫照会」に対応する `StockInquiryService` を置く。
在庫引当サービスは C1（引当のタイミング）を仕込んである関係で
`code_sources.py` に残してある。

`StockInquiryService` は `spec.MODULES` にモジュール定義が無い。機能要件
「在庫照会」に対応する実装が要るのに設計書がモジュールを起こしていないため、
G6 と同じ**コードにしか無い**ものとして javadoc に書いてある。
"""

from __future__ import annotations

import code_kit as ck

_BD = "java.math.BigDecimal"
_LD = "java.time.LocalDate"
_ROOT = ck.PACKAGE_ROOT


# ── 在庫移動区分（コード定義書の在庫移動区分と一致させる）────────────────────
_MOVE_TYPE = ck.code_enum(
    "inventory/MoveType.java",
    "在庫移動区分（T_STOCK_HISTORY.MOVE_TYPE）。\n *\n"
    " * コード値はコード定義書の在庫移動区分と一致させる。増減の向きは区分から\n"
    " * 決まるので、呼出元は数量に符号を付けない。",
    [
        ("PURCHASE_IN", "11", "仕入入庫"),
        ("RETURN_IN", "12", "返品入庫"),
        ("SHIPMENT_OUT", "21", "出荷出庫"),
        ("COUNT_ADJUST", "31", "棚卸調整"),
        ("DISPOSAL", "32", "破損廃棄"),
    ],
    extra="""    /** 実在庫数を増やす区分か。棚卸調整は差異の符号で決まるので偽とする。 */
    public boolean isIncoming() {
        return this == PURCHASE_IN || this == RETURN_IN;
    }""")


# ── 値クラス ──────────────────────────────────────────────────────────────
_STOCK_MOVE_DTO = ck.bean(
    "inventory/StockMoveDto.java",
    "在庫を動かす 1 件の指示。入庫・出庫・在庫調整に共通で使う。\n *\n"
    " * 数量は常に正の値で渡し、増減の向きは在庫移動区分から決める。",
    "String warehouseCd; String productCd; String lotNo; MoveType moveType;"
    " BigDecimal qty; LocalDate moveDate; String reason; String staffCd",
    [_BD, _LD])

_STOCK_HISTORY = ck.bean(
    "inventory/StockHistory.java",
    "在庫移動履歴（T_STOCK_HISTORY）。入出庫・調整の増減を時系列で残す。",
    "String warehouseCd; String productCd; String lotNo; MoveType moveType;"
    " BigDecimal qty; BigDecimal beforeQty; BigDecimal afterQty;"
    " LocalDate moveDate; String reason; String staffCd", [_BD, _LD])

_STOCK_VIEW = ck.bean(
    "inventory/StockView.java",
    "在庫照会 1 行。実在庫数・引当済数量・有効在庫数を商品ごとにまとめたもの。",
    "String warehouseCd; String productCd; String productName;"
    " BigDecimal stockQty; BigDecimal allocatedQty", [_BD],
    extra="""    /** 有効在庫数（実在庫数 − 引当済数量）。 */
    public BigDecimal getAvailableQty() {
        return stockQty.subtract(allocatedQty);
    }""")

_COUNT_LINE = ck.bean(
    "inventory/CountLine.java",
    "棚卸入力 1 行。帳簿在庫（実在庫数）と実地棚卸の結果を並べて持つ。",
    "String countNo; String warehouseCd; String productCd; String lotNo;"
    " BigDecimal bookQty; *BigDecimal actualQty", [_BD],
    extra="""    /** 差異数量（実地 − 帳簿）。正なら増、負なら減の在庫調整になる。 */
    public BigDecimal diffQty() {
        return actualQty.subtract(bookQty);
    }""")

_COUNT_RESULT = ck.bean(
    "inventory/CountResult.java",
    "棚卸の確定結果。差異のあった行数と、増減の合計を返す。",
    "String countNo; int adjustedLines; BigDecimal totalDiffQty", [_BD])

_SHIPMENT_RESULT_DTO = ck.bean(
    "inventory/ShipmentResultDto.java",
    "倉庫管理システム（WMS）から受信する出荷実績 1 行。\n *\n"
    " * 外部インターフェース「出荷実績受信」の受信電文に対応する。",
    "String shipmentNo; String orderNo; int lineNo; String warehouseCd;"
    " String productCd; String lotNo; BigDecimal shippedQty; LocalDate shippedDate",
    [_BD, _LD])


# ── リポジトリ ────────────────────────────────────────────────────────────
_STOCK_HISTORY_REPOSITORY = ck.iface(
    "inventory/repository/StockHistoryRepository.java",
    "在庫移動履歴（T_STOCK_HISTORY）への追記と参照。",
    [
        "void insert(StockHistory history)",
        "List<StockHistory> findByProduct(String warehouseCd, String productCd)",
    ],
    ["java.util.List", f"{_ROOT}.inventory.StockHistory"])

_COUNT_REPOSITORY = ck.iface(
    "inventory/repository/InventoryCountRepository.java",
    "棚卸（T_INVENTORY_COUNT）への登録と参照。",
    [
        "void insert(List<CountLine> lines)",
        "List<CountLine> findByCountNo(String countNo)",
        "List<String> findOpenCountNos()",
        "void markConfirmed(String countNo)",
    ],
    ["java.util.List", f"{_ROOT}.inventory.CountLine"])


# ── 在庫更新サービス ──────────────────────────────────────────────────────
_STOCK_UPDATE = '''package jp.co.contoso.sps.inventory.service;

import java.math.BigDecimal;

import jp.co.contoso.sps.common.AuditLogger;
import jp.co.contoso.sps.common.BusinessException;
import jp.co.contoso.sps.framework.Service;
import jp.co.contoso.sps.framework.Transactional;
import jp.co.contoso.sps.inventory.MoveType;
import jp.co.contoso.sps.inventory.Stock;
import jp.co.contoso.sps.inventory.StockHistory;
import jp.co.contoso.sps.inventory.StockMoveDto;
import jp.co.contoso.sps.inventory.repository.StockHistoryRepository;
import jp.co.contoso.sps.inventory.repository.StockRepository;

/**
 * 在庫更新サービス。
 *
 * 入庫・出庫・調整による在庫数の増減を反映し、移動履歴を記録する。
 *
 * 関連する機能: 入庫登録 / 出庫処理 / 在庫調整
 * 関連する画面: 入庫登録 / 在庫調整
 * 関連する業務ルール: 在庫マイナスの禁止
 *
 * 実在庫数が負になる更新は業務ルール「在庫マイナスの禁止」で禁じられている。
 * 出庫・調整のいずれも、更新後の実在庫数を先に求めて負なら例外を送出する。
 */
@Service
public class StockUpdateService {

    private final StockRepository stockRepository;
    private final StockHistoryRepository stockHistoryRepository;
    private final AuditLogger auditLogger;

    public StockUpdateService(StockRepository stockRepository,
                              StockHistoryRepository stockHistoryRepository,
                              AuditLogger auditLogger) {
        this.stockRepository = stockRepository;
        this.stockHistoryRepository = stockHistoryRepository;
        this.auditLogger = auditLogger;
    }

    /**
     * 入庫として実在庫数を加算し、在庫移動履歴を登録する。
     *
     * 対象のロットが在庫に無い場合は行を新しく起こす（初回入庫）。
     */
    @Transactional
    public void receive(StockMoveDto dto) {
        requirePositive(dto.getQty());
        Stock stock = stockRepository.find(dto.getWarehouseCd(), dto.getProductCd(),
                dto.getLotNo());
        BigDecimal before = stock == null ? BigDecimal.ZERO : stock.getStockQty();
        if (stock == null) {
            stockRepository.insert(new Stock(dto.getWarehouseCd(), dto.getProductCd(),
                    dto.getLotNo(), dto.getMoveDate(), dto.getQty(), BigDecimal.ZERO));
        } else {
            stockRepository.addStockQty(dto.getWarehouseCd(), dto.getProductCd(),
                    dto.getLotNo(), dto.getQty());
        }
        writeHistory(dto, before, before.add(dto.getQty()));
    }

    /**
     * 出庫として実在庫数と引当済数量を減算し、在庫移動履歴を登録する。
     *
     * 出荷実績の受信をもって呼ばれる。引当済数量は出庫した数量ぶんだけ戻す
     * （在庫から出た以上、確保しておく必要が無くなるため）。
     *
     * 関連する業務ルール: 在庫マイナスの禁止
     */
    @Transactional
    public void issue(StockMoveDto dto) {
        requirePositive(dto.getQty());
        Stock stock = require(dto);
        BigDecimal after = stock.getStockQty().subtract(dto.getQty());
        if (after.compareTo(BigDecimal.ZERO) < 0) {
            throw new BusinessException("実在庫数が不足しています。（商品:"
                    + dto.getProductCd() + " 在庫:" + stock.getStockQty()
                    + " 出庫:" + dto.getQty() + "）");
        }
        stockRepository.subtractStockQty(dto.getWarehouseCd(), dto.getProductCd(),
                dto.getLotNo(), dto.getQty());
        BigDecimal releasable = dto.getQty().min(stock.getAllocatedQty());
        if (releasable.compareTo(BigDecimal.ZERO) > 0) {
            stockRepository.subtractAllocatedQty(dto.getWarehouseCd(),
                    dto.getProductCd(), dto.getLotNo(), releasable);
        }
        writeHistory(dto, stock.getStockQty(), after);
    }

    /**
     * 在庫調整として実在庫数を補正し、在庫移動履歴を登録する。
     *
     * 破損・廃棄は減、棚卸調整は差異の符号ぶん増減する。調整後の実在庫数が
     * 負になる場合は業務エラーとする。
     *
     * 関連する業務ルール: 在庫マイナスの禁止
     */
    @Transactional
    public void adjust(StockMoveDto dto) {
        Stock stock = require(dto);
        BigDecimal delta = dto.getMoveType() == MoveType.DISPOSAL
                ? dto.getQty().negate()
                : dto.getQty();
        BigDecimal after = stock.getStockQty().add(delta);
        if (after.compareTo(BigDecimal.ZERO) < 0) {
            throw new BusinessException("在庫調整の結果が負になります。（商品:"
                    + dto.getProductCd() + " 在庫:" + stock.getStockQty()
                    + " 調整:" + delta + "）");
        }
        if (delta.compareTo(BigDecimal.ZERO) >= 0) {
            stockRepository.addStockQty(dto.getWarehouseCd(), dto.getProductCd(),
                    dto.getLotNo(), delta);
        } else {
            stockRepository.subtractStockQty(dto.getWarehouseCd(), dto.getProductCd(),
                    dto.getLotNo(), delta.negate());
        }
        writeHistory(dto, stock.getStockQty(), after);
    }

    private Stock require(StockMoveDto dto) {
        Stock stock = stockRepository.find(dto.getWarehouseCd(), dto.getProductCd(),
                dto.getLotNo());
        if (stock == null) {
            throw new BusinessException("在庫が登録されていません。（倉庫:"
                    + dto.getWarehouseCd() + " 商品:" + dto.getProductCd()
                    + " ロット:" + dto.getLotNo() + "）");
        }
        return stock;
    }

    private static void requirePositive(BigDecimal qty) {
        if (qty == null || qty.compareTo(BigDecimal.ZERO) <= 0) {
            throw new BusinessException("数量は0より大きい値を入力してください。");
        }
    }

    private void writeHistory(StockMoveDto dto, BigDecimal before, BigDecimal after) {
        stockHistoryRepository.insert(new StockHistory(dto.getWarehouseCd(),
                dto.getProductCd(), dto.getLotNo(), dto.getMoveType(), dto.getQty(),
                before, after, dto.getMoveDate(), dto.getReason(), dto.getStaffCd()));
        auditLogger.record(dto.getStaffCd(), "在庫更新（" + dto.getMoveType().getLabel() + "）",
                dto.getWarehouseCd() + "/" + dto.getProductCd() + "/" + dto.getLotNo(),
                before, after);
    }
}
'''


# ── 棚卸サービス ──────────────────────────────────────────────────────────
_INVENTORY_COUNT = '''package jp.co.contoso.sps.inventory.service;

import java.math.BigDecimal;
import java.time.LocalDate;
import java.util.List;

import jp.co.contoso.sps.common.AuditLogger;
import jp.co.contoso.sps.framework.Service;
import jp.co.contoso.sps.framework.Transactional;
import jp.co.contoso.sps.inventory.CountLine;
import jp.co.contoso.sps.inventory.CountResult;
import jp.co.contoso.sps.inventory.MoveType;
import jp.co.contoso.sps.inventory.StockMoveDto;
import jp.co.contoso.sps.inventory.repository.InventoryCountRepository;

/**
 * 棚卸サービス。
 *
 * 実地棚卸の入力から帳簿在庫との差異を算出し、在庫調整として確定する。
 *
 * 関連する機能: 棚卸
 * 関連する画面: 棚卸入力
 * 関連する帳票: 在庫棚卸表
 * 関連する業務ルール: 在庫マイナスの禁止
 *
 * 差異のある行だけを在庫調整（棚卸調整）として在庫更新サービスへ渡す。
 * 確定した棚卸は再確定できない。
 */
@Service
public class InventoryCountService {

    private final InventoryCountRepository countRepository;
    private final StockUpdateService stockUpdateService;
    private final AuditLogger auditLogger;

    public InventoryCountService(InventoryCountRepository countRepository,
                                 StockUpdateService stockUpdateService,
                                 AuditLogger auditLogger) {
        this.countRepository = countRepository;
        this.stockUpdateService = stockUpdateService;
        this.auditLogger = auditLogger;
    }

    /** 棚卸の入力を登録する。確定はしない。 */
    @Transactional
    public void saveCount(List<CountLine> lines) {
        countRepository.insert(lines);
    }

    /** 確定していない棚卸番号を返す。 */
    public List<String> listOpenCounts() {
        return countRepository.findOpenCountNos();
    }

    /** 棚卸の入力行を返す。 */
    public List<CountLine> findLines(String countNo) {
        return countRepository.findByCountNo(countNo);
    }

    /**
     * 棚卸入力と帳簿在庫の差異を在庫調整として確定する。
     *
     * 差異が 0 の行は在庫を動かさない。1 行でも異常が起きた場合は棚卸番号
     * ぶんをまとめてロールバックする。
     */
    @Transactional
    public CountResult confirmCount(String countNo, LocalDate countDate, String staffCd) {
        List<CountLine> lines = countRepository.findByCountNo(countNo);
        int adjusted = 0;
        BigDecimal totalDiff = BigDecimal.ZERO;
        for (CountLine line : lines) {
            BigDecimal diff = line.diffQty();
            if (diff.compareTo(BigDecimal.ZERO) == 0) {
                continue;
            }
            stockUpdateService.adjust(new StockMoveDto(line.getWarehouseCd(),
                    line.getProductCd(), line.getLotNo(), MoveType.COUNT_ADJUST,
                    diff, countDate, "棚卸差異（棚卸番号 " + countNo + "）", staffCd));
            adjusted++;
            totalDiff = totalDiff.add(diff);
        }
        countRepository.markConfirmed(countNo);
        auditLogger.record(staffCd, "棚卸確定", countNo, lines.size() + "行", adjusted + "行調整");
        return new CountResult(countNo, adjusted, totalDiff);
    }
}
'''


# ── 出荷実績取込サービス ──────────────────────────────────────────────────
_SHIPMENT_RESULT_IMPORT = '''package jp.co.contoso.sps.inventory.service;

import java.math.BigDecimal;
import java.math.RoundingMode;
import java.util.List;

import jp.co.contoso.sps.billing.SalesLine;
import jp.co.contoso.sps.billing.repository.SalesRepository;
import jp.co.contoso.sps.common.AuditLogger;
import jp.co.contoso.sps.common.BusinessException;
import jp.co.contoso.sps.framework.Service;
import jp.co.contoso.sps.framework.Transactional;
import jp.co.contoso.sps.inventory.MoveType;
import jp.co.contoso.sps.inventory.ShipmentResultDto;
import jp.co.contoso.sps.inventory.StockMoveDto;
import jp.co.contoso.sps.order.Order;
import jp.co.contoso.sps.order.OrderDetail;
import jp.co.contoso.sps.order.repository.OrderRepository;

/**
 * 出荷実績取込サービス。
 *
 * WMS から受信した出荷実績で在庫を引き落とし、売上を計上する。
 *
 * 関連する機能: 出庫処理
 * 関連する外部インターフェース: 出荷実績受信
 * 関連する業務ルール: 在庫マイナスの禁止
 *
 * 売上金額は受注明細の販売単価に出荷数量を掛けて求め、円未満を切り捨てる
 * （受注登録の明細金額と同じ丸め方にそろえる）。計上した売上は請求締めの
 * 集計対象になる。
 */
@Service
public class ShipmentResultImportService {

    private final StockUpdateService stockUpdateService;
    private final OrderRepository orderRepository;
    private final SalesRepository salesRepository;
    private final AuditLogger auditLogger;

    public ShipmentResultImportService(StockUpdateService stockUpdateService,
                                       OrderRepository orderRepository,
                                       SalesRepository salesRepository,
                                       AuditLogger auditLogger) {
        this.stockUpdateService = stockUpdateService;
        this.orderRepository = orderRepository;
        this.salesRepository = salesRepository;
        this.auditLogger = auditLogger;
    }

    /**
     * 出荷実績を 1 件ずつ取り込み、取り込めた件数を返す。
     *
     * 1 件の異常で全体を止めないよう、実績 1 件ごとにトランザクションを切る。
     */
    public int importResults(List<ShipmentResultDto> results) {
        int imported = 0;
        for (ShipmentResultDto result : results) {
            importOne(result);
            imported++;
        }
        return imported;
    }

    /** 出荷実績 1 件を取り込む。在庫の引落と売上計上を 1 トランザクションで行う。 */
    @Transactional
    public void importOne(ShipmentResultDto result) {
        Order order = orderRepository.find(result.getOrderNo());
        if (order == null) {
            throw new BusinessException("受注が存在しません。（受注番号:"
                    + result.getOrderNo() + "）");
        }
        OrderDetail detail = orderRepository.findDetail(result.getOrderNo(),
                result.getLineNo());
        if (detail == null) {
            throw new BusinessException("受注明細が存在しません。（受注番号:"
                    + result.getOrderNo() + " 明細:" + result.getLineNo() + "）");
        }

        stockUpdateService.issue(new StockMoveDto(result.getWarehouseCd(),
                result.getProductCd(), result.getLotNo(), MoveType.SHIPMENT_OUT,
                result.getShippedQty(), result.getShippedDate(),
                "出荷実績（出荷指示 " + result.getShipmentNo() + "）", AuditLogger.SYSTEM));

        BigDecimal netAmount = detail.getUnitPrice()
                .multiply(result.getShippedQty())
                .setScale(0, RoundingMode.FLOOR);
        salesRepository.insert(new SalesLine(order.getCustomerCd(),
                result.getShippedDate(), result.getProductCd(), netAmount,
                detail.getTaxType(), null));
        auditLogger.record("出荷実績取込", result.getShipmentNo(), null, netAmount);
    }
}
'''


# ── 在庫照会サービス ──────────────────────────────────────────────────────
_STOCK_INQUIRY = '''package jp.co.contoso.sps.inventory.service;

import java.math.BigDecimal;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

import jp.co.contoso.sps.common.Product;
import jp.co.contoso.sps.common.repository.ProductRepository;
import jp.co.contoso.sps.framework.Service;
import jp.co.contoso.sps.inventory.Stock;
import jp.co.contoso.sps.inventory.StockView;
import jp.co.contoso.sps.inventory.repository.StockRepository;

/**
 * 在庫照会サービス。
 *
 * 倉庫・商品ごとの実在庫数・引当済数量・有効在庫数を照会する。ロットに分かれて
 * いる在庫を商品単位に足し上げて返す。
 *
 * 関連する機能: 在庫照会
 * 関連する画面: 在庫照会
 *
 * 詳細設計にモジュールの定義が無い。機能要件「在庫照会」に対応する実装が
 * 要るのに設計書がモジュールを起こしていないため、コード側だけに存在する。
 */
@Service
public class StockInquiryService {

    private final StockRepository stockRepository;
    private final ProductRepository productRepository;

    public StockInquiryService(StockRepository stockRepository,
                               ProductRepository productRepository) {
        this.stockRepository = stockRepository;
        this.productRepository = productRepository;
    }

    /**
     * 倉庫の在庫を商品単位にまとめて返す。
     *
     * 商品コードを指定した場合はその商品だけに絞る。商品コードの昇順で返す。
     */
    public List<StockView> search(String warehouseCd, String productCd) {
        Map<String, StockView> merged = new LinkedHashMap<>();
        for (Stock stock : stockRepository.findByWarehouse(warehouseCd)) {
            if (productCd != null && !productCd.isBlank()
                    && !stock.getProductCd().equals(productCd)) {
                continue;
            }
            StockView view = merged.get(stock.getProductCd());
            if (view == null) {
                Product product = productRepository.find(stock.getProductCd());
                merged.put(stock.getProductCd(), new StockView(warehouseCd,
                        stock.getProductCd(),
                        product == null ? "" : product.getProductName(),
                        stock.getStockQty(), stock.getAllocatedQty()));
            } else {
                merged.put(stock.getProductCd(), new StockView(warehouseCd,
                        stock.getProductCd(), view.getProductName(),
                        view.getStockQty().add(stock.getStockQty()),
                        view.getAllocatedQty().add(stock.getAllocatedQty())));
            }
        }
        List<StockView> views = new ArrayList<>(merged.values());
        views.sort((left, right) -> left.getProductCd().compareTo(right.getProductCd()));
        return views;
    }

    /** ロット単位の在庫を入庫日の古い順に返す（先入先出の確認用）。 */
    public List<Stock> findLots(String warehouseCd, String productCd) {
        List<Stock> lots = new ArrayList<>();
        for (Stock stock : stockRepository.findByWarehouse(warehouseCd)) {
            if (stock.getProductCd().equals(productCd)) {
                lots.add(stock);
            }
        }
        lots.sort((left, right) -> {
            int byDate = left.getReceiveDate().compareTo(right.getReceiveDate());
            return byDate != 0 ? byDate : left.getLotNo().compareTo(right.getLotNo());
        });
        return lots;
    }

    /** 有効在庫数の合計を返す。 */
    public BigDecimal availableQty(String warehouseCd, String productCd) {
        BigDecimal sum = BigDecimal.ZERO;
        for (Stock lot : findLots(warehouseCd, productCd)) {
            sum = sum.add(lot.availableQty());
        }
        return sum;
    }
}
'''


INVENTORY: dict[str, str] = {
    "inventory/MoveType.java": _MOVE_TYPE,
    "inventory/StockMoveDto.java": _STOCK_MOVE_DTO,
    "inventory/StockHistory.java": _STOCK_HISTORY,
    "inventory/StockView.java": _STOCK_VIEW,
    "inventory/CountLine.java": _COUNT_LINE,
    "inventory/CountResult.java": _COUNT_RESULT,
    "inventory/ShipmentResultDto.java": _SHIPMENT_RESULT_DTO,
    "inventory/repository/StockHistoryRepository.java": _STOCK_HISTORY_REPOSITORY,
    "inventory/repository/InventoryCountRepository.java": _COUNT_REPOSITORY,
    "inventory/service/StockUpdateService.java": _STOCK_UPDATE,
    "inventory/service/InventoryCountService.java": _INVENTORY_COUNT,
    "inventory/service/ShipmentResultImportService.java": _SHIPMENT_RESULT_IMPORT,
    "inventory/service/StockInquiryService.java": _STOCK_INQUIRY,
}
