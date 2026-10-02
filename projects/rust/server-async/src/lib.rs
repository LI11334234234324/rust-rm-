pub mod http;

use pbkdf2::pbkdf2_hmac;
use rand::{RngCore, rngs::OsRng};
use serde_json::{Value, json};
use sha2::Sha256;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use subtle::ConstantTimeEq;

pub struct Route {
    pub method: &'static str,
    pub path: &'static str,
    pub needs_auth: bool,
}

impl Route {
    const fn new(method: &'static str, path: &'static str, needs_auth: bool) -> Self {
        Self {
            method,
            path,
            needs_auth,
        }
    }
}

/// 路由的唯一真源：404/405 判定、鉴权要求、启动横幅都从这张表读。
/// 路径里的 `{name}` 匹配恰好一段非空、不含 `/` 的名字。
pub const ROUTES: &[Route] = &[
    Route::new("GET", "/ping", false),
    Route::new("POST", "/users", false),
    Route::new("POST", "/sessions", false),
    Route::new("DELETE", "/sessions/current", true),
    Route::new("GET", "/texts", true),
    Route::new("POST", "/echo", false),
    Route::new("DELETE", "/users/me", true),
    Route::new("PUT", "/texts/{name}", true),
    Route::new("GET", "/texts/{name}", true),
    Route::new("DELETE", "/texts/{name}", true),
];

fn path_matches(pattern: &str, path: &str) -> bool {
    match pattern.strip_suffix("{name}") {
        Some(prefix) => path
            .strip_prefix(prefix)
            .is_some_and(|name| !name.is_empty() && !name.contains('/')),
        None => pattern == path,
    }
}

/// 命中哪条路由。`Err(404)` 是没有任何模式匹配这个路径，`Err(405)` 是路径匹配了
/// 但方法不对；名字的形状归这里判，字符集和长度归 `valid_name`。
pub fn route(method: &str, path: &str) -> Result<&'static Route, u16> {
    let mut path_matched = false;
    for route in ROUTES {
        if !path_matches(route.path, path) {
            continue;
        }
        if route.method == method {
            return Ok(route);
        }
        path_matched = true;
    }
    Err(if path_matched { 405 } else { 404 })
}

pub fn route_error(method: &str, path: &str) -> Option<u16> {
    route(method, path).err()
}

#[derive(Clone)]
struct Session {
    token: String,
    /// `None` 表示配置的 TTL 超出 `Instant` 能表示的范围（如 `u64::MAX` 秒），即不会过期。
    deadline: Option<Instant>,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("token", &"<redacted>")
            .field("deadline", &self.deadline)
            .finish()
    }
}

impl Session {
    fn is_valid(&self, token: &str, now: Instant) -> bool {
        bool::from(self.token.as_bytes().ct_eq(token.as_bytes()))
            && self.deadline.is_none_or(|deadline| now < deadline)
    }
}

struct User {
    salt: [u8; 16],
    digest: [u8; 32],
    session: Option<Session>,
    texts: BTreeMap<String, String>,
}

impl User {
    fn new(salt: [u8; 16], digest: [u8; 32]) -> Self {
        Self {
            salt,
            digest,
            session: None,
            texts: BTreeMap::new(),
        }
    }

    fn start_session(&mut self, token: String, ttl: Duration, now: Instant) {
        self.session = Some(Session {
            token,
            deadline: now.checked_add(ttl),
        });
    }

    fn clear_session(&mut self) {
        self.session = None;
    }

    fn has_valid_token(&self, token: &str, now: Instant) -> bool {
        self.session
            .as_ref()
            .is_some_and(|s| s.is_valid(token, now))
    }
}

/// 业务时间的来源：过期判定和令牌签发都必须从同一处取"现在"。
/// 生产用 `SystemClock`，测试用手动时钟把时刻拨到想要的位置。
pub trait Clock: Send + Sync {
    fn now(&self) -> Instant;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

impl<T: Clock + ?Sized> Clock for Arc<T> {
    fn now(&self) -> Instant {
        (**self).now()
    }
}

pub struct Service {
    users: Mutex<BTreeMap<String, User>>,
    token_ttl_seconds: u64,
    clock: Box<dyn Clock>,
}

impl Service {
    pub fn new(token_ttl_seconds: u64) -> Self {
        Self::with_clock(token_ttl_seconds, Box::new(SystemClock))
    }

    /// 用指定的时钟构造，测试借此控制"现在几点"而不必真的等过去。
    pub fn with_clock(token_ttl_seconds: u64, clock: Box<dyn Clock>) -> Self {
        assert!(token_ttl_seconds > 0, "token_ttl_seconds must be positive");
        Self {
            users: Mutex::new(BTreeMap::new()),
            token_ttl_seconds,
            clock,
        }
    }

