"""ブラウザから触れる画面を提供する `demo` の HTTP サーバ。

`資料/` の画面一覧にある 16 画面を、ブラウザから実際に操作できる形で動かす
ための足場。`demo` パッケージの位置づけ（動かすための足場であって設計書に
対応する成果物ではない）は `Main` と同じで、画面が呼ぶ業務ロジックの本体は
`order` / `inventory` / `billing` / `common` のサービス側にある。

外部依存を持たない方針を守るため、サーバは JDK 内蔵の `com.sun.net.httpserver`
だけで組む。待ち受けはループバック（127.0.0.1）に限る。

1 クラスに 16 画面を詰めると読めなくなるので、HTTP と HTML の下請けを `Web`、
画面をサブシステムごとの 4 クラスに分けてある。

Java の文字列に `\\n` や `\\"` をそのまま残したいので、この module の Java ソースは
raw 文字列（``r'''``）で書く。
"""

from __future__ import annotations

# ── HTTP と HTML の下請け ──────────────────────────────────────────────────
_WEB = r'''package jp.co.contoso.sps.demo;

import com.sun.net.httpserver.HttpExchange;

import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.math.BigDecimal;
import java.net.URLDecoder;
import java.nio.charset.StandardCharsets;
import java.time.LocalDate;
import java.util.HashMap;
import java.util.Map;

/**
 * 画面を組み立てるための下請け（HTTP の読み書きと HTML の組み立て）。
 *
 * 配布サンプルを動かすための足場で、設計書に対応する成果物ではない。
 * 本番は画面フレームワークとテンプレートエンジンが受け持つところにあたる。
 */
public final class Web {

    private Web() {
    }

    /** 行を改行でつないで 1 つの HTML 片にする。 */
    public static String html(String... lines) {
        return String.join("\n", lines) + "\n";
    }

    /** 画面に出す前に、記号を実体参照へ置き換える。 */
    public static String escape(String text) {
        if (text == null) {
            return "";
        }
        return text.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")
                .replace("\"", "&quot;").replace("'", "&#39;");
    }

    /** null を空文字にして返す。 */
    public static String text(Object value) {
        return value == null ? "" : String.valueOf(value);
    }

    /** POST の本文（application/x-www-form-urlencoded）を読む。 */
    public static Map<String, String> form(HttpExchange exchange) throws IOException {
        String body;
        try (InputStream in = exchange.getRequestBody()) {
            body = new String(in.readAllBytes(), StandardCharsets.UTF_8);
        }
        return parse(body);
    }

    /** POST の本文のうち、同じ名前で何度も送られる項目をすべて読む。 */
    public static java.util.List<String> formAll(String body, String key) {
        java.util.List<String> values = new java.util.ArrayList<>();
        for (String pair : body.split("&")) {
            int eq = pair.indexOf('=');
            if (eq > 0 && decode(pair.substring(0, eq)).equals(key)) {
                values.add(decode(pair.substring(eq + 1)));
            }
        }
        return values;
    }

    /** POST の本文を文字列のまま読む（複数選択のチェックボックス用）。 */
    public static String body(HttpExchange exchange) throws IOException {
        try (InputStream in = exchange.getRequestBody()) {
            return new String(in.readAllBytes(), StandardCharsets.UTF_8);
        }
    }

    /** URL のクエリ文字列を読む。 */
    public static Map<String, String> query(HttpExchange exchange) {
        String raw = exchange.getRequestURI().getRawQuery();
        return parse(raw == null ? "" : raw);
    }

    private static Map<String, String> parse(String source) {
        Map<String, String> values = new HashMap<>();
        for (String pair : source.split("&")) {
            int eq = pair.indexOf('=');
            if (eq > 0) {
                values.put(decode(pair.substring(0, eq)), decode(pair.substring(eq + 1)));
            }
        }
        return values;
    }

    private static String decode(String text) {
        return URLDecoder.decode(text, StandardCharsets.UTF_8);
    }

    /** 入力を数値へ。空や不正な値は null を返す。 */
    public static BigDecimal decimal(String source) {
        if (source == null || source.isBlank()) {
            return null;
        }
        try {
            return new BigDecimal(source.trim());
        } catch (NumberFormatException e) {
            return null;
        }
    }

    /** 入力を整数へ。空や不正な値は既定値を返す。 */
    public static int integer(String source, int fallback) {
        if (source == null || source.isBlank()) {
            return fallback;
        }
        try {
            return Integer.parseInt(source.trim());
        } catch (NumberFormatException e) {
            return fallback;
        }
    }

    /** 入力を日付へ。空や不正な値は null を返す。 */
    public static LocalDate date(String source) {
        if (source == null || source.isBlank()) {
            return null;
        }
        try {
            return LocalDate.parse(source.trim());
        } catch (RuntimeException e) {
            return null;
        }
    }

    /** 302 で別の画面へ送る。 */
    public static void redirect(HttpExchange exchange, String location)
            throws IOException {
        exchange.getResponseHeaders().add("Location", location);
        exchange.sendResponseHeaders(302, -1);
        exchange.close();
    }

    /** HTML を返す。 */
    public static void send(HttpExchange exchange, int status, String body)
            throws IOException {
        byte[] bytes = body.getBytes(StandardCharsets.UTF_8);
        exchange.getResponseHeaders().add("Content-Type", "text/html; charset=UTF-8");
        exchange.sendResponseHeaders(status, bytes.length);
        try (OutputStream out = exchange.getResponseBody()) {
            out.write(bytes);
        }
    }

    /** 画面の枠（head と body）を組む。 */
    public static String page(String title, String body) {
        return html("<!doctype html>",
                "<html lang='ja'>",
                "<head>",
                "<meta charset='UTF-8'>",
                "<meta name='viewport' content='width=device-width, initial-scale=1'>",
                "<title>" + escape(title) + "｜新販売管理システム</title>",
                "<style>",
                "body{font-family:'Segoe UI','Meiryo',sans-serif;margin:0;",
                "background:#f4f6f8;color:#1a1a1a;}",
                ".bar{display:flex;justify-content:space-between;align-items:center;",
                "background:#25476a;color:#fff;padding:.6rem 1rem;}",
                ".bar a{color:#cfe0f5;}",
                "h1{font-size:1.25rem;margin:1rem 0;}",
                "h2{font-size:1rem;margin:1.5rem 0 .5rem;color:#25476a;}",
                "main,.login{max-width:60rem;margin:1.5rem auto;padding:1.5rem;",
                "background:#fff;border:1px solid #d7dee6;border-radius:6px;}",
                ".login{max-width:24rem;}",
                "label{display:block;margin:.75rem 0;font-size:.9rem;}",
                "input,select{display:block;width:100%;box-sizing:border-box;",
                "padding:.5rem;margin-top:.25rem;border:1px solid #b9c4d0;",
                "border-radius:4px;}",
                ".row{display:flex;gap:1rem;flex-wrap:wrap;}",
                ".row label{flex:1 1 12rem;}",
                ".panes{display:flex;gap:1.5rem;flex-wrap:wrap;}",
                ".panes>section{flex:1 1 24rem;min-width:0;}",
                "button{margin-top:1rem;padding:.55rem 1.4rem;border:0;border-radius:4px;",
                "background:#25476a;color:#fff;font-size:1rem;cursor:pointer;}",
                "table{border-collapse:collapse;width:100%;margin:.5rem 0;}",
                "th,td{border:1px solid #d7dee6;padding:.4rem .6rem;text-align:left;",
                "font-size:.9rem;}",
                "th{background:#eef2f6;}",
                "td input[type=checkbox]{width:auto;display:inline;}",
                "td input{margin:0;padding:.25rem;}",
                ".num{text-align:right;}",
                ".msg{background:#fdecea;border:1px solid #f5c2c0;color:#9b1c1c;",
                "padding:.5rem .75rem;border-radius:4px;font-size:.9rem;}",
                ".ok{background:#eaf6ec;border:1px solid #b7dfc0;color:#1c6b2c;",
                "padding:.5rem .75rem;border-radius:4px;font-size:.9rem;}",
                ".note{color:#5b6976;font-size:.85rem;margin-top:1.5rem;}",
                ".ng{color:#9b1c1c;}",
                "</style>",
                "</head>",
                "<body>",
                body,
                "</body>",
                "</html>");
    }

    /** 見出しとメッセージだけの画面本体を組む。 */
    public static String main(String... lines) {
        return "<main>\n" + String.join("\n", lines) + "\n</main>\n";
    }

    /** 赤いメッセージ帯。 */
    public static String error(String message) {
        return message == null ? "" : "<p class='msg'>" + escape(message) + "</p>";
    }

    /** 緑のメッセージ帯。 */
    public static String done(String message) {
        return message == null ? "" : "<p class='ok'>" + escape(message) + "</p>";
    }

    /** メニューへ戻るリンク。 */
    public static String back() {
        return "<p class='note'><a href='/menu'>メニューへ戻る</a></p>";
    }
}
'''


