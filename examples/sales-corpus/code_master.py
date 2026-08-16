"""共通基盤（`common`）のうち、マスタ保守と監査ログ基盤の Java ソース。

機能要件「マスタ保守」「監査ログ」と、非機能要件「監査ログの保存期間」
「保守性」に対応する部分をここに置く。

- 商品マスタ・倉庫マスタの値クラスとリポジトリ（受注・在庫が名前で引く）
- 監査ログを**テーブルへ**書く `AuditLogger`。ハッシュ連鎖で改ざんを検知でき、
  保存期間 5 年を過ぎた行を削除する（非機能要件「監査ログの保存期間」）
- 締め日を**マスタから**引く `ClosingDayRepository`（非機能要件「保守性」の
  「締め日はプログラム修正なしに変更できること」に対応する）
- マスタ保守サービス

`MasterMaintenanceService` は `spec.MODULES` にモジュール定義が無い。機能要件
「マスタ保守」に対応する実装が必要なのに設計書がモジュールを起こしていないため、
G6（得意先コードの前ゼロ埋め）と同じ**コードにしか無い**ものとして javadoc に
その旨を書いてある。
"""

from __future__ import annotations

import code_kit as ck

_BD = "java.math.BigDecimal"
_LD = "java.time.LocalDate"
_LDT = "java.time.LocalDateTime"
_ROOT = ck.PACKAGE_ROOT


# ── マスタの値クラス ──────────────────────────────────────────────────────
_PRODUCT = ck.bean(
    "common/Product.java",
    "商品マスタ（M_PRODUCT）。\n *\n"
    " * 標準単価・税区分・保管区分を持つ。削除フラグは論理削除で、1 の商品は\n"
    " * 検索対象から外す。",
    "String productCd; *String productName; *String janCd; *String unit;"
    " *BigDecimal caseQty; *BigDecimal stdPrice; *String taxType;"
    " *String storageType; *boolean deleted", [_BD])

_WAREHOUSE = ck.bean(
    "common/Warehouse.java",
    "倉庫マスタ（M_WAREHOUSE）。\n *\n"
    " * 在庫を置く倉庫。保管区分は倉庫が扱える温度帯を表す。",
    "String warehouseCd; *String warehouseName; *String storageType;"
    " *boolean deleted", [])

_AUDIT_LOG = ck.bean(
    "common/AuditLog.java",
    "監査ログ 1 件（T_AUDIT_LOG）。\n *\n"
    " * 実施者・日時・操作・変更前後の値に加え、直前の行のハッシュと自身の\n"
    " * ハッシュを持つ。ハッシュが前の行と鎖になっているので、途中の行を\n"
    " * 書き換えると以降の検証が失敗する（非機能要件「監査ログの保存期間」）。",
    "LocalDateTime recordedAt; String staffCd; String operation; String targetKey;"
    " String beforeValue; String afterValue; String previousHash; String hash",
    [_LDT])


# ── リポジトリ ────────────────────────────────────────────────────────────
_PRODUCT_REPOSITORY = ck.iface(
    "common/repository/ProductRepository.java",
    "商品マスタ（M_PRODUCT）への参照と更新。",
    [
        "Product find(String productCd)",
        "Product findByJan(String janCd)",
        "List<Product> findActive()",
        "void save(Product product)",
        "void logicalDelete(String productCd)",
    ],
    ["java.util.List", f"{_ROOT}.common.Product"])

_WAREHOUSE_REPOSITORY = ck.iface(
    "common/repository/WarehouseRepository.java",
    "倉庫マスタ（M_WAREHOUSE）への参照と更新。",
    [
        "Warehouse find(String warehouseCd)",
        "List<Warehouse> findActive()",
        "void save(Warehouse warehouse)",
        "void logicalDelete(String warehouseCd)",
    ],
    ["java.util.List", f"{_ROOT}.common.Warehouse"])

_AUDIT_LOG_REPOSITORY = ck.iface(
    "common/repository/AuditLogRepository.java",
    "監査ログ（T_AUDIT_LOG）への追記と参照。\n *\n"
    " * 更新も部分削除も持たせない。消せるのは保存期間を過ぎた行だけである。",
    [
        "void insert(AuditLog log)",
        "String lastHash()",
        "List<AuditLog> findAll()",
        "int deleteBefore(LocalDate limit)",
    ],
    [_LD, "java.util.List", f"{_ROOT}.common.AuditLog"])

