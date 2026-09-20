"""請求管理（`billing`）のうち、請求締め以外のモジュールの Java ソース。

設計書のモジュール一覧にある

- 請求書発行サービス（`InvoicePrintService`）
- 入金消込サービス（`DepositMatchingService`）
- 売上仕訳連携バッチ（`SalesJournalExportBatch`）

と、機能要件「売掛残高管理」に対応する `ReceivableInquiryService` を置く。
請求締めバッチは C3（消費税の計算単位）を仕込んである関係で
`code_sources.py` に残してある。

`ReceivableInquiryService` は `spec.MODULES` にモジュール定義が無い。機能要件
「売掛残高管理」に対応する実装が要るのに設計書がモジュールを起こしていない
ため、G6 と同じ**コードにしか無い**ものとして javadoc に書いてある。

帳票そのもの（PDF）は既存の帳票基盤（SVF）が出す。制約「帳票基盤」に従い、
`InvoicePrintService` は帳票基盤へ渡す**出力データを組み立てるところまで**を
受け持つ。
"""

from __future__ import annotations

import code_kit as ck

_BD = "java.math.BigDecimal"
_LD = "java.time.LocalDate"
_ROOT = ck.PACKAGE_ROOT


# ── 請求ステータス（コード定義書の請求ステータスと一致させる）────────────────
_INVOICE_STATUS = ck.code_enum(
    "billing/InvoiceStatus.java",
    "請求ステータス（T_INVOICE.INVOICE_STATUS）。\n *\n"
    " * 締め → 発行 → 入金の順に進む。消込が請求金額に届いた時点で消込完了とする。",
    [
        ("CLOSED", "10", "締め済"),
        ("PRINTED", "20", "発行済"),
        ("PARTIAL", "30", "一部入金"),
        ("SETTLED", "40", "消込完了"),
    ])


# ── 値クラス ──────────────────────────────────────────────────────────────
_INVOICE = ck.bean(
    "billing/Invoice.java",
    "請求ヘッダ（T_INVOICE）。締めた 1 得意先・1 請求年月ぶんの金額を持つ。",
    "String invoiceNo; String closingYm; String customerCd; BigDecimal prevBalance;"
    " BigDecimal salesAmount; BigDecimal taxAmount; *BigDecimal depositAmount;"
    " BigDecimal billingAmount; *InvoiceStatus status", [_BD],
    extra="""    /** 未回収の残高（請求金額 − 消し込んだ入金額）。 */
    public BigDecimal getUnpaidAmount() {
        return billingAmount.subtract(depositAmount);
    }""")

_SALES_SUMMARY = ck.bean(
    "billing/SalesSummary.java",
    "締め期間内の売上の集計結果。税抜の売上額・消費税額と、集計に使った\n"
    " * 売上明細を返す（請求番号の書き戻しに要るため）。",
    "BigDecimal salesAmount; BigDecimal taxAmount; List<SalesLine> lines",
    [_BD, "java.util.List"])

_BATCH_RESULT = ck.bean(
    "billing/BatchResult.java",
    "請求締めバッチの実行結果。得意先単位でコミットするので、確定・対象外・\n"
    " * 異常の件数を分けて返す。",
    "int closedCount; int skippedCount; int failedCount", [],
    extra="""    /** 処理した得意先の件数。 */
    public int getTotalCount() {
        return closedCount + skippedCount + failedCount;
    }""")

_DEPOSIT = ck.bean(
    "billing/Deposit.java",
    "入金（T_DEPOSIT）。\n *\n"
    " * 全銀協フォーマットで受信した入金明細 1 件。振込人名から得意先を特定でき\n"
    " * ない場合があるので、得意先コードは後から埋まることがある。",
    "String depositNo; LocalDate depositDate; *String customerCd;"
    " BigDecimal depositAmount; *BigDecimal appliedAmount; String payerName;"
    " *String invoiceNo", [_BD, _LD],
    extra="""    /** まだ消し込めていない金額。 */
    public BigDecimal getRemainingAmount() {
        return depositAmount.subtract(appliedAmount);
    }""")