# ── 受注管理の画面 ────────────────────────────────────────────────────────
_ORDER_SCREENS = r'''package jp.co.contoso.sps.demo;

import com.sun.net.httpserver.HttpExchange;

import java.io.IOException;
import java.math.BigDecimal;
import java.time.LocalDate;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;

import jp.co.contoso.sps.common.Customer;
import jp.co.contoso.sps.common.Product;
import jp.co.contoso.sps.common.Session;
import jp.co.contoso.sps.order.CancelResult;
import jp.co.contoso.sps.order.EdiRecord;
import jp.co.contoso.sps.order.ErrorInfo;
import jp.co.contoso.sps.order.ImportSummary;
import jp.co.contoso.sps.order.Order;
import jp.co.contoso.sps.order.OrderDetail;
import jp.co.contoso.sps.order.OrderDetailDto;
import jp.co.contoso.sps.order.OrderDto;
import jp.co.contoso.sps.order.OrderPage;
import jp.co.contoso.sps.order.OrderResult;
import jp.co.contoso.sps.order.OrderSearchCondition;
import jp.co.contoso.sps.order.OrderStatus;
import jp.co.contoso.sps.order.OrderSummary;
import jp.co.contoso.sps.order.ShipmentResult;

/**
 * 受注管理の画面（受注入力・受注一覧照会・受注取消・出荷指示・EDI受注取込結果照会）。
 *
 * 配布サンプルの足場で、設計書に対応する成果物ではない。業務ロジックは
 * `order` パッケージのサービスが持ち、ここは入出力の変換だけを行う。
 */
public class OrderScreens {

    /** 受注入力画面に出す明細の行数。 */
    private static final int INPUT_ROWS = 3;

    private final Services services;

    public OrderScreens(Services services) {
        this.services = services;
    }

    // ── 受注入力 ────────────────────────────────────────────────────────────

    /** 画面「受注入力」。得意先・商品・数量を入力して受注を登録する。 */
    public void input(HttpExchange exchange, Session session) throws IOException {
        String message = null;
        String result = null;
        if ("POST".equals(exchange.getRequestMethod())) {
            Map<String, String> form = Web.form(exchange);
            List<OrderDetailDto> details = new ArrayList<>();
            for (int i = 1; i <= INPUT_ROWS; i++) {
                String productCd = form.get("productCd" + i);
                BigDecimal qty = Web.decimal(form.get("orderQty" + i));
                if (productCd == null || productCd.isBlank() || qty == null) {
                    continue;
                }
                Product product = services.repos.productRepository().find(productCd);
                details.add(new OrderDetailDto(productCd, qty,
                        product == null ? "1" : product.getTaxType()));
            }
            OrderDto dto = new OrderDto(form.getOrDefault("customerCd", ""),
                    Web.date(form.get("orderDate")), Web.date(form.get("deliveryDate")),
                    details);
            dto.setEntryStaffCd(session.getStaffCd());
            OrderResult registered = services.regist.registerOrder(dto);
            if (registered.isOk()) {
                result = "受注番号" + registered.getOrderNo() + "を登録しました。"
                        + "（ステータス " + registered.getStatus().getCode()
                        + " " + registered.getStatus().getLabel() + "）";
            } else {
                message = messages(registered.getErrors());
            }
        }

        StringBuilder rows = new StringBuilder();
        for (int i = 1; i <= INPUT_ROWS; i++) {
            rows.append("<tr><td>").append(i).append("</td><td>")
                    .append(productSelect("productCd" + i))
                    .append("</td><td><input type='text' name='orderQty")
                    .append(i).append("' class='num'></td></tr>\n");
        }
        LocalDate today = LocalDate.now();
        Web.send(exchange, 200, Web.page("受注入力", WebMain.header(session) + Web.main(
                "<h1>受注入力</h1>",
                Web.error(message),
                Web.done(result),
                "<form method='post' action='/order/new'>",
                "<div class='row'>",
                "<label>得意先" + customerSelect("customerCd") + "</label>",
                "<label>受注日<input type='date' name='orderDate' value='"
                        + today + "'></label>",
                "<label>納品希望日<input type='date' name='deliveryDate' value='"
                        + services.calendar.nextBusinessDay(today) + "'></label>",
                "</div>",
                "<h2>明細</h2>",
                "<table><tr><th>No</th><th>商品</th><th>数量</th></tr>",
                rows.toString(),
                "</table>",
                "<button type='submit'>登録する</button>",
                "</form>",
                "<p class='note'>納品希望日は受注日以降の営業日のみ。取引停止の得意先は登録できません。</p>",
                Web.back())));
    }

    // ── 受注一覧照会 ────────────────────────────────────────────────────────

    /** 画面「受注一覧照会」。受注日・得意先・ステータスを条件に受注を検索する。 */
    public void list(HttpExchange exchange, Session session) throws IOException {
        Map<String, String> query = Web.query(exchange);
        OrderSearchCondition condition = new OrderSearchCondition();
        condition.setOrderDateFrom(Web.date(query.get("from")));
        condition.setOrderDateTo(Web.date(query.get("to")));
        condition.setCustomerCd(query.get("customerCd"));
        if (query.get("status") != null && !query.get("status").isBlank()) {
            condition.setStatus(OrderStatus.of(Web.integer(query.get("status"), 10)));
        }
        condition.setPage(Web.integer(query.get("page"), 1));
        condition.setPageSize(Web.integer(query.get("pageSize"), 20));
        OrderPage page = services.search.search(condition);

        StringBuilder rows = new StringBuilder();
        for (OrderSummary row : page.getRows()) {
            rows.append("<tr><td><a href='/order/detail?orderNo=")
                    .append(Web.escape(row.getOrderNo())).append("'>")
                    .append(Web.escape(row.getOrderNo())).append("</a></td><td>")
                    .append(row.getOrderDate()).append("</td><td>")
                    .append(Web.escape(row.getCustomerName())).append("</td>")
                    .append("<td class='num'>").append(row.getTotalAmount())
                    .append("</td><td>").append(Web.escape(row.getStatus().getLabel()))
                    .append("（").append(row.getStatus().getCode()).append("）</td><td>")
                    .append(Web.escape(row.getRoute() == null
                            ? "" : row.getRoute().getLabel()))
                    .append("</td></tr>\n");
        }
        Web.send(exchange, 200, Web.page("受注一覧照会", WebMain.header(session) + Web.main(
                "<h1>受注一覧照会</h1>",
                "<form method='get' action='/orders'>",
                "<div class='row'>",
                "<label>受注日（自）<input type='date' name='from' value='"
                        + Web.text(query.get("from")) + "'></label>",
                "<label>受注日（至）<input type='date' name='to' value='"
                        + Web.text(query.get("to")) + "'></label>",
                "<label>得意先" + customerSelect("customerCd") + "</label>",
                "<label>ステータス" + statusSelect(query.get("status")) + "</label>",
                "</div>",
                "<button type='submit'>検索する</button>",
                "</form>",
                "<p class='note'>" + page.getTotalCount() + " 件中 "
                        + page.getPage() + " / " + page.getTotalPages() + " ページ</p>",
                "<table>",
                "<tr><th>受注番号</th><th>受注日</th><th>得意先</th><th>受注金額</th>",
                "<th>ステータス</th><th>経路</th></tr>",
                rows.toString(),
                "</table>",
                paging(page),
                Web.back())));
    }

    /** 受注 1 件の明細（受注一覧照会からの遷移先）。 */
    public void detail(HttpExchange exchange, Session session) throws IOException {
        String orderNo = Web.query(exchange).getOrDefault("orderNo", "");
        Order order = services.repos.orderRepository().find(orderNo);
        if (order == null) {
            Web.send(exchange, 404, Web.page("受注明細", WebMain.header(session) + Web.main(
                    "<h1>受注明細</h1>", Web.error("受注が存在しません。"), Web.back())));
            return;
        }
        StringBuilder rows = new StringBuilder();
        for (OrderDetail detail : services.search.findDetails(orderNo)) {
            rows.append("<tr><td>").append(detail.getLineNo()).append("</td><td>")
                    .append(Web.escape(detail.getProductCd())).append("</td>")
                    .append("<td class='num'>").append(detail.getOrderQty())
                    .append("</td><td class='num'>").append(Web.text(detail.getUnitPrice()))
                    .append("</td><td class='num'>")
                    .append(Web.text(detail.getDetailAmount()))
                    .append("</td><td class='num'>").append(detail.getAllocatedQty())
                    .append("</td><td>").append(detail.getStatus().getCode())
                    .append("</td></tr>\n");
        }
        Web.send(exchange, 200, Web.page("受注明細", WebMain.header(session) + Web.main(
                "<h1>受注明細 " + Web.escape(orderNo) + "</h1>",
                "<p class='note'>得意先 " + Web.escape(order.getCustomerCd())
                        + " ／ 受注日 " + order.getOrderDate()
                        + " ／ 納品希望日 " + order.getDeliveryDate()
                        + " ／ ステータス " + Web.escape(order.getStatus().getLabel())
                        + "</p>",
                "<table>",
                "<tr><th>No</th><th>商品</th><th>数量</th><th>単価</th><th>明細金額</th>",
                "<th>引当済</th><th>明細状態</th></tr>",
                rows.toString(),
                "</table>",
                "<p class='note'><a href='/orders'>受注一覧照会へ戻る</a></p>",
                Web.back())));
    }

    // ── 受注取消 ────────────────────────────────────────────────────────────

    /** 画面「受注取消」。登録済みの受注を取り消し、引当を解放する。 */
    public void cancel(HttpExchange exchange, Session session) throws IOException {
        String message = null;
        String result = null;
        if ("POST".equals(exchange.getRequestMethod())) {
            Map<String, String> form = Web.form(exchange);
            String orderNo = form.getOrDefault("orderNo", "");
            Order order = services.repos.orderRepository().find(orderNo);
            if (order == null) {
                message = "受注番号を入力してください。";
            } else {
                CancelResult cancelled = services.cancel.cancelOrder(orderNo,
                        session.getStaffCd(), form.getOrDefault("reason", "01"),
                        form.get("note"), order.getUpdDatetime());
                if (cancelled.isOk()) {
                    result = "受注番号" + orderNo + "を取り消しました。";
                } else {
                    message = cancelled.getMessage();
                }
            }
        }

        StringBuilder rows = new StringBuilder();
        for (Order order : services.repos.allOrders()) {
            if (order.getStatus() == OrderStatus.CANCELED) {
                continue;
            }
            boolean cancelable = services.cancel.isCancelable(order.getStatus());
            rows.append("<tr><td>").append(Web.escape(order.getOrderNo()))
                    .append("</td><td>").append(Web.escape(order.getCustomerCd()))
                    .append("</td><td>").append(Web.escape(order.getStatus().getLabel()))
                    .append("</td><td>")
                    .append(cancelable ? "取消できる" : "<span class='ng'>取消できない</span>")
                    .append("</td></tr>\n");
        }
        Web.send(exchange, 200, Web.page("受注取消", WebMain.header(session) + Web.main(
                "<h1>受注取消</h1>",
                Web.error(message),
                Web.done(result),
                "<form method='post' action='/order/cancel'>",
                "<div class='row'>",
                "<label>受注番号<input type='text' name='orderNo' required></label>",
                "<label>取消理由<select name='reason'>",
                "<option value='01'>得意先都合</option>",
                "<option value='02'>在庫調達不可</option>",
                "<option value='99'>その他</option>",
                "</select></label>",
                "<label>取消理由備考<input type='text' name='note'></label>",
                "</div>",
                "<button type='submit'>取り消す</button>",
                "</form>",
                "<p class='note'>取消可否は受注ステータスで判定します"
                        + "（出荷指示済 40 以降は取り消せません）。"
                        + "取消理由が「その他」のときは備考が必須です。</p>",
                "<h2>受注の一覧</h2>",
                "<table><tr><th>受注番号</th><th>得意先</th><th>ステータス</th>",
                "<th>取消可否</th></tr>",
                rows.toString(),
                "</table>",
                Web.back())));
    }

    // ── 出荷指示 ────────────────────────────────────────────────────────────

    /** 画面「出荷指示」。引当済みの受注を選んで倉庫へ出荷指示を出す。 */
    public void shipment(HttpExchange exchange, Session session) throws IOException {
        StringBuilder outcome = new StringBuilder();
        if ("POST".equals(exchange.getRequestMethod())) {
            String body = Web.body(exchange);
            List<String> orderNos = Web.formAll(body, "orderNo");
            LocalDate shipDate = LocalDate.now();
            String warehouseCd = "0102";
            for (String pair : body.split("&")) {
                if (pair.startsWith("shipDate=")) {
                    LocalDate parsed = Web.date(pair.substring("shipDate=".length()));
                    if (parsed != null) {
                        shipDate = parsed;
                    }
                }
                if (pair.startsWith("warehouseCd=")) {
                    warehouseCd = pair.substring("warehouseCd=".length());
                }
            }
            if (orderNos.isEmpty()) {
                outcome.append(Web.error("出荷指示する受注を選んでください。"));
            } else {
                List<ShipmentResult> results = services.shipment.createInstruction(
                        orderNos, shipDate, warehouseCd);
                for (ShipmentResult result : results) {
                    if (result.isOk()) {
                        outcome.append(Web.done("受注 " + result.getOrderNo()
                                + " → 出荷指示 " + result.getShipmentNo()));
                    } else if (!result.getShortages().isEmpty()) {
                        StringBuilder shortage = new StringBuilder();
                        for (int i = 0; i < result.getShortages().size(); i++) {
                            shortage.append(i == 0 ? "" : " / ")
                                    .append(result.getShortages().get(i).getProductCd())
                                    .append(" 不足 ")
                                    .append(result.getShortages().get(i).getShortQty());
                        }
                        outcome.append(Web.error("受注 " + result.getOrderNo()
                                + " は在庫が足りません（" + shortage + "）"));
                    } else {
                        outcome.append(Web.error("受注 " + result.getOrderNo()
                                + " は " + result.getMessage()));
                    }
                }
            }
        }

        StringBuilder rows = new StringBuilder();
        for (Order order : services.repos.allOrders()) {
            if (!services.shipment.isInstructable(order.getStatus())) {
                continue;
            }
            rows.append("<tr><td><input type='checkbox' name='orderNo' value='")
                    .append(Web.escape(order.getOrderNo())).append("'></td><td>")
                    .append(Web.escape(order.getOrderNo())).append("</td><td>")
                    .append(Web.escape(order.getCustomerCd())).append("</td><td>")
                    .append(order.getDeliveryDate()).append("</td><td>")
                    .append(Web.escape(order.getStatus().getLabel()))
                    .append("</td></tr>\n");
        }
        Web.send(exchange, 200, Web.page("出荷指示", WebMain.header(session) + Web.main(
                "<h1>出荷指示</h1>",
                outcome.toString(),
                "<form method='post' action='/shipment'>",
                "<div class='row'>",
                "<label>出荷日<input type='date' name='shipDate' value='"
                        + LocalDate.now() + "'></label>",
                "<label>倉庫" + WebMain.warehouseSelect(services, "warehouseCd")
                        + "</label>",
                "</div>",
                "<table><tr><th>選択</th><th>受注番号</th><th>得意先</th>",
                "<th>納品希望日</th><th>ステータス</th></tr>",
                rows.toString(),
                "</table>",
                "<button type='submit'>出荷指示を出す</button>",
                "</form>",
                "<p class='note'>ここで在庫の引当が走ります（受注登録の時点では引き当てません）。"
                        + "与信保留の受注は一覧に出ません。</p>",
                Web.back())));
    }

    // ── EDI受注取込結果照会 ─────────────────────────────────────────────────

    /** 画面「EDI受注取込結果照会」。取り込んだ受注とエラー内容を確認する。 */
    public void edi(HttpExchange exchange, Session session) throws IOException {
        String result = null;
        LocalDate targetDate = LocalDate.of(2026, 1, 15);
        if ("POST".equals(exchange.getRequestMethod())) {
            Map<String, String> form = Web.form(exchange);
            LocalDate parsed = Web.date(form.get("targetDate"));
            if (parsed != null) {
                targetDate = parsed;
            }
            ImportSummary summary = services.ediImport.importOrders(targetDate);
            result = "取込を実行しました。成功 " + summary.getSuccessCount()
                    + " 件 / エラー " + summary.getErrorCount() + " 件";
        } else {
            LocalDate parsed = Web.date(Web.query(exchange).get("targetDate"));
            if (parsed != null) {
                targetDate = parsed;
            }
        }

        StringBuilder rows = new StringBuilder();
        for (EdiRecord record : services.ediImport.findResults(targetDate)) {
            rows.append("<tr><td>").append(Web.escape(record.getRecvNo()))
                    .append("</td><td>").append(Web.escape(record.getEdiCustomerCd()))
                    .append("</td><td>").append(Web.escape(record.getJanCd()))
                    .append("</td><td class='num'>").append(record.getOrderQty())
                    .append("</td><td>").append(Web.escape(record.getStatus()))
                    .append("</td><td>").append(Web.escape(record.getOrderNo()))
                    .append("</td><td class='ng'>")
                    .append(Web.escape(record.getErrorMessage()))
                    .append("</td></tr>\n");
        }
        Web.send(exchange, 200, Web.page("EDI受注取込結果照会",
                WebMain.header(session) + Web.main(
                "<h1>EDI受注取込結果照会</h1>",
                Web.done(result),
                "<form method='post' action='/edi'>",
                "<label>対象日<input type='date' name='targetDate' value='"
                        + targetDate + "'></label>",
                "<button type='submit'>取込を実行する</button>",
                "</form>",
                "<table>",
                "<tr><th>受信番号</th><th>得意先(EDI)</th><th>JANコード</th><th>数量</th>",
                "<th>状態</th><th>受注番号</th><th>エラー内容</th></tr>",
                rows.toString(),
                "</table>",
                "<p class='note'>得意先コードは 6 桁で届くので 8 桁へ読み替え、"
                        + "商品は JAN コードから商品マスタを引きます。</p>",
                Web.back())));
    }

    // ── 部品 ────────────────────────────────────────────────────────────────

    private String customerSelect(String name) {
        StringBuilder options = new StringBuilder("<select name='" + name + "'>");
        options.append("<option value=''>（指定しない）</option>");
        for (Customer customer : services.master.listCustomers()) {
            options.append("<option value='").append(Web.escape(customer.getCustomerCd()))
                    .append("'>").append(Web.escape(customer.getCustomerCd()))
                    .append(" ").append(Web.escape(customer.getName()))
                    .append("</option>");
        }
        return options.append("</select>").toString();
    }

    private String productSelect(String name) {
        StringBuilder options = new StringBuilder("<select name='" + name + "'>");
        options.append("<option value=''>（選択しない）</option>");
        for (Product product : services.master.listProducts()) {
            options.append("<option value='").append(Web.escape(product.getProductCd()))
                    .append("'>").append(Web.escape(product.getProductCd()))
                    .append(" ").append(Web.escape(product.getProductName()))
                    .append("</option>");
        }
        return options.append("</select>").toString();
    }

    private static String statusSelect(String selected) {
        StringBuilder options = new StringBuilder("<select name='status'>");
        options.append("<option value=''>（指定しない）</option>");
        for (OrderStatus status : OrderStatus.values()) {
            String value = String.valueOf(status.getCode());
            options.append("<option value='").append(value).append("'")
                    .append(value.equals(selected) ? " selected" : "").append(">")
                    .append(Web.escape(status.getLabel())).append("（").append(value)
                    .append("）</option>");
        }
        return options.append("</select>").toString();
    }

    private static String paging(OrderPage page) {
        StringBuilder links = new StringBuilder("<p class='note'>");
        if (page.getPage() > 1) {
            links.append("<a href='/orders?page=").append(page.getPage() - 1)
                    .append("'>前のページ</a> ");
        }
        if (!page.isLast()) {
            links.append("<a href='/orders?page=").append(page.getPage() + 1)
                    .append("'>次のページ</a>");
        }
        return links.append("</p>").toString();
    }

    private static String messages(List<ErrorInfo> errors) {
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


# ── 在庫管理の画面 ────────────────────────────────────────────────────────
_INVENTORY_SCREENS = r'''package jp.co.contoso.sps.demo;

