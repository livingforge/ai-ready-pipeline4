"""認証まわりの Java ソース（`common` の認証サービスと `demo` のログイン画面）。

## なぜ増やしたか

`資料/` には認証が一式そろって定義されているのに、コード側に一つも無かった。

- 機能「利用者認証」（CMN）—— 社員コードとパスワードで認証し、権限に応じて機能を制限する
- 画面「ログイン」（CMN・入力）—— 社員コードとパスワードで利用者を認証する
- テーブル `M_STAFF` 社員マスタ —— 利用者の認証情報と権限ロールを保持する
- 詳細設計「認証サービス」`AuthService` —— 社員コードとパスワードを照合し、
  セッションと権限情報を発行する（利用元は「全画面（認証フィルタ）」）
- 非機能「パスワード方針」—— 8 文字以上・英数記号混在、90 日ごとに変更を強制

つまりここで足すのは**発明ではなく、設計書にあってコードに無かったものの実装**である。
クラス名は `spec.MODULES` の `AuthService` に合わせる（名寄せの手がかりを壊さない）。

## 設計書との既知の食い違い

非機能要件「通信の暗号化」は TLS1.2 以上を求めるが、ここで立てるのは平文 HTTP の
ローカルサーバである。配布物に証明書を同梱できないためで、**新たな
コードと設計書の食い違い**として残る（C1〜C5 と同じ性質のもの）。

画面は `demo` パッケージに置く。基本設計のソフトウェア構成は Spring Boot だが、
外部依存を持たない方針は変えられないので、JDK 内蔵の HTTP サーバで代替する。
`demo` は「動かすための足場で、設計書に対応する成果物ではない」という位置づけを
そのまま引き継ぐ。
"""

from __future__ import annotations

_ROLE = '''package jp.co.contoso.sps.common;

/**
 * 権限ロール（M_STAFF.roleCd）。
 *
 * 利用者に与える権限の区分。ログイン後に使える画面をこの区分で絞る。
 *
 * 関連する機能: 利用者認証
 * 関連する業務ルール: 権限に応じた機能の制限
 */
public enum Role {

    /** 営業担当。受注の入力・照会・取消を行う。 */
    SALES("1", "営業担当"),

    /** 倉庫担当。在庫の照会と出荷指示を行う。 */
    WAREHOUSE("2", "倉庫担当"),

    /** 経理担当。請求の締めと売掛の照会を行う。 */
    ACCOUNTING("3", "経理担当"),

    /** 管理者。全画面を使える。 */
    ADMIN("9", "管理者");

    private final String code;
    private final String label;

    Role(String code, String label) {
        this.code = code;
        this.label = label;
    }

    public String getCode() {
        return code;
    }

    public String getLabel() {
        return label;
    }
}
'''


_STAFF = '''package jp.co.contoso.sps.common;

import java.time.LocalDate;

/**
 * 社員（M_STAFF）。
 *
 * 利用者の認証情報と権限ロールを保持する。パスワードは平文では持たず、
 * ソルトと反復ハッシュの結果だけを保持する。
 *
 * 関連する機能: 利用者認証
 */
public class Staff {

    private final String staffCd;
    private final String staffName;
    private final Role role;
    private String passwordHash;
    private String passwordSalt;
    private LocalDate passwordChangedOn;

    public Staff(String staffCd, String staffName, Role role, String passwordHash,
                 String passwordSalt, LocalDate passwordChangedOn) {
        this.staffCd = staffCd;
        this.staffName = staffName;
        this.role = role;
        this.passwordHash = passwordHash;
        this.passwordSalt = passwordSalt;
        this.passwordChangedOn = passwordChangedOn;
    }

    public String getStaffCd() {
        return staffCd;
    }

    public String getStaffName() {
        return staffName;
    }

    public Role getRole() {
        return role;
    }

    public String getPasswordHash() {
        return passwordHash;
    }

    public void setPasswordHash(String passwordHash) {
        this.passwordHash = passwordHash;
    }

    public String getPasswordSalt() {
        return passwordSalt;
    }

    public void setPasswordSalt(String passwordSalt) {
        this.passwordSalt = passwordSalt;
    }

    public LocalDate getPasswordChangedOn() {
        return passwordChangedOn;
    }

    public void setPasswordChangedOn(LocalDate passwordChangedOn) {
        this.passwordChangedOn = passwordChangedOn;
    }
}
'''