_DEPOSIT_CANDIDATE = ck.bean(
    "billing/DepositCandidate.java",
    "入金消込の候補 1 件。請求番号が一致しなかった入金に対して提示する。",
    "String depositNo; String invoiceNo; String customerCd;"
    " BigDecimal billingAmount; BigDecimal unpaidAmount; String reason", [_BD])

_MATCH_SUMMARY = ck.bean(
    "billing/MatchSummary.java",
    "入金消込の結果。自動で消し込めた件数と、担当者の選択に回した候補を返す。",
    "int matchedCount; int unmatchedCount; List<DepositCandidate> candidates",
    ["java.util.List"])

_INVOICE_PRINT_LINE = ck.bean(
    "billing/InvoicePrintLine.java",
    "請求書の明細 1 行。帳票基盤へ渡す出力データの一部。",
    "LocalDate salesDate; String productCd; BigDecimal netAmount; String taxType",
    [_BD, _LD])

_INVOICE_PRINT_DATA = ck.bean(
    "billing/InvoicePrintData.java",
    "請求書 1 通ぶんの出力データ。\n *\n"
    " * 制約「帳票基盤」により PDF そのものは既存の帳票基盤（SVF）が出す。\n"
    " * 本クラスは帳票基盤へ渡す項目を組み立てたものである。",
    "String invoiceNo; String closingYm; String customerCd; String customerName;"
    " LocalDate issueDate; BigDecimal prevBalance; BigDecimal salesAmount;"
    " BigDecimal taxAmount; BigDecimal depositAmount; BigDecimal billingAmount;"
    " List<InvoicePrintLine> lines", [_BD, _LD, "java.util.List"])

_JOURNAL_LINE = ck.bean(
    "billing/JournalLine.java",
    "売上仕訳 1 行。会計システムへ送る固定長 CSV の 1 レコードに対応する。",
    "LocalDate journalDate; String debitAccount; String creditAccount;"
    " BigDecimal amount; String customerCd; String summary", [_BD, _LD])

_RECEIVABLE = ck.bean(
    "billing/Receivable.java",
    "売掛残高照会の 1 行。得意先ごとの残高と滞留の状況を持つ。",
    "String customerCd; String customerName; BigDecimal billingAmount;"
    " BigDecimal depositAmount; BigDecimal unpaidAmount; int overdueDays", [_BD],
    extra="""    /** 支払期日を過ぎているか（滞留）。 */
    public boolean isOverdue() {
        return overdueDays > 0 && unpaidAmount.signum() > 0;
    }""")


# ── リポジトリ ────────────────────────────────────────────────────────────
_DEPOSIT_REPOSITORY = ck.iface(
    "billing/repository/DepositRepository.java",
    "入金（T_DEPOSIT）への参照と更新。",
    [
        "List<Deposit> findByDate(LocalDate depositDate)",
        "Deposit find(String depositNo)",
        "void updateApplied(String depositNo, String invoiceNo, BigDecimal amount)",
    ],
    [_BD, _LD, "java.util.List", f"{_ROOT}.billing.Deposit"])

_JOURNAL_REPOSITORY = ck.iface(
    "billing/repository/JournalRepository.java",
    "会計システムへ渡す仕訳ファイルの出力先。\n *\n"
    " * 制約「既存システム」により会計システムは改修対象外なので、連携は\n"
    " * SFTP へ置く固定長 CSV のままとする。",
    ["void export(LocalDate targetDate, List<JournalLine> lines)"],
    [_LD, "java.util.List", f"{_ROOT}.billing.JournalLine"])


