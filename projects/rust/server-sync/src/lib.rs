pub mod http;
mod infrastructure;

use pbkdf2::pbkdf2_hmac;
use rand::{RngCore, rngs::OsRng};
use serde_json::{Value, json};
use sha2::Sha256;
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use subtle::ConstantTimeEq;

pub const ROUTES: &[(&str, &str)] = &[
    ("GET", "/ping"),
    ("POST", "/users"),
    ("POST", "/sessions"),
    ("DELETE", "/sessions/current"),
    ("GET", "/texts"),
    ("POST", "/echo"),
    ("DELETE", "/users/me"),
];

pub fn route_error(method: &str, path: &str) -> Option<u16> {
    if let Some(name) = path.strip_prefix("/texts/") {
        if name.is_empty() || name.contains('/') {
            return Some(404);
        }
        return match method {
            "GET" | "PUT" | "DELETE" => None,
            _ => Some(405),
        };
    }
    match ROUTES.iter().find(|(_, route)| *route == path) {
        None => Some(404),
        Some((allowed, _)) if *allowed != method => Some(405),
        Some(_) => None,
    }
}

#[derive(Clone)]
pub struct Session {
    pub token: String,
    pub deadline: Instant,
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
    pub fn is_valid(&self, token: &str, now: Instant) -> bool {
        !token.is_empty() && self.token == token && now < self.deadline
    }
}

pub struct User {
    pub salt: [u8; 16],
    pub digest: [u8; 32],
    pub session: Option<Session>,
    pub texts: BTreeMap<String, String>,
}

impl User {
    pub fn new(salt: [u8; 16], digest: [u8; 32]) -> Self {
        Self {
            salt,
            digest,
            session: None,
            texts: BTreeMap::new(),
        }
    }

    pub fn start_session(&mut self, token: String, ttl: Duration) {
        self.session = Some(Session {
            token,
            deadline: Instant::now() + ttl,
        });
    }

    pub fn clear_session(&mut self) {
        self.session = None;
    }

    pub fn has_valid_token(&self, token: &str, now: Instant) -> bool {
        self.session
            .as_ref()
            .is_some_and(|s| s.is_valid(token, now))
    }
}

pub struct Service {
    pub users: Mutex<BTreeMap<String, User>>,
    pub token_ttl_seconds: u64,
}

impl Service {
    pub fn new(token_ttl_seconds: u64) -> Self {
        assert!(token_ttl_seconds > 0, "token_ttl_seconds must be positive");
        Self {
            users: Mutex::new(BTreeMap::new()),
            token_ttl_seconds,
        }
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
        let users = self.users.lock().unwrap();
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
        let mut users = self.users.lock().unwrap();
        let Some(user) = users.get_mut(name) else {
            return error(401, "Invalid username or password");
        };
        if user.salt != credentials.salt || !bool::from(digest.ct_eq(&credentials.digest)) {
            return error(401, "Invalid username or password");
        }
        let token = new_token();
        user.start_session(token.clone(), Duration::from_secs(self.token_ttl_seconds));
        (
            200,
            json!({"data": {"token": token, "expires_in": self.token_ttl_seconds}}),
        )
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
        if let Some(status) = route_error(method, path) {
            return error(
                status,
                if status == 404 {
                    "Not found"
                } else {
                    "Method not allowed"
                },
            );
        }
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
                let mut users = self.users.lock().unwrap();
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
        let protected = matches!(path, "/texts" | "/sessions/current" | "/users/me")
            || path.starts_with("/texts/");
        if protected {
            let token = authorization.strip_prefix("Bearer ").unwrap_or("");
            let now = Instant::now();
            let mut users = self.users.lock().unwrap();
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
}