import com.sun.net.httpserver.HttpExchange;

import java.io.IOException;
import java.math.BigDecimal;
import java.time.LocalDate;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;

import jp.co.contoso.sps.common.BusinessException;
import jp.co.contoso.sps.common.Product;
import jp.co.contoso.sps.common.Session;
import jp.co.contoso.sps.inventory.CountLine;
import jp.co.contoso.sps.inventory.CountResult;
import jp.co.contoso.sps.inventory.MoveType;
import jp.co.contoso.sps.inventory.Stock;
import jp.co.contoso.sps.inventory.StockHistory;
import jp.co.contoso.sps.inventory.StockMoveDto;
import jp.co.contoso.sps.inventory.StockView;

/**
 * 在庫管理の画面（在庫照会・入庫登録・棚卸入力・在庫調整）。
 *
 * 配布サンプルの足場で、設計書に対応する成果物ではない。
 */
public class InventoryScreens {

    private final Services services;

    public InventoryScreens(Services services) {
        this.services = services;
    }

    /** 画面「在庫照会」。倉庫・商品ごとの実在庫数と有効在庫数を照会する。 */
    public void stock(HttpExchange exchange, Session session) throws IOException {
        Map<String, String> query = Web.query(exchange);
        String warehouseCd = query.getOrDefault("warehouseCd", "0102");
        String productCd = query.getOrDefault("productCd", "");

        StringBuilder rows = new StringBuilder();
        for (StockView view : services.stockInquiry.search(warehouseCd, productCd)) {
            rows.append("<tr><td>").append(Web.escape(view.getProductCd()))
                    .append("</td><td>").append(Web.escape(view.getProductName()))
                    .append("</td><td class='num'>").append(view.getStockQty())
                    .append("</td><td class='num'>").append(view.getAllocatedQty())
                    .append("</td><td class='num'>").append(view.getAvailableQty())
                    .append("</td><td><a href='/stock?warehouseCd=")
                    .append(Web.escape(warehouseCd)).append("&productCd=")
                    .append(Web.escape(view.getProductCd())).append("'>ロット</a>")
                    .append("</td></tr>\n");
        }

        StringBuilder lots = new StringBuilder();
        if (!productCd.isBlank()) {
            lots.append("<h2>ロット別の在庫（入庫日の古い順に引き当てます）</h2>")
                    .append("<table><tr><th>ロット</th><th>入庫日</th><th>実在庫</th>")
                    .append("<th>引当済</th><th>有効在庫</th></tr>");
            for (Stock lot : services.stockInquiry.findLots(warehouseCd, productCd)) {
                lots.append("<tr><td>").append(Web.escape(lot.getLotNo()))
                        .append("</td><td>").append(lot.getReceiveDate())
                        .append("</td><td class='num'>").append(lot.getStockQty())
                        .append("</td><td class='num'>").append(lot.getAllocatedQty())
                        .append("</td><td class='num'>").append(lot.availableQty())
                        .append("</td></tr>\n");
            }
            lots.append("</table>");
        }

        Web.send(exchange, 200, Web.page("在庫照会", WebMain.header(session) + Web.main(
                "<h1>在庫照会</h1>",
                "<form method='get' action='/stock'>",
                "<div class='row'>",
                "<label>倉庫" + WebMain.warehouseSelect(services, "warehouseCd")
                        + "</label>",
                "<label>商品コード<input type='text' name='productCd' value='"
                        + Web.escape(productCd) + "'></label>",
                "</div>",
                "<button type='submit'>照会する</button>",
                "</form>",
                "<table><tr><th>商品コード</th><th>商品名</th><th>実在庫数</th>",
                "<th>引当済数量</th><th>有効在庫数</th><th></th></tr>",
                rows.toString(),
                "</table>",
                lots.toString(),
                Web.back())));
    }