# ── 請求書発行サービス ────────────────────────────────────────────────────
_INVOICE_PRINT = '''package jp.co.contoso.sps.billing.service;

import java.time.LocalDate;
import java.util.ArrayList;
import java.util.List;

import jp.co.contoso.sps.billing.Invoice;
import jp.co.contoso.sps.billing.InvoicePrintData;
import jp.co.contoso.sps.billing.InvoicePrintLine;
import jp.co.contoso.sps.billing.InvoiceStatus;
import jp.co.contoso.sps.billing.SalesLine;
import jp.co.contoso.sps.billing.repository.InvoiceRepository;
import jp.co.contoso.sps.billing.repository.SalesRepository;
import jp.co.contoso.sps.common.AuditLogger;
import jp.co.contoso.sps.common.BusinessException;
import jp.co.contoso.sps.common.Customer;
import jp.co.contoso.sps.common.repository.CustomerRepository;
import jp.co.contoso.sps.framework.Service;
import jp.co.contoso.sps.framework.Transactional;

/**
 * 請求書発行サービス。
 *
 * 確定した請求から帳票基盤へ渡す出力データを組み立て、請求書 PDF を生成する。
 *
 * 関連する機能: 請求書発行
 * 関連する画面: 請求書発行
 * 関連する帳票: 請求書
 *
 * 制約「帳票基盤」により PDF の組版は既存の帳票基盤（SVF）が行う。本サービスは
 * 帳票基盤へ渡す出力データを組み立て、発行済（20）へ進めるところまでを行う。
 * 締め済（10）でない請求は発行できない。
 */
@Service
public class InvoicePrintService {

    private final InvoiceRepository invoiceRepository;
    private final SalesRepository salesRepository;
    private final CustomerRepository customerRepository;
    private final AuditLogger auditLogger;

    public InvoicePrintService(InvoiceRepository invoiceRepository,
                               SalesRepository salesRepository,
                               CustomerRepository customerRepository,
                               AuditLogger auditLogger) {
        this.invoiceRepository = invoiceRepository;
        this.salesRepository = salesRepository;
        this.customerRepository = customerRepository;
        this.auditLogger = auditLogger;
    }

    /** 請求年月のうち、まだ発行していない請求を返す。 */
    public List<Invoice> findPrintable(String closingYm) {
        List<Invoice> printable = new ArrayList<>();
        for (Invoice invoice : invoiceRepository.findByClosingYm(closingYm)) {
            if (invoice.getStatus() == InvoiceStatus.CLOSED) {
                printable.add(invoice);
            }
        }
        return printable;
    }

    /**
     * 請求 1 件ぶんの出力データを組み立てる。
     *
     * 明細は締めのときに請求番号を書き戻した売上明細から引く。
     */
    public InvoicePrintData buildPrintData(String invoiceNo, LocalDate issueDate) {
        Invoice invoice = invoiceRepository.find(invoiceNo);
        if (invoice == null) {
            throw new BusinessException("請求が存在しません。（請求番号:" + invoiceNo + "）");
        }
        Customer customer = customerRepository.find(invoice.getCustomerCd());
        List<InvoicePrintLine> lines = new ArrayList<>();
        for (SalesLine line : salesRepository.findByInvoiceNo(invoiceNo)) {
            lines.add(new InvoicePrintLine(line.getSalesDate(), line.getProductCd(),
                    line.getNetAmount(), line.getTaxType()));
        }
        return new InvoicePrintData(invoice.getInvoiceNo(), invoice.getClosingYm(),
                invoice.getCustomerCd(), customer == null ? "" : customer.getName(),
                issueDate, invoice.getPrevBalance(), invoice.getSalesAmount(),
                invoice.getTaxAmount(), invoice.getDepositAmount(),
                invoice.getBillingAmount(), lines);
    }

    /**
     * 請求書を発行する。
     *
     * 出力データを帳票基盤へ渡し、請求ステータスを発行済（20）へ進める。
     * 締め済でない請求を指定した場合は業務エラーとする。
     */
    @Transactional
    public InvoicePrintData print(String invoiceNo, LocalDate issueDate, String staffCd) {
        Invoice invoice = invoiceRepository.find(invoiceNo);
        if (invoice == null) {
            throw new BusinessException("請求が存在しません。（請求番号:" + invoiceNo + "）");
        }
        if (invoice.getStatus() != InvoiceStatus.CLOSED) {
            throw new BusinessException("締め済の請求だけを発行できます。（請求番号:"
                    + invoiceNo + " ステータス:" + invoice.getStatus().getLabel() + "）");
        }
        InvoicePrintData data = buildPrintData(invoiceNo, issueDate);
        invoiceRepository.updateStatus(invoiceNo, InvoiceStatus.PRINTED);
        auditLogger.record(staffCd, "請求書発行", invoiceNo,
                InvoiceStatus.CLOSED.getLabel(), InvoiceStatus.PRINTED.getLabel());
        return data;
    }
}
'''