_CLOSING_DAY_REPOSITORY = ck.iface(
    "common/repository/ClosingDayRepository.java",
    "締め日マスタ（M_CLOSING_DAY）への参照。\n *\n"
    " * 締日区分ごとの締め日（何日で締めるか）を保持する。非機能要件「保守性」が\n"
    " * 「締め日はプログラム修正なしに変更できること」を求めるため、20 日・末日を\n"
    " * プログラムに埋め込まずここから引く。末日は 31 を入れておく。",
    ["Integer findClosingDay(String closingTypeCd)"], [])


# ── 監査ログ出力部品 ──────────────────────────────────────────────────────
_AUDIT_LOGGER = '''package jp.co.contoso.sps.common;

import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.time.LocalDate;
import java.time.LocalDateTime;
import java.util.HexFormat;
import java.util.List;

import jp.co.contoso.sps.common.repository.AuditLogRepository;
import jp.co.contoso.sps.framework.Component;

/**
 * 監査ログ出力部品。
 *
 * 更新操作の実施者・日時・変更前後の値を監査ログテーブルへ記録する。
 * 受注・在庫・請求の更新操作を対象とする。
 *
 * 関連する機能: 監査ログ出力
 * 関連する非機能要件: 監査ログの保存期間
 *
 * 非機能要件「監査ログの保存期間」は 5 年保存と改ざんできない形式での保管を
 * 求める。改ざん防止は行のハッシュを直前の行のハッシュと連鎖させることで
 * 実現する（途中の行を書き換えると {@link #verifyChain()} が偽を返す）。
 */
@Component
public class AuditLogger {

    /** 監査ログの保存年数。 */
    private static final int RETENTION_YEARS = 5;

    /** 実施者が利用者でない場合（バッチ）に記録する実施者コード。 */
    public static final String SYSTEM = "BATCH";

    /** 連鎖の先頭に置く値。 */
    private static final String GENESIS = "0000000000000000000000000000000000000000000000000000000000000000";

    private final AuditLogRepository auditLogRepository;

    public AuditLogger(AuditLogRepository auditLogRepository) {
        this.auditLogRepository = auditLogRepository;
    }

    /**
     * 監査ログを 1 件記録する。
     *
     * 直前の行のハッシュを取り込んで自身のハッシュを求め、追記する。
     * 出力に失敗した場合は呼出元を異常終了させる。
     */
    public void record(String staffCd, String operation, String key,
                       Object before, Object after) {
        LocalDateTime now = LocalDateTime.now();
        String previousHash = auditLogRepository.lastHash();
        if (previousHash == null) {
            previousHash = GENESIS;
        }
        String actor = staffCd == null ? SYSTEM : staffCd;
        String beforeValue = text(before);
        String afterValue = text(after);
        String hash = digest(previousHash + "|" + now + "|" + actor + "|" + operation
                + "|" + key + "|" + beforeValue + "|" + afterValue);
        auditLogRepository.insert(new AuditLog(now, actor, operation, key,
                beforeValue, afterValue, previousHash, hash));
        System.out.printf("[監査ログ] %s %s key=%s before=%s after=%s%n",
                actor, operation, key, beforeValue, afterValue);
    }

    /** 実施者が利用者でない処理（バッチ）から記録する。 */
    public void record(String operation, String key, Object before, Object after) {
        record(SYSTEM, operation, key, before, after);
    }

    /**
     * ハッシュの連鎖が途切れていないかを検証する。
     *
     * 1 行でも書き換えられていれば偽を返す。
     */
    public boolean verifyChain() {
        String previousHash = GENESIS;
        List<AuditLog> logs = auditLogRepository.findAll();
        for (AuditLog log : logs) {
            if (!previousHash.equals(log.getPreviousHash())) {
                return false;
            }
            String expected = digest(log.getPreviousHash() + "|" + log.getRecordedAt()
                    + "|" + log.getStaffCd() + "|" + log.getOperation()
                    + "|" + log.getTargetKey() + "|" + log.getBeforeValue()
                    + "|" + log.getAfterValue());
            if (!expected.equals(log.getHash())) {
                return false;
            }
            previousHash = log.getHash();
        }
        return true;
    }

    /** 保存期間（5 年）を過ぎた監査ログを削除し、消した件数を返す。 */
    public int purgeExpired(LocalDate today) {
        return auditLogRepository.deleteBefore(today.minusYears(RETENTION_YEARS));
    }

    /** 保存年数を返す。 */
    public int getRetentionYears() {
        return RETENTION_YEARS;
    }

    private static String text(Object value) {
        return value == null ? "" : String.valueOf(value);
    }

    private static String digest(String source) {
        try {
            MessageDigest sha = MessageDigest.getInstance("SHA-256");
            return HexFormat.of().formatHex(
                    sha.digest(source.getBytes(StandardCharsets.UTF_8)));
        } catch (NoSuchAlgorithmException e) {
            throw new IllegalStateException("監査ログのハッシュ計算に失敗しました。", e);
        }
    }
}
'''