    /** 画面「入庫登録」。仕入・返品による入庫を登録して在庫を増やす。 */
    public void receive(HttpExchange exchange, Session session) throws IOException {
        String message = null;
        String result = null;
        if ("POST".equals(exchange.getRequestMethod())) {
            Map<String, String> form = Web.form(exchange);
            BigDecimal qty = Web.decimal(form.get("qty"));
            if (qty == null) {
                message = "数量を入力してください。";
            } else {
                try {
                    services.stockUpdate.receive(new StockMoveDto(
                            form.getOrDefault("warehouseCd", "0102"),
                            form.getOrDefault("productCd", ""),
                            form.getOrDefault("lotNo", ""),
                            MoveType.of(form.getOrDefault("moveType", "11")),
                            qty, dateOrToday(form.get("moveDate")),
                            form.get("reason"), session.getStaffCd()));
                    result = "入庫を登録しました。";
                } catch (BusinessException e) {
                    message = e.getMessage();
                }
            }
        }
        Web.send(exchange, 200, Web.page("入庫登録", WebMain.header(session) + Web.main(
                "<h1>入庫登録</h1>",
                Web.error(message),
                Web.done(result),
                "<form method='post' action='/stock/receive'>",
                "<div class='row'>",
                "<label>倉庫" + WebMain.warehouseSelect(services, "warehouseCd")
                        + "</label>",
                "<label>商品" + productSelect("productCd") + "</label>",
                "<label>ロット番号<input type='text' name='lotNo' required></label>",
                "</div>",
                "<div class='row'>",
                "<label>区分<select name='moveType'>",
                "<option value='11'>仕入入庫</option>",
                "<option value='12'>返品入庫</option>",
                "</select></label>",
                "<label>数量<input type='text' name='qty' class='num' required></label>",
                "<label>入庫日<input type='date' name='moveDate' value='"
                        + LocalDate.now() + "'></label>",
                "<label>摘要<input type='text' name='reason'></label>",
                "</div>",
                "<button type='submit'>登録する</button>",
                "</form>",
                "<p class='note'>未登録のロットを指定すると在庫の行を新しく起こします。</p>",
                history(),
                Web.back())));
    }

    /** 画面「棚卸入力」。実地棚卸の結果を入力して帳簿在庫との差異を確定する。 */
    public void count(HttpExchange exchange, Session session) throws IOException {
        String message = null;
        String result = null;
        String warehouseCd = "0102";
        if ("POST".equals(exchange.getRequestMethod())) {
            String body = Web.body(exchange);
            Map<String, String> form = new java.util.HashMap<>();
            for (String pair : body.split("&")) {
                int eq = pair.indexOf('=');
                if (eq > 0) {
                    form.put(pair.substring(0, eq), pair.substring(eq + 1));
                }
            }
            warehouseCd = form.getOrDefault("warehouseCd", "0102");
            List<String> lotKeys = Web.formAll(body, "lotKey");
            List<String> actuals = Web.formAll(body, "actualQty");

            List<CountLine> lines = new ArrayList<>();
            String countNo = services.numbering.next("COUNT");
            for (int i = 0; i < lotKeys.size() && i < actuals.size(); i++) {
                BigDecimal actual = Web.decimal(actuals.get(i));
                if (actual == null) {
                    continue;
                }
                String[] key = lotKeys.get(i).split(":");
                Stock lot = services.repos.stockRepository()
                        .find(key[0], key[1], key[2]);
                if (lot == null) {
                    continue;
                }
                lines.add(new CountLine(countNo, key[0], key[1], key[2],
                        lot.getStockQty(), actual));
            }
            if (lines.isEmpty()) {
                message = "実地数量を 1 件以上入力してください。";
            } else {
                try {
                    services.count.saveCount(lines);
                    CountResult confirmed = services.count.confirmCount(countNo,
                            LocalDate.now(), session.getStaffCd());
                    result = "棚卸番号" + confirmed.getCountNo() + "を確定しました。"
                            + "差異のあった " + confirmed.getAdjustedLines()
                            + " 行を在庫調整として計上（差異合計 "
                            + confirmed.getTotalDiffQty() + "）";
                } catch (BusinessException e) {
                    message = e.getMessage();
                }
            }
        } else {
            warehouseCd = Web.query(exchange).getOrDefault("warehouseCd", "0102");
        }

        StringBuilder rows = new StringBuilder();
        for (Stock lot : services.repos.stockRepository().findByWarehouse(warehouseCd)) {
            String key = lot.getWarehouseCd() + ":" + lot.getProductCd() + ":"
                    + lot.getLotNo();
            rows.append("<tr><td>").append(Web.escape(lot.getProductCd()))
                    .append("</td><td>").append(Web.escape(lot.getLotNo()))
                    .append("</td><td class='num'>").append(lot.getStockQty())
                    .append("</td><td><input type='hidden' name='lotKey' value='")
                    .append(Web.escape(key))
                    .append("'><input type='text' name='actualQty' class='num'></td>")
                    .append("</tr>\n");
        }
        Web.send(exchange, 200, Web.page("棚卸入力", WebMain.header(session) + Web.main(
                "<h1>棚卸入力</h1>",
                Web.error(message),
                Web.done(result),
                "<form method='post' action='/stock/count'>",
                "<label>倉庫" + WebMain.warehouseSelect(services, "warehouseCd")
                        + "</label>",
                "<table><tr><th>商品コード</th><th>ロット</th><th>帳簿在庫</th>",
                "<th>実地数量</th></tr>",
                rows.toString(),
                "</table>",
                "<button type='submit'>確定する</button>",
                "</form>",
                "<p class='note'>入力した行だけを対象にします。"
                        + "帳簿在庫と一致する行は在庫を動かしません。</p>",
                Web.back())));
    }