# ── 入金消込サービス ──────────────────────────────────────────────────────
_DEPOSIT_MATCHING = '''package jp.co.contoso.sps.billing.service;

import java.math.BigDecimal;
import java.time.LocalDate;
import java.util.ArrayList;
import java.util.List;

import jp.co.contoso.sps.billing.Deposit;
import jp.co.contoso.sps.billing.DepositCandidate;
import jp.co.contoso.sps.billing.Invoice;
import jp.co.contoso.sps.billing.InvoiceStatus;
import jp.co.contoso.sps.billing.MatchSummary;
import jp.co.contoso.sps.billing.repository.DepositRepository;
import jp.co.contoso.sps.billing.repository.InvoiceRepository;
import jp.co.contoso.sps.common.AuditLogger;
import jp.co.contoso.sps.common.BusinessException;
import jp.co.contoso.sps.framework.Service;
import jp.co.contoso.sps.framework.Transactional;

/**
 * 入金消込サービス。
 *
 * 入金データと請求を突き合わせ、自動消込と候補提示を行う。
 *
 * 関連する機能: 入金消込
 * 関連する画面: 入金消込
 * 関連する外部インターフェース: 入金データ受信
 * 関連する業務ルール: 入金消込の突合順序
 *
 * 突合は請求番号の一致を最優先とし、一致しない場合は「得意先 + 金額」の一致で
 * 候補を提示して担当者が選択する。自動で消し込むのは請求番号が一致した場合
 * だけで、候補は消し込まずに返す。
 */
@Service
public class DepositMatchingService {

    private final DepositRepository depositRepository;
    private final InvoiceRepository invoiceRepository;
    private final AuditLogger auditLogger;

    public DepositMatchingService(DepositRepository depositRepository,
                                  InvoiceRepository invoiceRepository,
                                  AuditLogger auditLogger) {
        this.depositRepository = depositRepository;
        this.invoiceRepository = invoiceRepository;
        this.auditLogger = auditLogger;
    }

    /**
     * 入金データを請求番号一致・得意先金額一致の順に突き合わせる。
     *
     * 関連する業務ルール: 入金消込の突合順序
     */
    @Transactional
    public MatchSummary matchDeposits(LocalDate depositDate) {
        int matched = 0;
        List<DepositCandidate> candidates = new ArrayList<>();
        for (Deposit deposit : depositRepository.findByDate(depositDate)) {
            if (deposit.getRemainingAmount().compareTo(BigDecimal.ZERO) <= 0) {
                continue;
            }
            Invoice byNumber = deposit.getInvoiceNo() == null
                    ? null
                    : invoiceRepository.find(deposit.getInvoiceNo());
            if (byNumber != null) {
                applyDeposit(deposit.getDepositNo(), byNumber.getInvoiceNo(),
                        deposit.getRemainingAmount().min(byNumber.getUnpaidAmount()),
                        AuditLogger.SYSTEM);
                matched++;
                continue;
            }
            candidates.addAll(findCandidates(deposit));
        }
        auditLogger.record("入金消込", depositDate.toString(), null,
                "自動消込 " + matched + " 件 / 候補 " + candidates.size() + " 件");
        return new MatchSummary(matched, candidates.size(), candidates);
    }

    /**
     * 得意先と金額の一致で消込の候補を探す。
     *
     * 得意先が特定できない入金は候補を出せない（振込人名からの推定は行わない）。
     */
    public List<DepositCandidate> findCandidates(Deposit deposit) {
        List<DepositCandidate> candidates = new ArrayList<>();
        if (deposit.getCustomerCd() == null) {
            return candidates;
        }
        for (Invoice invoice : invoiceRepository.findUnpaid(deposit.getCustomerCd())) {
            String reason = invoice.getUnpaidAmount()
                    .compareTo(deposit.getRemainingAmount()) == 0
                    ? "得意先と金額が一致"
                    : "得意先が一致（金額は不一致）";
            candidates.add(new DepositCandidate(deposit.getDepositNo(),
                    invoice.getInvoiceNo(), invoice.getCustomerCd(),
                    invoice.getBillingAmount(), invoice.getUnpaidAmount(), reason));
        }
        return candidates;
    }

    /**
     * 指定した請求へ入金を充当し、請求ステータスを更新する。
     *
     * 未回収残高に届いたら消込完了（40）、届かなければ一部入金（30）とする。
     * 充当額が入金の残額や請求の未回収残高を超える指定は業務エラーとする。
     */
    @Transactional
    public void applyDeposit(String depositNo, String invoiceNo, BigDecimal amount,
                             String staffCd) {
        Deposit deposit = depositRepository.find(depositNo);
        Invoice invoice = invoiceRepository.find(invoiceNo);
        if (deposit == null || invoice == null) {
            throw new BusinessException("入金または請求が存在しません。");
        }
        if (amount == null || amount.compareTo(BigDecimal.ZERO) <= 0) {
            throw new BusinessException("充当額は0より大きい値を入力してください。");
        }
        if (amount.compareTo(deposit.getRemainingAmount()) > 0) {
            throw new BusinessException("入金の残額を超えて充当できません。（残額:"
                    + deposit.getRemainingAmount() + "）");
        }
        if (amount.compareTo(invoice.getUnpaidAmount()) > 0) {
            throw new BusinessException("請求の未回収残高を超えて充当できません。（残高:"
                    + invoice.getUnpaidAmount() + "）");
        }

        depositRepository.updateApplied(depositNo, invoiceNo, amount);
        BigDecimal applied = invoice.getDepositAmount().add(amount);
        InvoiceStatus status = applied.compareTo(invoice.getBillingAmount()) >= 0
                ? InvoiceStatus.SETTLED
                : InvoiceStatus.PARTIAL;
        invoiceRepository.updateDeposit(invoiceNo, applied, status);
        auditLogger.record(staffCd, "入金消込", invoiceNo,
                invoice.getDepositAmount(), applied);
    }
}
'''