    /// 拿锁，顺手把中毒恢复掉：互斥锁中毒不会自愈，`into_inner` 让一次 panic 只
    /// 影响那一个请求，而不是此后每个碰状态的请求。
    fn users(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, User>> {
        self.users.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 拿锁并在临界区内取时刻。顺序由签名保证：调用方在拿到 guard 之前手里
    /// 没有 `now` 可用，所以写不出"在锁外取时刻"的那个 bug。
    fn with_users<T>(&self, f: impl FnOnce(&mut BTreeMap<String, User>, Instant) -> T) -> T {
        let mut users = self.users();
        let now = self.clock.now();
        f(&mut users, now)
    }
}

impl Default for Service {
    fn default() -> Self {
        Self::new(300)
    }
}

/// 登录前读取的凭据快照。
///
/// 把"读取凭据"和"提交令牌"分成两步，并发场景才可断言：读取快照之后账号
/// 可能被注销并同名重注册，此时必须拒绝用旧快照提交令牌。
#[derive(Clone, Copy)]
pub struct Credentials {
    salt: [u8; 16],
    digest: [u8; 32],
}

impl Service {
    /// 读取用户当前的凭据快照；用户名不存在时返回 `None`。
    pub fn read_credentials(&self, name: &str) -> Option<Credentials> {
        let users = self.users();
        users.get(name).map(|user| Credentials {
            salt: user.salt,
            digest: user.digest,
        })
    }

    /// 用 `read_credentials` 取得的快照校验密码并签发新令牌。
    ///
    /// 快照已经失效时（例如账号在读取后被注销并同名重注册）返回 401，
    /// 不会把令牌签发给新的同名账号。
    pub fn login_with(
        &self,
        name: &str,
        password: &str,
        credentials: &Credentials,
    ) -> (u16, Value) {
        let digest = password_hash(password, &credentials.salt);
        self.with_users(|users, now| {
            let Some(user) = users.get_mut(name) else {
                return error(401, "Invalid username or password");
            };
            if user.salt != credentials.salt || !bool::from(digest.ct_eq(&credentials.digest)) {
                return error(401, "Invalid username or password");
            }
            let token = new_token();
            user.start_session(
                token.clone(),
                Duration::from_secs(self.token_ttl_seconds),
                now,
            );
            (
                200,
                json!({"data": {"token": token, "expires_in": self.token_ttl_seconds}}),
            )
        })
    }
}

pub fn error(status: u16, message: &str) -> (u16, Value) {
    (status, json!({"message": message}))
}

pub(crate) fn extract_text_field(body: &Value) -> Result<&str, (u16, Value)> {
    let Some(text) = body.get("text").and_then(Value::as_str) else {
        return Err(error(400, "Expected text"));
    };
    if body.as_object().map(|v| v.len()) != Some(1) {
        return Err(error(400, "Invalid fields"));
    }
    if text.len() > 65_536 {
        return Err(error(413, "Text too large"));
    }
    Ok(text)
}

pub fn valid_name(name: &str, max: usize) -> bool {
    !name.is_empty()
        && name.len() <= max
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn password_hash(password: &str, salt: &[u8; 16]) -> [u8; 32] {
    let mut output = [0; 32];
    pbkdf2_hmac::<Sha256>(password.as_bytes(), salt, 100_000, &mut output);
    output
}

fn new_token() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

impl Service {
    pub fn handle(
        &self,
        method: &str,
        path: &str,
        body: &Value,
        authorization: &str,
    ) -> (u16, Value) {
        let matched = match route(method, path) {
            Ok(matched) => matched,
            Err(status) => {
                return error(
                    status,
                    if status == 404 {
                        "Not found"
                    } else {
                        "Method not allowed"
                    },
                );
            }
        };
        if method == "GET" && path == "/ping" {
            return (200, json!({"data": "pong"}));
        }
        if method == "POST" && path == "/echo" {
            let text = match extract_text_field(body) {
                Ok(text) => text,
                Err(err) => return err,
            };
            return (200, json!({"data": text}));
        }
        if method == "POST" && matches!(path, "/users" | "/sessions") {
            let Some(name) = body.get("username").and_then(Value::as_str) else {
                return error(400, "Expected username");
            };
            let Some(password) = body.get("password").and_then(Value::as_str) else {
                return error(400, "Expected password");
            };
            if body.as_object().map(|v| v.len()) != Some(2)
                || !valid_name(name, 32)
                || !(8..=128).contains(&password.chars().count())
            {
                return error(400, "Invalid account fields");
            }
            if path == "/users" {
                let mut salt = [0; 16];
                OsRng.fill_bytes(&mut salt);
                let digest = password_hash(password, &salt);
                let mut users = self.users();
                if users.contains_key(name) {
                    return error(409, "Username exists");
                }
                users.insert(name.into(), User::new(salt, digest));
                return (201, json!({"data": {"username": name}}));
            }
            let Some(credentials) = self.read_credentials(name) else {
                return error(401, "Invalid username or password");
            };
            return self.login_with(name, password, &credentials);
        }
        if matched.needs_auth {
            let token = authorization.strip_prefix("Bearer ").unwrap_or("");
            return self.with_users(|users, now| {
                let name = users
                    .iter()
                    .find(|(_, user)| user.has_valid_token(token, now))
                    .map(|(name, _)| name.clone());
                let Some(name) = name else {
                    return error(401, "Login required");
                };
                if method == "DELETE" && path == "/users/me" {
                    users.remove(&name);
                    return (200, json!({"data": null}));
                }
                let user = users.get_mut(&name).unwrap();
                if method == "DELETE" && path == "/sessions/current" {
                    user.clear_session();
                    return (200, json!({"data": null}));
                }
                if method == "GET" && path == "/texts" {
                    return (200, json!({"data": user.texts.keys().collect::<Vec<_>>()}));
                }
                if let Some(text_name) = path.strip_prefix("/texts/") {
                    if !valid_name(text_name, 64) {
                        return error(400, "Invalid text name");
                    }
                    if method == "GET" {
                        if let Some(text) = user.texts.get(text_name) {
                            return (200, json!({"data": text}));
                        } else {
                            return error(404, "Text not found");
                        }
                    }
                    if method == "PUT" {
                        let text = match extract_text_field(body) {
                            Ok(text) => text,
                            Err(err) => return err,
                        };
                        user.texts.insert(text_name.to_owned(), text.to_owned());
                        return (200, json!({"data": null}));
                    }
                    if method == "DELETE" {
                        if user.texts.remove(text_name).is_some() {
                            return (200, json!({"data": null}));
                        } else {
                            return error(404, "Text not found");
                        }
                    }
                }
                error(404, "Not found")
            });
        }
        error(404, "Not found")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn account_lifecycle() {
        let service = Service::default();
        let account = json!({"username":"alice", "password":"password1"});
        assert_eq!(service.handle("POST", "/users", &account, "").0, 201);
        assert_eq!(service.handle("POST", "/users", &account, "").0, 409);
        let login = service.handle("POST", "/sessions", &account, "").1;
        let old = format!("Bearer {}", login["data"]["token"].as_str().unwrap());
        let login = service.handle("POST", "/sessions", &account, "").1;
        let current = format!("Bearer {}", login["data"]["token"].as_str().unwrap());
        assert_ne!(old, current);
        assert_eq!(service.handle("GET", "/texts", &Value::Null, &old).0, 401);
        assert_eq!(
            service.handle("GET", "/texts", &Value::Null, &current),
            (200, json!({"data":[]}))
        );
        assert_eq!(
            service
                .handle("DELETE", "/sessions/current", &Value::Null, &current)
                .0,
            200
        );
        assert_eq!(
            service.handle("GET", "/texts", &Value::Null, &current).0,
            401
        );
    }

    #[test]
    fn a_poisoned_lock_does_not_brick_the_service() {
        // 回归：`.lock().unwrap()` 会把一次 panic 升级成"此后每个触及状态的请求都
        // 500"——互斥锁中毒不会自愈，进程却还活着。恢复逻辑现在只在 `Service::users()`
        // 一处，这里就绕到 interface 背后毒一把锁，验证服务照样活着。
        let service = Service::default();
        let account = json!({"username":"alice","password":"password1"});
        assert_eq!(service.handle("POST", "/users", &account, "").0, 201);

        let previous_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = service.users.lock().unwrap();
            panic!("simulated handler panic while holding the users lock");
        }));
        std::panic::set_hook(previous_hook);

        assert!(panicked.is_err());
        assert!(service.users.is_poisoned());
        assert_eq!(service.handle("POST", "/sessions", &account, "").0, 200);
        let login = service.handle("POST", "/sessions", &account, "");
        let auth = format!("Bearer {}", login.1["data"]["token"].as_str().unwrap());
        assert_eq!(
            service.handle("GET", "/texts", &Value::Null, &auth),
            (200, json!({"data": []}))
        );
    }

    #[test]
    fn a_token_that_expires_while_waiting_for_the_lock_is_rejected() {
        // 回归：`now` 原来在拿锁之前取，等锁跨过 deadline 时本应失效的令牌仍被接受。
        // 顺序如今由 `with_users` 的签名保证，这条测试是行为上的哨兵：1.2 秒的真实
        // 持锁窗口换不来一次确定性，换成手动时钟就换了不变量，所以照旧真睡。
        let service = std::sync::Arc::new(Service::new(1));
        let account = json!({"username":"alice","password":"password1"});
        assert_eq!(service.handle("POST", "/users", &account, "").0, 201);
        let login = service.handle("POST", "/sessions", &account, "");
        assert_eq!(login.0, 200);
        let auth = format!("Bearer {}", login.1["data"]["token"].as_str().unwrap());

        // 占住锁 1.2 秒（超过 1 秒有效期），请求只能在锁外排队
        let holder_service = service.clone();
        let (locked, is_locked) = std::sync::mpsc::channel();
        let holder = std::thread::spawn(move || {
            let _guard = holder_service.users.lock().unwrap();
            locked.send(()).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(1200));
        });
        is_locked.recv().unwrap();

        let request_service = service.clone();
        let request = std::thread::spawn(move || {
            request_service
                .handle("GET", "/texts", &Value::Null, &auth)
                .0
        });
        assert_eq!(request.join().unwrap(), 401);
        holder.join().unwrap();
    }
}