    /** 画面「在庫調整」。破損・廃棄などの理由を付けて在庫数を補正する。 */
    public void adjust(HttpExchange exchange, Session session) throws IOException {
        String message = null;
        String result = null;
        if ("POST".equals(exchange.getRequestMethod())) {
            Map<String, String> form = Web.form(exchange);
            BigDecimal qty = Web.decimal(form.get("qty"));
            if (qty == null) {
                message = "数量を入力してください。";
            } else {
                try {
                    services.stockUpdate.adjust(new StockMoveDto(
                            form.getOrDefault("warehouseCd", "0102"),
                            form.getOrDefault("productCd", ""),
                            form.getOrDefault("lotNo", ""),
                            MoveType.of(form.getOrDefault("moveType", "32")),
                            qty, dateOrToday(form.get("moveDate")),
                            form.get("reason"), session.getStaffCd()));
                    result = "在庫を調整しました。";
                } catch (BusinessException e) {
                    message = e.getMessage();
                }
            }
        }
        Web.send(exchange, 200, Web.page("在庫調整", WebMain.header(session) + Web.main(
                "<h1>在庫調整</h1>",
                Web.error(message),
                Web.done(result),
                "<form method='post' action='/stock/adjust'>",
                "<div class='row'>",
                "<label>倉庫" + WebMain.warehouseSelect(services, "warehouseCd")
                        + "</label>",
                "<label>商品" + productSelect("productCd") + "</label>",
                "<label>ロット番号<input type='text' name='lotNo' required></label>",
                "</div>",
                "<div class='row'>",
                "<label>区分<select name='moveType'>",
                "<option value='32'>破損廃棄（減）</option>",
                "<option value='31'>棚卸調整（増）</option>",
                "</select></label>",
                "<label>数量<input type='text' name='qty' class='num' required></label>",
                "<label>調整日<input type='date' name='moveDate' value='"
                        + LocalDate.now() + "'></label>",
                "<label>理由<input type='text' name='reason' required></label>",
                "</div>",
                "<button type='submit'>調整する</button>",
                "</form>",
                "<p class='note'>調整の結果、実在庫数が負になる更新はできません"
                        + "（業務ルール「在庫マイナスの禁止」）。</p>",
                history(),
                Web.back())));
    }

    // ── 部品 ────────────────────────────────────────────────────────────────

    private static LocalDate dateOrToday(String source) {
        LocalDate parsed = Web.date(source);
        return parsed == null ? LocalDate.now() : parsed;
    }

    private String productSelect(String name) {
        StringBuilder options = new StringBuilder("<select name='" + name + "'>");
        for (Product product : services.master.listProducts()) {
            options.append("<option value='").append(Web.escape(product.getProductCd()))
                    .append("'>").append(Web.escape(product.getProductCd()))
                    .append(" ").append(Web.escape(product.getProductName()))
                    .append("</option>");
        }
        return options.append("</select>").toString();
    }

    private String history() {
        StringBuilder rows = new StringBuilder();
        List<StockHistory> histories = services.repos.stockHistories();
        for (int i = Math.max(0, histories.size() - 10); i < histories.size(); i++) {
            StockHistory history = histories.get(i);
            rows.append("<tr><td>").append(history.getMoveDate())
                    .append("</td><td>").append(Web.escape(history.getProductCd()))
                    .append("</td><td>").append(Web.escape(history.getLotNo()))
                    .append("</td><td>")
                    .append(Web.escape(history.getMoveType().getLabel()))
                    .append("</td><td class='num'>").append(history.getBeforeQty())
                    .append("</td><td class='num'>").append(history.getAfterQty())
                    .append("</td></tr>\n");
        }
        return Web.html("<h2>在庫移動履歴（直近 10 件）</h2>",
                "<table><tr><th>移動日</th><th>商品</th><th>ロット</th><th>区分</th>",
                "<th>変更前</th><th>変更後</th></tr>",
                rows.toString(),
                "</table>");
    }
}
'''


# ── 請求管理の画面 ────────────────────────────────────────────────────────
_BILLING_SCREENS = r'''package jp.co.contoso.sps.demo;

import com.sun.net.httpserver.HttpExchange;

import java.io.IOException;
import java.math.BigDecimal;
import java.time.LocalDate;
import java.util.List;
import java.util.Map;

import jp.co.contoso.sps.billing.BatchResult;
import jp.co.contoso.sps.billing.Deposit;
import jp.co.contoso.sps.billing.DepositCandidate;
import jp.co.contoso.sps.billing.Invoice;
import jp.co.contoso.sps.billing.InvoicePrintData;
import jp.co.contoso.sps.billing.InvoicePrintLine;
import jp.co.contoso.sps.billing.MatchSummary;
import jp.co.contoso.sps.billing.Receivable;
import jp.co.contoso.sps.common.BusinessException;
import jp.co.contoso.sps.common.ClosingType;
import jp.co.contoso.sps.common.Session;

/**
 * 請求管理の画面（請求締め処理・請求書発行・入金消込・売掛残高照会）。
 *
 * 配布サンプルの足場で、設計書に対応する成果物ではない。
 */
public class BillingScreens {

    private final Services services;

    public BillingScreens(Services services) {
        this.services = services;
    }

    /** 画面「請求締め処理」。締め対象の得意先を指定して請求を確定する。 */
    public void close(HttpExchange exchange, Session session) throws IOException {
        String result = null;
        String message = null;
        if ("POST".equals(exchange.getRequestMethod())) {
            Map<String, String> form = Web.form(exchange);
            LocalDate businessDate = Web.date(form.get("businessDate"));
            String closingYm = form.getOrDefault("closingYm", "");
            if (businessDate == null || closingYm.length() != 6) {
                message = "請求年月（YYYYMM）と業務日付を入力してください。";
            } else {
                String customerCd = form.getOrDefault("customerCd", "");
                BatchResult batch = services.billing.execute(closingYm,
                        customerCd.isBlank() ? List.of() : List.of(customerCd),
                        businessDate);
                result = "確定 " + batch.getClosedCount() + " 件 / 対象外 "
                        + batch.getSkippedCount() + " 件 / 異常 "
                        + batch.getFailedCount() + " 件";
            }
        }

        StringBuilder rows = new StringBuilder();
        for (Invoice invoice : services.repos.invoiceRepository().findAll()) {
            rows.append("<tr><td>").append(Web.escape(invoice.getInvoiceNo()))
                    .append("</td><td>").append(Web.escape(invoice.getClosingYm()))
                    .append("</td><td>").append(Web.escape(invoice.getCustomerCd()))
                    .append("</td><td class='num'>").append(invoice.getSalesAmount())
                    .append("</td><td class='num'>").append(invoice.getTaxAmount())
                    .append("</td><td class='num'>").append(invoice.getBillingAmount())
                    .append("</td><td>").append(Web.escape(invoice.getStatus().getLabel()))
                    .append("</td></tr>\n");
        }
        Web.send(exchange, 200, Web.page("請求締め処理",
                WebMain.header(session) + Web.main(
                "<h1>請求締め処理</h1>",
                Web.error(message),
                Web.done(result),
                "<form method='post' action='/billing/close'>",
                "<div class='row'>",
                "<label>請求年月（YYYYMM）<input type='text' name='closingYm'"
                        + " value='202601'></label>",
                "<label>業務日付<input type='date' name='businessDate'"
                        + " value='2026-01-20'></label>",
                "<label>得意先コード（空なら全件）<input type='text'"
                        + " name='customerCd'></label>",
                "</div>",
                "<button type='submit'>締める</button>",
                "</form>",
                "<p class='note'>締め日は締め日マスタから引きます"
                        + "（20日締め = "
                        + services.billing.closingDay(ClosingType.TWENTIETH)
                        + " 日、末日締めは月の末日）。休日にあたる場合は前営業日に締めます。</p>",
                "<h2>確定済みの請求</h2>",
                "<table><tr><th>請求番号</th><th>請求年月</th><th>得意先</th>",
                "<th>売上額</th><th>消費税</th><th>請求金額</th><th>状態</th></tr>",
                rows.toString(),
                "</table>",
                Web.back())));
    }

    /** 画面「請求書発行」。確定した請求から請求書を出力する。 */
    public void invoice(HttpExchange exchange, Session session) throws IOException {
        String message = null;
        StringBuilder printed = new StringBuilder();
        if ("POST".equals(exchange.getRequestMethod())) {
            Map<String, String> form = Web.form(exchange);
            try {
                InvoicePrintData data = services.invoicePrint.print(
                        form.getOrDefault("invoiceNo", ""), LocalDate.now(),
                        session.getStaffCd());
                printed.append(Web.done("請求書を出力しました（帳票基盤へ渡す出力データ）"));
                printed.append(printPreview(data));
            } catch (BusinessException e) {
                message = e.getMessage();
            }
        }

        StringBuilder rows = new StringBuilder();
        for (Invoice invoice : services.repos.invoiceRepository().findAll()) {
            rows.append("<tr><td>").append(Web.escape(invoice.getInvoiceNo()))
                    .append("</td><td>").append(Web.escape(invoice.getCustomerCd()))
                    .append("</td><td class='num'>").append(invoice.getBillingAmount())
                    .append("</td><td>").append(Web.escape(invoice.getStatus().getLabel()))
                    .append("</td></tr>\n");
        }
        Web.send(exchange, 200, Web.page("請求書発行",
                WebMain.header(session) + Web.main(
                "<h1>請求書発行</h1>",
                Web.error(message),
                printed.toString(),
                "<form method='post' action='/billing/invoice'>",
                "<label>請求番号<input type='text' name='invoiceNo' required></label>",
                "<button type='submit'>発行する</button>",
                "</form>",
                "<p class='note'>締め済（10）の請求だけを発行できます。"
                        + "PDF の組版は既存の帳票基盤（SVF）が行うため、"
                        + "ここでは帳票基盤へ渡す出力データを表示します。</p>",
                "<table><tr><th>請求番号</th><th>得意先</th><th>請求金額</th>",
                "<th>状態</th></tr>",
                rows.toString(),
                "</table>",
                Web.back())));
    }