# ── 売上仕訳連携バッチ ────────────────────────────────────────────────────
_SALES_JOURNAL = '''package jp.co.contoso.sps.billing.batch;

import java.math.BigDecimal;
import java.time.LocalDate;
import java.util.ArrayList;
import java.util.List;

import jp.co.contoso.sps.billing.JournalLine;
import jp.co.contoso.sps.billing.SalesLine;
import jp.co.contoso.sps.billing.TaxGroup;
import jp.co.contoso.sps.billing.repository.JournalRepository;
import jp.co.contoso.sps.billing.repository.SalesRepository;
import jp.co.contoso.sps.common.AuditLogger;
import jp.co.contoso.sps.common.TaxCalculator;
import jp.co.contoso.sps.framework.Component;

/**
 * 売上仕訳連携バッチ。
 *
 * 確定した売上を仕訳データに変換し、会計システム向けの固定長ファイルを出力する。
 *
 * 関連する機能: 会計システム連携
 * 関連する外部インターフェース: 売上仕訳連携
 *
 * 日次 1 回、当日ぶんの売上を対象に起動する。制約「既存システム」により会計
 * システム（ライトウェア会計）は改修対象外なので、勘定科目と仕訳の形は現行を
 * 踏襲する。売上は税抜額と仮受消費税に分けて計上する。
 */
@Component
public class SalesJournalExportBatch {

    /** 借方の勘定科目。売上は売掛金の増加として立てる。 */
    private static final String DEBIT_ACCOUNT = "1310";

    /** 貸方の勘定科目（売上高）。 */
    private static final String CREDIT_SALES = "4110";

    /** 貸方の勘定科目（仮受消費税）。 */
    private static final String CREDIT_TAX = "2180";

    private final SalesRepository salesRepository;
    private final JournalRepository journalRepository;
    private final TaxCalculator taxCalculator;
    private final AuditLogger auditLogger;

    public SalesJournalExportBatch(SalesRepository salesRepository,
                                   JournalRepository journalRepository,
                                   TaxCalculator taxCalculator,
                                   AuditLogger auditLogger) {
        this.salesRepository = salesRepository;
        this.journalRepository = journalRepository;
        this.taxCalculator = taxCalculator;
        this.auditLogger = auditLogger;
    }

    /**
     * 対象日の売上を仕訳へ変換し、会計システム向けのファイルを出力する。
     *
     * 出力した仕訳の行数を返す。対象の売上が無い場合も空のファイルを出す
     * （会計システム側が未着とファイル無しを区別できないため）。
     */
    public int execute(LocalDate targetDate) {
        List<SalesLine> lines = salesRepository.findByDate(targetDate);
        List<JournalLine> journal = convertToJournal(targetDate, lines);
        journalRepository.export(targetDate, journal);
        auditLogger.record("売上仕訳連携", targetDate.toString(), null,
                journal.size() + " 行");
        return journal.size();
    }

    /**
     * 売上を得意先・税区分ごとにまとめ、仕訳の行へ変換する。
     *
     * 消費税は税区分ごとに集計してから計算する（請求締めと同じ単位にそろえる）。
     */
    public List<JournalLine> convertToJournal(LocalDate targetDate,
                                              List<SalesLine> lines) {
        List<JournalLine> journal = new ArrayList<>();
        for (String customerCd : customersOf(lines)) {
            List<SalesLine> ofCustomer = new ArrayList<>();
            for (SalesLine line : lines) {
                if (line.getCustomerCd().equals(customerCd)) {
                    ofCustomer.add(line);
                }
            }
            BigDecimal netTotal = BigDecimal.ZERO;
            BigDecimal taxTotal = BigDecimal.ZERO;
            for (TaxGroup group : TaxGroup.groupBy(ofCustomer)) {
                netTotal = netTotal.add(group.getNetAmount());
                taxTotal = taxTotal.add(taxCalculator.calcTax(
                        group.getNetAmount(), group.getTaxType()));
            }
            journal.add(new JournalLine(targetDate, DEBIT_ACCOUNT, CREDIT_SALES,
                    netTotal, customerCd, "売上高"));
            if (taxTotal.compareTo(BigDecimal.ZERO) != 0) {
                journal.add(new JournalLine(targetDate, DEBIT_ACCOUNT, CREDIT_TAX,
                        taxTotal, customerCd, "仮受消費税"));
            }
        }
        return journal;
    }

    private static List<String> customersOf(List<SalesLine> lines) {
        List<String> customers = new ArrayList<>();
        for (SalesLine line : lines) {
            if (!customers.contains(line.getCustomerCd())) {
                customers.add(line.getCustomerCd());
            }
        }
        return customers;
    }
}
'''