_SESSION = '''package jp.co.contoso.sps.common;

import java.time.LocalDateTime;

/**
 * ログインセッション。
 *
 * 認証に成功した利用者の識別子と権限情報を持つ。認証サービスが発行し、
 * 画面はセッション ID を手がかりにこれを引く。
 *
 * 関連する機能: 利用者認証
 */
public class Session {

    private final String sessionId;
    private final String staffCd;
    private final String staffName;
    private final Role role;
    private final LocalDateTime loginDatetime;

    public Session(String sessionId, String staffCd, String staffName, Role role,
                   LocalDateTime loginDatetime) {
        this.sessionId = sessionId;
        this.staffCd = staffCd;
        this.staffName = staffName;
        this.role = role;
        this.loginDatetime = loginDatetime;
    }

    public String getSessionId() {
        return sessionId;
    }

    public String getStaffCd() {
        return staffCd;
    }

    public String getStaffName() {
        return staffName;
    }

    public Role getRole() {
        return role;
    }

    public LocalDateTime getLoginDatetime() {
        return loginDatetime;
    }
}
'''


_AUTH_RESULT = '''package jp.co.contoso.sps.common;

/**
 * 認証の結果。
 *
 * 認証に成功したときはセッションを、失敗したときは画面に出すメッセージを持つ。
 * パスワードの有効期限が切れている場合は、セッションを発行したうえで
 * 変更を促す状態（passwordExpired）にする。
 *
 * 関連する機能: 利用者認証
 */
public class AuthResult {

    private final Session session;
    private final String message;
    private final boolean passwordExpired;

    private AuthResult(Session session, String message, boolean passwordExpired) {
        this.session = session;
        this.message = message;
        this.passwordExpired = passwordExpired;
    }

    /** 認証に成功した。 */
    public static AuthResult ok(Session session) {
        return new AuthResult(session, null, false);
    }

    /** 認証には成功したが、パスワードの変更が必要である。 */
    public static AuthResult expired(Session session) {
        return new AuthResult(session, "パスワードの有効期限が切れています。変更してください。", true);
    }

    /** 認証に失敗した。 */
    public static AuthResult error(String message) {
        return new AuthResult(null, message, false);
    }

    public boolean isOk() {
        return session != null;
    }

    public Session getSession() {
        return session;
    }

    public String getMessage() {
        return message;
    }

    public boolean isPasswordExpired() {
        return passwordExpired;
    }
}
'''


_PASSWORD_POLICY = '''package jp.co.contoso.sps.common;

import java.time.LocalDate;

import jp.co.contoso.sps.framework.Component;

/**
 * パスワード方針。
 *
 * 非機能要件「パスワード方針」に対応する。パスワードは 8 文字以上・英数記号
 * 混在とし、90 日ごとに変更を強制する。
 *
 * 関連する機能: 利用者認証
 */
@Component
public class PasswordPolicy {

    /** 最小文字数。 */
    private static final int MIN_LENGTH = 8;

    /** 変更してから次に変更を強制するまでの日数。 */
    private static final int VALID_DAYS = 90;

    /**
     * 書式を検査する。
     *
     * 違反していれば画面に出すメッセージを、問題なければ null を返す。
     */
    public String validate(String password) {
        if (password == null || password.length() < MIN_LENGTH) {
            return "パスワードは " + MIN_LENGTH + " 文字以上にしてください。";
        }
        boolean hasLetter = false;
        boolean hasDigit = false;
        boolean hasSymbol = false;
        for (int i = 0; i < password.length(); i++) {
            char ch = password.charAt(i);
            if (Character.isLetter(ch)) {
                hasLetter = true;
            } else if (Character.isDigit(ch)) {
                hasDigit = true;
            } else {
                hasSymbol = true;
            }
        }
        if (!hasLetter || !hasDigit || !hasSymbol) {
            return "パスワードは英字・数字・記号を混在させてください。";
        }
        return null;
    }

    /** 最後に変更した日から 90 日を過ぎているか。 */
    public boolean isExpired(LocalDate changedOn, LocalDate today) {
        if (changedOn == null) {
            return true;
        }
        return changedOn.plusDays(VALID_DAYS).isBefore(today);
    }

    /** 変更を強制するまでの日数。 */
    public int getValidDays() {
        return VALID_DAYS;
    }
}
'''