    /** 画面「入金消込」。入金データと請求を突き合わせて消し込む。 */
    public void deposit(HttpExchange exchange, Session session) throws IOException {
        String message = null;
        String result = null;
        LocalDate depositDate = LocalDate.now();
        MatchSummary summary = null;

        if ("POST".equals(exchange.getRequestMethod())) {
            Map<String, String> form = Web.form(exchange);
            LocalDate parsed = Web.date(form.get("depositDate"));
            if (parsed != null) {
                depositDate = parsed;
            }
            if ("apply".equals(form.get("action"))) {
                BigDecimal amount = Web.decimal(form.get("amount"));
                try {
                    services.deposit.applyDeposit(form.getOrDefault("depositNo", ""),
                            form.getOrDefault("invoiceNo", ""), amount,
                            session.getStaffCd());
                    result = "入金を充当しました。";
                } catch (BusinessException e) {
                    message = e.getMessage();
                }
            } else {
                summary = services.deposit.matchDeposits(depositDate);
                result = "自動消込 " + summary.getMatchedCount() + " 件 / 候補 "
                        + summary.getUnmatchedCount() + " 件";
            }
        } else {
            LocalDate parsed = Web.date(Web.query(exchange).get("depositDate"));
            if (parsed != null) {
                depositDate = parsed;
            }
        }

        StringBuilder deposits = new StringBuilder();
        for (Deposit item : services.repos.depositRepository().findByDate(depositDate)) {
            deposits.append("<tr><td>").append(Web.escape(item.getDepositNo()))
                    .append("</td><td>").append(Web.escape(item.getPayerName()))
                    .append("</td><td class='num'>").append(item.getDepositAmount())
                    .append("</td><td class='num'>").append(item.getAppliedAmount())
                    .append("</td><td class='num'>").append(item.getRemainingAmount())
                    .append("</td></tr>\n");
        }

        StringBuilder candidates = new StringBuilder();
        if (summary != null) {
            for (DepositCandidate candidate : summary.getCandidates()) {
                candidates.append("<tr><td>")
                        .append(Web.escape(candidate.getDepositNo()))
                        .append("</td><td>").append(Web.escape(candidate.getInvoiceNo()))
                        .append("</td><td class='num'>")
                        .append(candidate.getUnpaidAmount())
                        .append("</td><td>").append(Web.escape(candidate.getReason()))
                        .append("</td></tr>\n");
            }
        }

        Web.send(exchange, 200, Web.page("入金消込", WebMain.header(session) + Web.main(
                "<h1>入金消込</h1>",
                Web.error(message),
                Web.done(result),
                "<form method='post' action='/billing/deposit'>",
                "<label>入金日<input type='date' name='depositDate' value='"
                        + depositDate + "'></label>",
                "<button type='submit'>突合する</button>",
                "</form>",
                "<div class='panes'>",
                "<section><h2>入金一覧</h2>",
                "<table><tr><th>入金番号</th><th>振込人名</th><th>入金額</th>",
                "<th>充当済</th><th>残額</th></tr>",
                deposits.toString(),
                "</table></section>",
                "<section><h2>消込候補</h2>",
                "<table><tr><th>入金番号</th><th>請求番号</th><th>未回収</th>",
                "<th>理由</th></tr>",
                candidates.toString(),
                "</table></section>",
                "</div>",
                "<h2>手動で充当する</h2>",
                "<form method='post' action='/billing/deposit'>",
                "<input type='hidden' name='action' value='apply'>",
                "<input type='hidden' name='depositDate' value='" + depositDate + "'>",
                "<div class='row'>",
                "<label>入金番号<input type='text' name='depositNo' required></label>",
                "<label>請求番号<input type='text' name='invoiceNo' required></label>",
                "<label>充当額<input type='text' name='amount' class='num' required>"
                        + "</label>",
                "</div>",
                "<button type='submit'>充当する</button>",
                "</form>",
                "<p class='note'>請求番号が一致する入金は自動で消し込みます。"
                        + "一致しない入金は「得意先 + 金額」で候補を出し、"
                        + "選ぶのは担当者に委ねます。</p>",
                Web.back())));
    }

    /** 画面「売掛残高照会」。得意先ごとの売掛残高と回収予定を照会する。 */
    public void receivable(HttpExchange exchange, Session session) throws IOException {
        Map<String, String> query = Web.query(exchange);
        LocalDate baseDate = Web.date(query.get("baseDate"));
        if (baseDate == null) {
            baseDate = LocalDate.now();
        }
        String customerCd = query.getOrDefault("customerCd", "");

        StringBuilder rows = new StringBuilder();
        for (Receivable item : services.receivable.search(customerCd, baseDate)) {
            rows.append("<tr><td>").append(Web.escape(item.getCustomerCd()))
                    .append("</td><td>").append(Web.escape(item.getCustomerName()))
                    .append("</td><td class='num'>").append(item.getBillingAmount())
                    .append("</td><td class='num'>").append(item.getDepositAmount())
                    .append("</td><td class='num'>").append(item.getUnpaidAmount())
                    .append("</td><td class='num'>").append(item.getOverdueDays())
                    .append("</td><td>")
                    .append(item.isOverdue() ? "<span class='ng'>滞留</span>" : "")
                    .append("</td></tr>\n");
        }
        Web.send(exchange, 200, Web.page("売掛残高照会",
                WebMain.header(session) + Web.main(
                "<h1>売掛残高照会</h1>",
                "<form method='get' action='/billing/receivable'>",
                "<div class='row'>",
                "<label>基準日<input type='date' name='baseDate' value='"
                        + baseDate + "'></label>",
                "<label>得意先コード<input type='text' name='customerCd' value='"
                        + Web.escape(customerCd) + "'></label>",
                "</div>",
                "<button type='submit'>照会する</button>",
                "</form>",
                "<table><tr><th>得意先</th><th>得意先名</th><th>請求金額</th>",
                "<th>入金額</th><th>未回収残高</th><th>滞留日数</th><th></th></tr>",
                rows.toString(),
                "</table>",
                "<p class='note'>支払期日は締め月の翌月末とみなします。"
                        + "消込完了の請求は残高に含めません。</p>",
                Web.back())));
    }

    private static String printPreview(InvoicePrintData data) {
        StringBuilder rows = new StringBuilder();
        for (InvoicePrintLine line : data.getLines()) {
            rows.append("<tr><td>").append(line.getSalesDate())
                    .append("</td><td>").append(Web.escape(line.getProductCd()))
                    .append("</td><td class='num'>").append(line.getNetAmount())
                    .append("</td><td>").append(Web.escape(line.getTaxType()))
                    .append("</td></tr>\n");
        }
        return Web.html("<h2>請求書（" + Web.escape(data.getInvoiceNo()) + "）</h2>",
                "<p class='note'>" + Web.escape(data.getCustomerName())
                        + " 御中 ／ 発行日 " + data.getIssueDate()
                        + " ／ 前回請求残高 " + data.getPrevBalance()
                        + " ／ 当月売上 " + data.getSalesAmount()
                        + " ／ 消費税 " + data.getTaxAmount()
                        + " ／ 請求金額 " + data.getBillingAmount() + "</p>",
                "<table><tr><th>売上日</th><th>商品</th><th>金額</th>",
                "<th>税区分</th></tr>",
                rows.toString(),
                "</table>");
    }
}
'''


# ── 共通基盤の画面（マスタ保守）───────────────────────────────────────────
_MASTER_SCREENS = r'''package jp.co.contoso.sps.demo;

import com.sun.net.httpserver.HttpExchange;

import java.io.IOException;
import java.math.BigDecimal;
import java.util.List;
import java.util.Map;

import jp.co.contoso.sps.common.ClosingType;
import jp.co.contoso.sps.common.Customer;
import jp.co.contoso.sps.common.Product;
import jp.co.contoso.sps.common.Session;
import jp.co.contoso.sps.order.ErrorInfo;

/**
 * 共通基盤の画面（得意先マスタ保守・商品マスタ保守）。
 *
 * 配布サンプルの足場で、設計書に対応する成果物ではない。
 */
public class MasterScreens {

    private final Services services;

    public MasterScreens(Services services) {
        this.services = services;
    }

    /** 画面「得意先マスタ保守」。得意先の登録・変更・論理削除を行う。 */
    public void customer(HttpExchange exchange, Session session) throws IOException {
        String message = null;
        String result = null;
        if ("POST".equals(exchange.getRequestMethod())) {
            Map<String, String> form = Web.form(exchange);
            String customerCd = form.getOrDefault("customerCd", "");
            if ("delete".equals(form.get("action"))) {
                services.master.deleteCustomer(customerCd, session.getStaffCd());
                result = "得意先" + customerCd + "を削除しました。";
            } else {
                BigDecimal creditLimit = Web.decimal(form.get("creditLimit"));
                Customer customer = new Customer(customerCd,
                        form.getOrDefault("customerName", ""),
                        creditLimit == null ? BigDecimal.ZERO : creditLimit,
                        ClosingType.of(form.getOrDefault("closingType", "1")),
                        "1".equals(form.get("suspended")));
                List<ErrorInfo> errors = services.master.saveCustomer(customer,
                        session.getStaffCd());
                if (errors.isEmpty()) {
                    result = "得意先" + customerCd + "を登録しました。";
                } else {
                    message = messages(errors);
                }
            }
        }

        StringBuilder rows = new StringBuilder();
        for (Customer customer : services.master.listCustomers()) {
            rows.append("<tr><td>").append(Web.escape(customer.getCustomerCd()))
                    .append("</td><td>").append(Web.escape(customer.getName()))
                    .append("</td><td class='num'>").append(customer.getCreditLimit())
                    .append("</td><td>")
                    .append(Web.escape(customer.getClosingType().getLabel()))
                    .append("</td><td>").append(customer.isSuspended() ? "停止中" : "")
                    .append("</td></tr>\n");
        }
        Web.send(exchange, 200, Web.page("得意先マスタ保守",
                WebMain.header(session) + Web.main(
                "<h1>得意先マスタ保守</h1>",
                Web.error(message),
                Web.done(result),
                "<form method='post' action='/master/customer'>",
                "<div class='row'>",
                "<label>得意先コード（8桁）<input type='text' name='customerCd'"
                        + " required></label>",
                "<label>得意先名<input type='text' name='customerName'></label>",
                "<label>与信限度額<input type='text' name='creditLimit'"
                        + " class='num'></label>",
                "</div>",
                "<div class='row'>",
                "<label>締日区分<select name='closingType'>",
                "<option value='1'>20日締め</option>",
                "<option value='2'>末日締め</option>",
                "</select></label>",
                "<label>取引停止<select name='suspended'>",
                "<option value='0'>取引中</option>",
                "<option value='1'>停止中</option>",
                "</select></label>",
                "</div>",
                "<button type='submit'>登録・変更する</button>",
                "</form>",
                "<form method='post' action='/master/customer'>",
                "<input type='hidden' name='action' value='delete'>",
                "<label>削除する得意先コード<input type='text' name='customerCd'"
                        + " required></label>",
                "<button type='submit'>論理削除する</button>",
                "</form>",
                "<h2>得意先の一覧</h2>",
                "<table><tr><th>得意先コード</th><th>得意先名</th><th>与信限度額</th>",
                "<th>締日区分</th><th>取引停止</th></tr>",
                rows.toString(),
                "</table>",
                "<p class='note'>削除は物理削除ではなく削除フラグを立てる論理削除です。</p>",
                Web.back())));
    }