# ── 売掛残高照会サービス ──────────────────────────────────────────────────
_RECEIVABLE_INQUIRY = '''package jp.co.contoso.sps.billing.service;

import java.math.BigDecimal;
import java.time.LocalDate;
import java.time.temporal.ChronoUnit;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

import jp.co.contoso.sps.billing.Invoice;
import jp.co.contoso.sps.billing.InvoiceStatus;
import jp.co.contoso.sps.billing.Receivable;
import jp.co.contoso.sps.billing.repository.InvoiceRepository;
import jp.co.contoso.sps.common.Customer;
import jp.co.contoso.sps.common.repository.CustomerRepository;
import jp.co.contoso.sps.framework.Service;

/**
 * 売掛残高照会サービス。
 *
 * 得意先ごとの売掛残高と滞留状況を照会する。
 *
 * 関連する機能: 売掛残高管理
 * 関連する画面: 売掛残高照会
 * 関連する帳票: 売掛残高一覧表
 *
 * 詳細設計にモジュールの定義が無い。機能要件「売掛残高管理」に対応する実装が
 * 要るのに設計書がモジュールを起こしていないため、コード側だけに存在する。
 *
 * 滞留は「支払期日を過ぎても入金されていない売掛」（用語集）とする。支払条件は
 * 得意先ごとに違うので、ここでは締め月の翌月末を支払期日とみなす。
 */
@Service
public class ReceivableInquiryService {

    /** 締め月の何か月後の末日を支払期日とみなすか。 */
    private static final int PAYMENT_MONTHS = 1;

    private final InvoiceRepository invoiceRepository;
    private final CustomerRepository customerRepository;

    public ReceivableInquiryService(InvoiceRepository invoiceRepository,
                                    CustomerRepository customerRepository) {
        this.invoiceRepository = invoiceRepository;
        this.customerRepository = customerRepository;
    }

    /**
     * 得意先ごとの売掛残高を返す。
     *
     * 消込完了（40）の請求は残高に含めない。得意先コードを指定した場合は
     * その得意先だけに絞る。
     */
    public List<Receivable> search(String customerCd, LocalDate baseDate) {
        Map<String, Receivable> merged = new LinkedHashMap<>();
        for (Invoice invoice : invoiceRepository.findAll()) {
            if (invoice.getStatus() == InvoiceStatus.SETTLED) {
                continue;
            }
            if (customerCd != null && !customerCd.isBlank()
                    && !invoice.getCustomerCd().equals(customerCd)) {
                continue;
            }
            int overdue = overdueDays(invoice.getClosingYm(), baseDate);
            Receivable current = merged.get(invoice.getCustomerCd());
            if (current == null) {
                Customer customer = customerRepository.find(invoice.getCustomerCd());
                merged.put(invoice.getCustomerCd(), new Receivable(
                        invoice.getCustomerCd(),
                        customer == null ? "" : customer.getName(),
                        invoice.getBillingAmount(), invoice.getDepositAmount(),
                        invoice.getUnpaidAmount(), overdue));
            } else {
                merged.put(invoice.getCustomerCd(), new Receivable(
                        current.getCustomerCd(), current.getCustomerName(),
                        current.getBillingAmount().add(invoice.getBillingAmount()),
                        current.getDepositAmount().add(invoice.getDepositAmount()),
                        current.getUnpaidAmount().add(invoice.getUnpaidAmount()),
                        Math.max(current.getOverdueDays(), overdue)));
            }
        }
        return new ArrayList<>(merged.values());
    }

    /** 得意先 1 件の未回収残高の合計。 */
    public BigDecimal unpaidTotal(String customerCd, LocalDate baseDate) {
        BigDecimal sum = BigDecimal.ZERO;
        for (Receivable receivable : search(customerCd, baseDate)) {
            sum = sum.add(receivable.getUnpaidAmount());
        }
        return sum;
    }

    /** 支払期日（締め月の翌月末）からの経過日数。期日前なら 0。 */
    public int overdueDays(String closingYm, LocalDate baseDate) {
        int year = Integer.parseInt(closingYm.substring(0, 4));
        int month = Integer.parseInt(closingYm.substring(4, 6));
        LocalDate closing = LocalDate.of(year, month, 1).plusMonths(PAYMENT_MONTHS);
        LocalDate due = closing.withDayOfMonth(closing.lengthOfMonth());
        if (!baseDate.isAfter(due)) {
            return 0;
        }
        return (int) ChronoUnit.DAYS.between(due, baseDate);
    }
}
'''