_PASSWORD_HASHER = '''package jp.co.contoso.sps.common;

import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.security.SecureRandom;
import java.security.spec.InvalidKeySpecException;
import java.util.Base64;

import javax.crypto.SecretKeyFactory;
import javax.crypto.spec.PBEKeySpec;

import jp.co.contoso.sps.framework.Component;

/**
 * パスワードのハッシュ化部品。
 *
 * 社員マスタにはパスワードそのものを持たせず、ソルトと反復ハッシュの結果を
 * 保持する。照合は先頭一致で早く抜けないよう、時間差の出ない比較で行う。
 *
 * 関連する機能: 利用者認証
 */
@Component
public class PasswordHasher {

    /** 反復回数。大きいほど総当たりに時間がかかる。 */
    private static final int ITERATIONS = 210000;

    /** 鍵長（ビット）。 */
    private static final int KEY_LENGTH = 256;

    private final SecureRandom random = new SecureRandom();

    /** ソルトを新しく作る。 */
    public String newSalt() {
        byte[] salt = new byte[16];
        random.nextBytes(salt);
        return Base64.getEncoder().encodeToString(salt);
    }

    /** パスワードとソルトからハッシュを求める。 */
    public String hash(String password, String salt) {
        try {
            PBEKeySpec spec = new PBEKeySpec(password.toCharArray(),
                    Base64.getDecoder().decode(salt), ITERATIONS, KEY_LENGTH);
            SecretKeyFactory factory =
                    SecretKeyFactory.getInstance("PBKDF2WithHmacSHA256");
            return Base64.getEncoder().encodeToString(factory.generateSecret(spec).getEncoded());
        } catch (NoSuchAlgorithmException | InvalidKeySpecException e) {
            throw new IllegalStateException("パスワードのハッシュ化に失敗しました。", e);
        }
    }

    /** 入力されたパスワードが、保存してあるハッシュと一致するか。 */
    public boolean matches(String password, String salt, String expectedHash) {
        if (password == null || salt == null || expectedHash == null) {
            return false;
        }
        byte[] actual = Base64.getDecoder().decode(hash(password, salt));
        byte[] expected = Base64.getDecoder().decode(expectedHash);
        return MessageDigest.isEqual(actual, expected);
    }
}
'''


_STAFF_REPOSITORY = '''package jp.co.contoso.sps.common.repository;

import java.time.LocalDate;
import java.util.List;

import jp.co.contoso.sps.common.Staff;

/**
 * 社員マスタ（M_STAFF）への参照と更新。
 */
public interface StaffRepository {

    Staff find(String staffCd);

    List<Staff> findAll();

    void save(Staff staff);

    void updatePassword(String staffCd, String passwordHash, String passwordSalt, LocalDate changedOn);
}
'''