    /** 画面「商品マスタ保守」。商品の登録・変更・論理削除を行う。 */
    public void product(HttpExchange exchange, Session session) throws IOException {
        String message = null;
        String result = null;
        if ("POST".equals(exchange.getRequestMethod())) {
            Map<String, String> form = Web.form(exchange);
            String productCd = form.getOrDefault("productCd", "");
            if ("delete".equals(form.get("action"))) {
                services.master.deleteProduct(productCd, session.getStaffCd());
                result = "商品" + productCd + "を削除しました。";
            } else {
                BigDecimal stdPrice = Web.decimal(form.get("stdPrice"));
                BigDecimal caseQty = Web.decimal(form.get("caseQty"));
                Product product = new Product(productCd,
                        form.getOrDefault("productName", ""),
                        form.getOrDefault("janCd", ""),
                        form.getOrDefault("unit", "本"),
                        caseQty == null ? BigDecimal.ONE : caseQty,
                        stdPrice == null ? BigDecimal.ZERO : stdPrice,
                        form.getOrDefault("taxType", "1"),
                        form.getOrDefault("storageType", "1"), false);
                List<ErrorInfo> errors = services.master.saveProduct(product,
                        session.getStaffCd());
                if (errors.isEmpty()) {
                    result = "商品" + productCd + "を登録しました。";
                } else {
                    message = messages(errors);
                }
            }
        }

        StringBuilder rows = new StringBuilder();
        for (Product product : services.master.listProducts()) {
            rows.append("<tr><td>").append(Web.escape(product.getProductCd()))
                    .append("</td><td>").append(Web.escape(product.getProductName()))
                    .append("</td><td>").append(Web.escape(product.getJanCd()))
                    .append("</td><td class='num'>").append(product.getStdPrice())
                    .append("</td><td>").append(Web.escape(product.getTaxType()))
                    .append("</td></tr>\n");
        }
        Web.send(exchange, 200, Web.page("商品マスタ保守",
                WebMain.header(session) + Web.main(
                "<h1>商品マスタ保守</h1>",
                Web.error(message),
                Web.done(result),
                "<form method='post' action='/master/product'>",
                "<div class='row'>",
                "<label>商品コード（10桁）<input type='text' name='productCd'"
                        + " required></label>",
                "<label>商品名<input type='text' name='productName'></label>",
                "<label>JANコード<input type='text' name='janCd'></label>",
                "</div>",
                "<div class='row'>",
                "<label>標準単価<input type='text' name='stdPrice' class='num'></label>",
                "<label>入数<input type='text' name='caseQty' class='num'></label>",
                "<label>税区分<select name='taxType'>",
                "<option value='1'>標準税率</option>",
                "<option value='2'>軽減税率</option>",
                "<option value='9'>非課税</option>",
                "</select></label>",
                "<label>保管区分<select name='storageType'>",
                "<option value='1'>常温</option>",
                "<option value='2'>冷蔵</option>",
                "<option value='3'>冷凍</option>",
                "</select></label>",
                "</div>",
                "<button type='submit'>登録・変更する</button>",
                "</form>",
                "<form method='post' action='/master/product'>",
                "<input type='hidden' name='action' value='delete'>",
                "<label>削除する商品コード<input type='text' name='productCd'"
                        + " required></label>",
                "<button type='submit'>論理削除する</button>",
                "</form>",
                "<h2>商品の一覧</h2>",
                "<table><tr><th>商品コード</th><th>商品名</th><th>JANコード</th>",
                "<th>標準単価</th><th>税区分</th></tr>",
                rows.toString(),
                "</table>",
                Web.back())));
    }