BILLING: dict[str, str] = {
    "billing/InvoiceStatus.java": _INVOICE_STATUS,
    "billing/Invoice.java": _INVOICE,
    "billing/SalesSummary.java": _SALES_SUMMARY,
    "billing/BatchResult.java": _BATCH_RESULT,
    "billing/Deposit.java": _DEPOSIT,
    "billing/DepositCandidate.java": _DEPOSIT_CANDIDATE,
    "billing/MatchSummary.java": _MATCH_SUMMARY,
    "billing/InvoicePrintLine.java": _INVOICE_PRINT_LINE,
    "billing/InvoicePrintData.java": _INVOICE_PRINT_DATA,
    "billing/JournalLine.java": _JOURNAL_LINE,
    "billing/Receivable.java": _RECEIVABLE,
    "billing/repository/DepositRepository.java": _DEPOSIT_REPOSITORY,
    "billing/repository/JournalRepository.java": _JOURNAL_REPOSITORY,
    "billing/service/InvoicePrintService.java": _INVOICE_PRINT,
    "billing/service/DepositMatchingService.java": _DEPOSIT_MATCHING,
    "billing/batch/SalesJournalExportBatch.java": _SALES_JOURNAL,
    "billing/service/ReceivableInquiryService.java": _RECEIVABLE_INQUIRY,
}