# ── マスタ保守サービス ────────────────────────────────────────────────────
_MASTER_MAINTENANCE = '''package jp.co.contoso.sps.common.service;

import java.math.BigDecimal;
import java.util.ArrayList;
import java.util.List;

import jp.co.contoso.sps.common.AuditLogger;
import jp.co.contoso.sps.common.Customer;
import jp.co.contoso.sps.common.Product;
import jp.co.contoso.sps.common.Staff;
import jp.co.contoso.sps.common.Warehouse;
import jp.co.contoso.sps.common.repository.CustomerRepository;
import jp.co.contoso.sps.common.repository.ProductRepository;
import jp.co.contoso.sps.common.repository.StaffRepository;
import jp.co.contoso.sps.common.repository.WarehouseRepository;
import jp.co.contoso.sps.framework.Service;
import jp.co.contoso.sps.framework.Transactional;
import jp.co.contoso.sps.order.ErrorInfo;

/**
 * マスタ保守サービス。
 *
 * 得意先・商品・倉庫・社員のマスタを画面から保守する。削除は物理削除ではなく
 * 削除フラグを立てる論理削除とする。
 *
 * 関連する機能: マスタ保守
 * 関連する画面: 得意先マスタ保守 / 商品マスタ保守
 *
 * 詳細設計にモジュールの定義が無い。機能要件「マスタ保守」に対応する実装が
 * 要るのに設計書がモジュールを起こしていないため、コード側だけに存在する。
 * 得意先コードの前ゼロ埋め（G6）と同じ性質のものである。
 */
@Service
public class MasterMaintenanceService {

    /** 得意先コードの桁数。 */
    private static final int CUSTOMER_CD_LENGTH = 8;

    /** 商品コードの桁数。 */
    private static final int PRODUCT_CD_LENGTH = 10;

    private final CustomerRepository customerRepository;
    private final ProductRepository productRepository;
    private final WarehouseRepository warehouseRepository;
    private final StaffRepository staffRepository;
    private final AuditLogger auditLogger;

    public MasterMaintenanceService(CustomerRepository customerRepository,
                                    ProductRepository productRepository,
                                    WarehouseRepository warehouseRepository,
                                    StaffRepository staffRepository,
                                    AuditLogger auditLogger) {
        this.customerRepository = customerRepository;
        this.productRepository = productRepository;
        this.warehouseRepository = warehouseRepository;
        this.staffRepository = staffRepository;
        this.auditLogger = auditLogger;
    }

    /** 得意先を登録・変更する。 */
    @Transactional
    public List<ErrorInfo> saveCustomer(Customer customer, String staffCd) {
        List<ErrorInfo> errors = validateCustomer(customer);
        if (!errors.isEmpty()) {
            return errors;
        }
        Customer before = customerRepository.find(customer.getCustomerCd());
        customerRepository.save(customer);
        auditLogger.record(staffCd, "得意先マスタ保守", customer.getCustomerCd(),
                before == null ? null : before.getName(), customer.getName());
        return errors;
    }

    /** 得意先を論理削除する。 */
    @Transactional
    public void deleteCustomer(String customerCd, String staffCd) {
        Customer before = customerRepository.find(customerCd);
        customerRepository.logicalDelete(customerCd);
        auditLogger.record(staffCd, "得意先マスタ削除", customerCd,
                before == null ? null : before.getName(), "削除");
    }

    /** 商品を登録・変更する。 */
    @Transactional
    public List<ErrorInfo> saveProduct(Product product, String staffCd) {
        List<ErrorInfo> errors = validateProduct(product);
        if (!errors.isEmpty()) {
            return errors;
        }
        Product before = productRepository.find(product.getProductCd());
        productRepository.save(product);
        auditLogger.record(staffCd, "商品マスタ保守", product.getProductCd(),
                before == null ? null : before.getProductName(), product.getProductName());
        return errors;
    }

    /** 商品を論理削除する。 */
    @Transactional
    public void deleteProduct(String productCd, String staffCd) {
        Product before = productRepository.find(productCd);
        productRepository.logicalDelete(productCd);
        auditLogger.record(staffCd, "商品マスタ削除", productCd,
                before == null ? null : before.getProductName(), "削除");
    }

    /** 倉庫を登録・変更する。 */
    @Transactional
    public void saveWarehouse(Warehouse warehouse, String staffCd) {
        Warehouse before = warehouseRepository.find(warehouse.getWarehouseCd());
        warehouseRepository.save(warehouse);
        auditLogger.record(staffCd, "倉庫マスタ保守", warehouse.getWarehouseCd(),
                before == null ? null : before.getWarehouseName(),
                warehouse.getWarehouseName());
    }

    /** 社員を登録・変更する。パスワードは本サービスでは扱わない。 */
    @Transactional
    public void saveStaff(Staff staff, String staffCd) {
        Staff before = staffRepository.find(staff.getStaffCd());
        staffRepository.save(staff);
        auditLogger.record(staffCd, "社員マスタ保守", staff.getStaffCd(),
                before == null ? null : before.getStaffName(), staff.getStaffName());
    }

    /** 削除されていない得意先を返す。 */
    public List<Customer> listCustomers() {
        return customerRepository.findActive();
    }

    /** 削除されていない商品を返す。 */
    public List<Product> listProducts() {
        return productRepository.findActive();
    }

    /** 削除されていない倉庫を返す。 */
    public List<Warehouse> listWarehouses() {
        return warehouseRepository.findActive();
    }

    /** 得意先の入力値を検証する。 */
    public List<ErrorInfo> validateCustomer(Customer customer) {
        List<ErrorInfo> errors = new ArrayList<>();
        if (customer.getCustomerCd() == null
                || customer.getCustomerCd().length() != CUSTOMER_CD_LENGTH) {
            errors.add(ErrorInfo.of("customerCd",
                    "得意先コードは" + CUSTOMER_CD_LENGTH + "桁で入力してください。"));
        }
        if (customer.getName() == null || customer.getName().isBlank()) {
            errors.add(ErrorInfo.of("customerName", "得意先名を入力してください。"));
        }
        if (customer.getCreditLimit() == null
                || customer.getCreditLimit().compareTo(BigDecimal.ZERO) < 0) {
            errors.add(ErrorInfo.of("creditLimit", "与信限度額を入力してください。"));
        }
        return errors;
    }

    /** 商品の入力値を検証する。 */
    public List<ErrorInfo> validateProduct(Product product) {
        List<ErrorInfo> errors = new ArrayList<>();
        if (product.getProductCd() == null
                || product.getProductCd().length() != PRODUCT_CD_LENGTH) {
            errors.add(ErrorInfo.of("productCd",
                    "商品コードは" + PRODUCT_CD_LENGTH + "桁以下で入力してください。"));
        }
        if (product.getProductName() == null || product.getProductName().isBlank()) {
            errors.add(ErrorInfo.of("productName", "商品名を入力してください。"));
        }
        if (product.getStdPrice() == null
                || product.getStdPrice().compareTo(BigDecimal.ZERO) < 0) {
            errors.add(ErrorInfo.of("stdPrice", "標準単価を入力してください。"));
        }
        return errors;
    }
}
'''


MASTER: dict[str, str] = {
    "common/Product.java": _PRODUCT,
    "common/Warehouse.java": _WAREHOUSE,
    "common/AuditLog.java": _AUDIT_LOG,
    "common/AuditLogger.java": _AUDIT_LOGGER,
    "common/repository/ProductRepository.java": _PRODUCT_REPOSITORY,
    "common/repository/WarehouseRepository.java": _WAREHOUSE_REPOSITORY,
    "common/repository/AuditLogRepository.java": _AUDIT_LOG_REPOSITORY,
    "common/repository/ClosingDayRepository.java": _CLOSING_DAY_REPOSITORY,
    "common/service/MasterMaintenanceService.java": _MASTER_MAINTENANCE,
}