    private static String messages(List<ErrorInfo> errors) {
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


# ── サーバ本体 ────────────────────────────────────────────────────────────
_WEB_MAIN = r'''package jp.co.contoso.sps.demo;

import com.sun.net.httpserver.HttpExchange;
import com.sun.net.httpserver.HttpHandler;
import com.sun.net.httpserver.HttpServer;

import java.io.IOException;
import java.math.BigDecimal;
import java.net.InetSocketAddress;
import java.time.LocalDate;
import java.util.List;
import java.util.Map;
import java.util.concurrent.Executors;

import jp.co.contoso.sps.common.AuthResult;
import jp.co.contoso.sps.common.Role;
import jp.co.contoso.sps.common.Session;
import jp.co.contoso.sps.common.Warehouse;
import jp.co.contoso.sps.order.OrderDetailDto;
import jp.co.contoso.sps.order.OrderDto;

/**
 * 画面つきの配布サンプル（ブラウザ版）。
 *
 * 画面「ログイン」から認証し、権限に応じたメニューを出す。DB は使わず、
 * すべてメモリ上で動く。
 *
 * 設計書との既知の食い違い: 非機能要件「通信の暗号化」は TLS1.2 以上を求めるが、
 * ここは平文 HTTP で待ち受ける（配布物に証明書を同梱できないため）。
 */
public class WebMain {

    /** セッション ID を入れる cookie の名前。 */
    private static final String COOKIE = "SPSSESSIONID";

    /**
     * メニューに並べる画面。{画面名, 使えるロールのコード, リンク}。
     *
     * 設計書の画面一覧（16 画面）と 1 対 1 に対応させてある。
     */
    private static final String[][] SCREENS = {
        {"受注入力", "1", "/order/new"},
        {"受注一覧照会", "1", "/orders"},
        {"受注取消", "1", "/order/cancel"},
        {"出荷指示", "2", "/shipment"},
        {"EDI受注取込結果照会", "1", "/edi"},
        {"在庫照会", "2", "/stock"},
        {"入庫登録", "2", "/stock/receive"},
        {"棚卸入力", "2", "/stock/count"},
        {"在庫調整", "2", "/stock/adjust"},
        {"請求締め処理", "3", "/billing/close"},
        {"請求書発行", "3", "/billing/invoice"},
        {"入金消込", "3", "/billing/deposit"},
        {"売掛残高照会", "3", "/billing/receivable"},
        {"ログイン", "123", "/login"},
        {"得意先マスタ保守", "9", "/master/customer"},
        {"商品マスタ保守", "9", "/master/product"},
    };

    private final InMemoryRepositories repos = new InMemoryRepositories();
    private final Services services = new Services(repos);
    private final OrderScreens orderScreens = new OrderScreens(services);
    private final InventoryScreens inventoryScreens = new InventoryScreens(services);
    private final BillingScreens billingScreens = new BillingScreens(services);
    private final MasterScreens masterScreens = new MasterScreens(services);

    public static void main(String[] args) throws IOException {
        int port = args.length > 0 ? Integer.parseInt(args[0]) : 8080;
        new WebMain().start(port);
    }

    /**
     * 受注・売上・入金を何件か積んでおく。
     *
     * 受注一覧照会に出すためと、請求締め → 請求書発行 → 入金消込を画面から
     * 一通り試せるようにするため。本来は出荷実績の受信と全銀ネットからの
     * 受信で積まれるデータである。
     */
    private void seedOrders() {
        register("10001", LocalDate.of(2026, 1, 15), LocalDate.of(2026, 1, 19),
                "4901234001", "3", "4901234055", "5");
        register("00010002", LocalDate.of(2026, 1, 15), LocalDate.of(2026, 1, 19),
                "4901234001", "10", null, null);
        register("00010001", LocalDate.of(2026, 1, 16), LocalDate.of(2026, 1, 20),
                "4901234002", "4", null, null);

        repos.seedSales("00010001", LocalDate.of(2026, 1, 6), "4901234001",
                new BigDecimal("10005"), "1");
        repos.seedSales("00010001", LocalDate.of(2026, 1, 13), "4901234055",
                new BigDecimal("10005"), "1");
        repos.seedSales("00010001", LocalDate.of(2026, 1, 20), "4901234002",
                new BigDecimal("2940"), "1");
        repos.seedDeposit("D20260225001", LocalDate.of(2026, 2, 25), "00010001",
                new BigDecimal("5000"), "チユウオウフードサービス", null);
    }

    private void register(String customerCd, LocalDate orderDate, LocalDate deliveryDate,
                          String productCd1, String qty1,
                          String productCd2, String qty2) {
        java.util.List<OrderDetailDto> details = new java.util.ArrayList<>();
        details.add(new OrderDetailDto(productCd1, new BigDecimal(qty1), "1"));
        if (productCd2 != null) {
            details.add(new OrderDetailDto(productCd2, new BigDecimal(qty2), "1"));
        }
        OrderDto dto = new OrderDto(customerCd, orderDate, deliveryDate, details);
        dto.setEntryStaffCd("100001");
        services.regist.registerOrder(dto);
    }

    /** サーバを立てる。 */
    public void start(int port) throws IOException {
        seedOrders();
        HttpServer server = HttpServer.create(new InetSocketAddress("127.0.0.1", port), 0);
        server.createContext("/", this::handleRoot);
        server.createContext("/login", this::handleLogin);
        server.createContext("/menu", this::handleMenu);
        server.createContext("/password", this::handlePassword);
        server.createContext("/logout", this::handleLogout);

        screen(server, "/order/new", "1", orderScreens::input);
        screen(server, "/orders", "1", orderScreens::list);
        screen(server, "/order/detail", "1", orderScreens::detail);
        screen(server, "/order/cancel", "1", orderScreens::cancel);
        screen(server, "/shipment", "2", orderScreens::shipment);
        screen(server, "/edi", "1", orderScreens::edi);

        screen(server, "/stock", "2", inventoryScreens::stock);
        screen(server, "/stock/receive", "2", inventoryScreens::receive);
        screen(server, "/stock/count", "2", inventoryScreens::count);
        screen(server, "/stock/adjust", "2", inventoryScreens::adjust);

        screen(server, "/billing/close", "3", billingScreens::close);
        screen(server, "/billing/invoice", "3", billingScreens::invoice);
        screen(server, "/billing/deposit", "3", billingScreens::deposit);
        screen(server, "/billing/receivable", "3", billingScreens::receivable);

        screen(server, "/master/customer", "9", masterScreens::customer);
        screen(server, "/master/product", "9", masterScreens::product);

        server.setExecutor(Executors.newFixedThreadPool(8));
        server.start();
        System.out.println("ログイン画面: http://127.0.0.1:" + port + "/login");
        System.out.println("社員コードとパスワードは画面に表示してあります。");
        System.out.println("停止するには Ctrl+C。");
    }

    /** ログインと権限の確認を挟んでから画面へ渡す。 */
    private void screen(HttpServer server, String path, String allowedCodes,
                        ScreenHandler handler) {
        server.createContext(path, exchange -> {
            Session session = session(exchange);
            if (session == null) {
                Web.redirect(exchange, "/login");
                return;
            }
            if (!visible(session.getRole(), allowedCodes)) {
                Web.send(exchange, 403, Web.page("権限がありません",
                        header(session) + Web.main(
                        "<h1>権限がありません</h1>",
                        "<p>この画面はいまの権限では使えません。</p>",
                        Web.back())));
                return;
            }
            handler.handle(exchange, session);
        });
    }

    /** ログイン済みの利用者へ画面を返す処理。 */
    private interface ScreenHandler {
        void handle(HttpExchange exchange, Session session) throws IOException;
    }

    // ── 共通の画面 ──────────────────────────────────────────────────────────

    private void handleRoot(HttpExchange exchange) throws IOException {
        Web.redirect(exchange, session(exchange) == null ? "/login" : "/menu");
    }

    private void handleLogin(HttpExchange exchange) throws IOException {
        if ("POST".equals(exchange.getRequestMethod())) {
            Map<String, String> form = Web.form(exchange);
            String staffCd = form.getOrDefault("staffCd", "");
            AuthResult result = services.auth.authenticate(staffCd,
                    form.getOrDefault("password", ""));
            if (!result.isOk()) {
                Web.send(exchange, 401, loginPage(result.getMessage(), staffCd));
                return;
            }
            exchange.getResponseHeaders().add("Set-Cookie",
                    COOKIE + "=" + result.getSession().getSessionId()
                    + "; Path=/; HttpOnly; SameSite=Strict");
            Web.redirect(exchange, result.isPasswordExpired() ? "/password" : "/menu");
            return;
        }
        Web.send(exchange, 200, loginPage(null, ""));
    }

    private void handleMenu(HttpExchange exchange) throws IOException {
        Session session = session(exchange);
        if (session == null) {
            Web.redirect(exchange, "/login");
            return;
        }
        StringBuilder rows = new StringBuilder();
        for (String[] screen : SCREENS) {
            boolean allowed = visible(session.getRole(), screen[1]);
            String cell = allowed
                    ? "<a href='" + screen[2] + "'>開く</a>"
                    : "<span class='ng'>権限なし</span>";
            rows.append("<tr><td>").append(Web.escape(screen[0])).append("</td><td>")
                    .append(cell).append("</td></tr>\n");
        }
        Web.send(exchange, 200, Web.page("メニュー", header(session) + Web.main(
                "<h1>メニュー</h1>",
                "<p class='note'>設計書の画面一覧（16 画面）と 1 対 1 に対応しています。",
                "いまの権限で使えない画面は「権限なし」と出ます。</p>",
                "<table><tr><th>画面名</th><th></th></tr>",
                rows.toString(),
                "</table>")));
    }

    private void handlePassword(HttpExchange exchange) throws IOException {
        Session session = session(exchange);
        if (session == null) {
            Web.redirect(exchange, "/login");
            return;
        }
        String message = "パスワードの有効期限が切れています。新しいパスワードを設定してください。";
        if ("POST".equals(exchange.getRequestMethod())) {
            Map<String, String> form = Web.form(exchange);
            String failed = services.auth.changePassword(session.getStaffCd(),
                    form.getOrDefault("current", ""), form.getOrDefault("next", ""));
            if (failed == null) {
                Web.send(exchange, 200, Web.page("パスワード変更",
                        header(session) + Web.main(
                        "<h1>パスワードを変更しました</h1>",
                        "<p><a href='/menu'>メニューへ進む</a></p>")));
                return;
            }
            message = failed;
        }
        Web.send(exchange, 200, Web.page("パスワード変更", header(session) + Web.main(
                "<h1>パスワード変更</h1>",
                Web.error(message),
                "<form method='post' action='/password'>",
                "<label>現在のパスワード<input type='password' name='current' required></label>",
                "<label>新しいパスワード<input type='password' name='next' required></label>",
                "<button type='submit'>変更する</button>",
                "</form>",
                "<p class='note'>8 文字以上・英数記号混在。90 日ごとに変更が必要です。</p>")));
    }

    private void handleLogout(HttpExchange exchange) throws IOException {
        Session session = session(exchange);
        if (session != null) {
            services.auth.logout(session.getSessionId());
        }
        exchange.getResponseHeaders().add("Set-Cookie",
                COOKIE + "=; Path=/; HttpOnly; Max-Age=0");
        Web.redirect(exchange, "/login");
    }

    // ── 部品 ────────────────────────────────────────────────────────────────

    private String loginPage(String message, String staffCd) {
        return Web.page("ログイン", Web.html(
                "<div class='login'>",
                "<h1>新販売管理システム</h1>",
                Web.error(message),
                "<form method='post' action='/login'>",
                "<label>社員コード<input type='text' name='staffCd' value='"
                        + Web.escape(staffCd) + "' autofocus required></label>",
                "<label>パスワード<input type='password' name='password' required></label>",
                "<button type='submit'>ログイン</button>",
                "</form>",
                "<div class='note'>",
                "<p>この資材は架空のデモです。以下で試せます。</p>",
                "<table>",
                "<tr><th>社員コード</th><th>パスワード</th><th>権限</th></tr>",
                "<tr><td>100001</td><td>Sales#2026</td><td>営業担当</td></tr>",
                "<tr><td>100002</td><td>Ware#2026a</td><td>倉庫担当</td></tr>",
                "<tr><td>100003</td><td>Acct#2026b</td><td>経理担当</td></tr>",
                "<tr><td>100099</td><td>Admin#2026</td><td>管理者（全画面）</td></tr>",
                "<tr><td>100004</td><td>Old#20251</td><td>営業担当（期限切れ）</td></tr>",
                "</table>",
                "</div>",
                "</div>"));
    }

    /** 画面の上端に出す帯。 */
    public static String header(Session session) {
        return Web.html("<div class='bar'>",
                "<span>" + Web.escape(session.getStaffName()) + " さん（"
                        + Web.escape(session.getRole().getLabel()) + "）</span>",
                "<a href='/logout'>ログアウト</a>",
                "</div>");
    }

    /** 倉庫の選択肢。複数の画面から使う。 */
    public static String warehouseSelect(Services services, String name) {
        StringBuilder options = new StringBuilder("<select name='" + name + "'>");
        List<Warehouse> warehouses = services.master.listWarehouses();
        for (Warehouse warehouse : warehouses) {
            options.append("<option value='").append(Web.escape(warehouse.getWarehouseCd()))
                    .append("'>").append(Web.escape(warehouse.getWarehouseCd()))
                    .append(" ").append(Web.escape(warehouse.getWarehouseName()))
                    .append("</option>");
        }
        return options.append("</select>").toString();
    }

    /** ロールがその画面を使えるか。管理者は全画面を使える。 */
    private static boolean visible(Role role, String allowedCodes) {
        return role == Role.ADMIN || allowedCodes.contains(role.getCode());
    }

    private Session session(HttpExchange exchange) {
        List<String> cookies = exchange.getRequestHeaders().get("Cookie");
        if (cookies == null) {
            return null;
        }
        for (String line : cookies) {
            for (String part : line.split(";")) {
                String item = part.trim();
                if (item.startsWith(COOKIE + "=")) {
                    return services.auth.find(item.substring(COOKIE.length() + 1));
                }
            }
        }
        return null;
    }
}
'''


WEB: dict[str, str] = {
    "demo/Web.java": _WEB,
    "demo/WebMain.java": _WEB_MAIN,
    "demo/OrderScreens.java": _ORDER_SCREENS,
    "demo/InventoryScreens.java": _INVENTORY_SCREENS,
    "demo/BillingScreens.java": _BILLING_SCREENS,
    "demo/MasterScreens.java": _MASTER_SCREENS,
}