_AUTH_SERVICE = '''package jp.co.contoso.sps.common;

import java.security.SecureRandom;
import java.time.LocalDate;
import java.time.LocalDateTime;
import java.util.Base64;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;

import jp.co.contoso.sps.common.repository.StaffRepository;
import jp.co.contoso.sps.framework.Service;

/**
 * 認証サービス。
 *
 * 社員コードとパスワードを照合し、セッションと権限情報を発行する。
 *
 * 関連する機能: 利用者認証 / ログイン・ログアウト
 * 関連する画面: ログイン（全画面の認証フィルタから呼ばれる）
 * 関連する業務ルール: パスワード方針 / 権限に応じた機能の制限
 *
 * 社員コードが無い場合とパスワードが違う場合で応答を変えない。どちらが誤りかを
 * 教えると、実在する社員コードを総当たりで割り出せてしまうため。
 */
@Service
public class AuthService {

    /** 認証に失敗したときのメッセージ。どちらの誤りかは明かさない。 */
    private static final String FAILED = "社員コードまたはパスワードが違います。";

    private final StaffRepository staffRepository;
    private final PasswordHasher passwordHasher;
    private final PasswordPolicy passwordPolicy;
    private final AuditLogger auditLogger;
    private final SecureRandom random = new SecureRandom();
    private final Map<String, Session> sessions = new ConcurrentHashMap<>();

    public AuthService(StaffRepository staffRepository, PasswordHasher passwordHasher,
                       PasswordPolicy passwordPolicy, AuditLogger auditLogger) {
        this.staffRepository = staffRepository;
        this.passwordHasher = passwordHasher;
        this.passwordPolicy = passwordPolicy;
        this.auditLogger = auditLogger;
    }

    /**
     * 社員コードとパスワードを照合し、セッションを発行する。
     *
     * パスワードの有効期限が切れている場合もセッションは発行するが、
     * 変更を促す状態にして返す。
     */
    public AuthResult authenticate(String staffCd, String password) {
        Staff staff = staffRepository.find(staffCd);
        if (staff == null) {
            auditLogger.record("ログイン", staffCd, null, "失敗（社員コードなし）");
            return AuthResult.error(FAILED);
        }
        if (!passwordHasher.matches(password, staff.getPasswordSalt(),
                staff.getPasswordHash())) {
            auditLogger.record("ログイン", staffCd, null, "失敗（パスワード相違）");
            return AuthResult.error(FAILED);
        }

        Session session = new Session(newSessionId(), staff.getStaffCd(),
                staff.getStaffName(), staff.getRole(), LocalDateTime.now());
        sessions.put(session.getSessionId(), session);

        if (passwordPolicy.isExpired(staff.getPasswordChangedOn(), LocalDate.now())) {
            auditLogger.record("ログイン", staffCd, null, "成功（パスワード期限切れ）");
            return AuthResult.expired(session);
        }
        auditLogger.record("ログイン", staffCd, null, "成功");
        return AuthResult.ok(session);
    }

    /** セッション ID から利用者を引く。無効なら null。 */
    public Session find(String sessionId) {
        if (sessionId == null) {
            return null;
        }
        return sessions.get(sessionId);
    }

    /** ログアウトしてセッションを破棄する。 */
    public void logout(String sessionId) {
        Session session = sessions.remove(sessionId);
        if (session != null) {
            auditLogger.record("ログアウト", session.getStaffCd(), "ログイン中", "ログアウト");
        }
    }

    /**
     * パスワードを変更する。
     *
     * 書式が方針に反していればメッセージを返し、変更しない。成功したら null。
     */
    public String changePassword(String staffCd, String currentPassword,
                                 String newPassword) {
        Staff staff = staffRepository.find(staffCd);
        if (staff == null || !passwordHasher.matches(currentPassword,
                staff.getPasswordSalt(), staff.getPasswordHash())) {
            return FAILED;
        }
        String violation = passwordPolicy.validate(newPassword);
        if (violation != null) {
            return violation;
        }
        String salt = passwordHasher.newSalt();
        staffRepository.updatePassword(staffCd, passwordHasher.hash(newPassword, salt),
                salt, LocalDate.now());
        auditLogger.record("パスワード変更", staffCd, "変更前", "変更後");
        return null;
    }

    private String newSessionId() {
        byte[] bytes = new byte[32];
        random.nextBytes(bytes);
        return Base64.getUrlEncoder().withoutPadding().encodeToString(bytes);
    }
}
'''


AUTH: dict[str, str] = {
    "common/Role.java": _ROLE,
    "common/Staff.java": _STAFF,
    "common/Session.java": _SESSION,
    "common/AuthResult.java": _AUTH_RESULT,
    "common/PasswordPolicy.java": _PASSWORD_POLICY,
    "common/PasswordHasher.java": _PASSWORD_HASHER,
    "common/AuthService.java": _AUTH_SERVICE,
    "common/repository/StaffRepository.java": _STAFF_REPOSITORY,
}
