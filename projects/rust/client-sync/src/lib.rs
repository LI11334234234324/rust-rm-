use reqwest::{Method, blocking::Client};
use serde_json::{Value, json};
use std::io::BufRead;

pub fn read_text<R: BufRead>(reader: &mut R) -> std::io::Result<String> {
    let mut lines = Vec::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);

        if trimmed == "." {
            break;
        }

        let content = if let Some(stripped) = trimmed.strip_prefix(".") {
            stripped
        } else {
            trimmed
        };
        lines.push(content.to_string());
    }
    Ok(lines.join("\n"))
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct RequestSpec {
    pub method: Method,
    pub path: String,
    pub body: Option<Value>,
}

pub fn build_ping_request() -> RequestSpec {
    RequestSpec {
        method: Method::GET,
        path: "/ping".into(),
        body: None,
    }
}

pub fn build_register_request(username: &str, password: &str) -> RequestSpec {
    RequestSpec {
        method: Method::POST,
        path: "/users".into(),
        body: Some(json!({
            "username": username,
            "password": password
        })),
    }
}

pub fn build_login_request(username: &str, password: &str) -> RequestSpec {
    RequestSpec {
        method: Method::POST,
        path: "/sessions".into(),
        body: Some(json!({
            "username": username,
            "password": password
        })),
    }
}

pub fn build_logout_request() -> RequestSpec {
    RequestSpec {
        method: Method::DELETE,
        path: "/sessions/current".into(),
        body: None,
    }
}

pub fn build_list_request() -> RequestSpec {
    RequestSpec {
        method: Method::GET,
        path: "/texts".into(),
        body: None,
    }
}

pub fn build_echo_request(text: &str) -> RequestSpec {
    RequestSpec {
        method: Method::POST,
        path: "/echo".into(),
        body: Some(json!({
            "text": text
        })),
    }
}

pub fn build_put_request(name: &str, text: &str) -> RequestSpec {
    RequestSpec {
        method: Method::PUT,
        path: format!("/texts/{name}"),
        body: Some(json!({
            "text": text
        })),
    }
}

pub fn build_get_request(name: &str) -> RequestSpec {
    RequestSpec {
        method: Method::GET,
        path: format!("/texts/{name}"),
        body: None,
    }
}

pub fn build_delete_request(name: &str) -> RequestSpec {
    RequestSpec {
        method: Method::DELETE,
        path: format!("/texts/{name}"),
        body: None,
    }
}

pub fn build_delete_user_request() -> RequestSpec {
    RequestSpec {
        method: Method::DELETE,
        path: "/users/me".into(),
        body: None,
    }
}

/// 交互式命令。解析集中在 `Command::parse`，命令循环与响应处理共用同一个
/// 枚举，避免两处各写一遍字符串字面量。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Ping,
    Register,
    Login,
    Logout,
    List,
    Echo,
    Put,
    Get,
    Delete,
    DeleteUser,
    Quit,
}

impl Command {
    /// 解析用户输入的命令；无法识别时返回 `None`。
    pub fn parse(input: &str) -> Option<Self> {
        let command = match input {
            "ping" => Self::Ping,
            "register" => Self::Register,
            "login" => Self::Login,
            "logout" => Self::Logout,
            "list" => Self::List,
            "echo" => Self::Echo,
            "put" => Self::Put,
            "get" => Self::Get,
            "delete" => Self::Delete,
            "delete-user" => Self::DeleteUser,
            "q" => Self::Quit,
            _ => return None,
        };
        Some(command)
    }
}

/// 一次响应对本地令牌的影响。
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum ResponseEffect {
    /// 令牌与本地状态都不需要变化。
    Nothing,
    /// 登录成功，保存新令牌。
    StoreToken(String),
    /// 退出或注销成功，丢弃本地令牌。
    ClearToken,
    /// 身份无效，提示重新登录并丢弃本地令牌。
    RequireRelogin,
}

pub fn handle_response(command: Command, status: u16, value: &Value) -> ResponseEffect {
    if status == 401 {
        return ResponseEffect::RequireRelogin;
    }
    if status == 200 && matches!(command, Command::Logout | Command::DeleteUser) {
        return ResponseEffect::ClearToken;
    }
    if command == Command::Login
        && status == 200
        && let Some(token) = value["data"]["token"].as_str()
    {
        return ResponseEffect::StoreToken(token.to_string());
    }
    ResponseEffect::Nothing
}

/// Preserve HTTP status even when the error body is not JSON.
pub fn exchange(
    client: &Client,
    url: &str,
    method: Method,
    path: &str,
    token: &str,
    body: Option<&Value>,
) -> Result<(u16, Value), reqwest::Error> {
    let mut request = client.request(method, format!("{}{path}", url.trim_end_matches('/')));
    if !token.is_empty() {
        request = request.bearer_auth(token);
    }
    if let Some(body) = body {
        request = request.json(body);
    }
    let response = request.send()?;
    let status = response.status().as_u16();
    let text = response.text()?;
    let value =
        serde_json::from_str(&text).unwrap_or_else(|_| serde_json::json!({"message": text}));
    Ok((status, value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn reads_empty_text() {
        let mut input = Cursor::new(".\n");
        assert_eq!(read_text(&mut input).unwrap(), "");
    }

    #[test]
    fn reads_dot_as_body_line() {
        // 输入 ".." 应该被脱壳为 "." 作为正文
        let mut input = Cursor::new("first\n..\nlast\n.\n");
        assert_eq!(read_text(&mut input).unwrap(), "first\n.\nlast");
    }

    #[test]
    fn preserves_trailing_newline() {
        // 在 "." 前面敲空行，应该保留末尾换行
        let mut input = Cursor::new("hello\n\n.\n");
        assert_eq!(read_text(&mut input).unwrap(), "hello\n");
    }

    #[test]
    fn request_builders_construct_expected_specs() {
        assert_eq!(
            build_ping_request(),
            RequestSpec {
                method: Method::GET,
                path: "/ping".into(),
                body: None,
            }
        );
        assert_eq!(
            build_register_request("alice", "pass1234"),
            RequestSpec {
                method: Method::POST,
                path: "/users".into(),
                body: Some(json!({"username": "alice", "password": "pass1234"})),
            }
        );
        assert_eq!(
            build_login_request("alice", "pass1234"),
            RequestSpec {
                method: Method::POST,
                path: "/sessions".into(),
                body: Some(json!({"username": "alice", "password": "pass1234"})),
            }
        );
        assert_eq!(
            build_logout_request(),
            RequestSpec {
                method: Method::DELETE,
                path: "/sessions/current".into(),
                body: None,
            }
        );
        assert_eq!(
            build_list_request(),
            RequestSpec {
                method: Method::GET,
                path: "/texts".into(),
                body: None,
            }
        );
        assert_eq!(
            build_echo_request("hello"),
            RequestSpec {
                method: Method::POST,
                path: "/echo".into(),
                body: Some(json!({"text": "hello"})),
            }
        );
        assert_eq!(
            build_put_request("note", "content"),
            RequestSpec {
                method: Method::PUT,
                path: "/texts/note".into(),
                body: Some(json!({"text": "content"})),
            }
        );
        assert_eq!(
            build_get_request("note"),
            RequestSpec {
                method: Method::GET,
                path: "/texts/note".into(),
                body: None,
            }
        );
        assert_eq!(
            build_delete_request("note"),
            RequestSpec {
                method: Method::DELETE,
                path: "/texts/note".into(),
                body: None,
            }
        );
        assert_eq!(
            build_delete_user_request(),
            RequestSpec {
                method: Method::DELETE,
                path: "/users/me".into(),
                body: None,
            }
        );
    }

    #[test]
    fn command_parse_maps_known_commands_and_rejects_others() {
        assert_eq!(Command::parse("ping"), Some(Command::Ping));
        assert_eq!(Command::parse("delete-user"), Some(Command::DeleteUser));
        assert_eq!(Command::parse("q"), Some(Command::Quit));
        // 命令区分大小写，也不接受别名
        assert_eq!(Command::parse("Ping"), None);
        assert_eq!(Command::parse("delete_user"), None);
        assert_eq!(Command::parse(""), None);
    }

    #[test]
    fn handle_response_401_requires_relogin() {
        assert_eq!(
            handle_response(Command::List, 401, &json!({"message": "Unauthorized"})),
            ResponseEffect::RequireRelogin
        );
        // 即使响应体缺失或不是预期结构，也只看状态码
        assert_eq!(
            handle_response(Command::Get, 401, &Value::Null),
            ResponseEffect::RequireRelogin
        );
    }

    #[test]
    fn handle_response_login_success_stores_token() {
        assert_eq!(
            handle_response(
                Command::Login,
                200,
                &json!({"data": {"token": "secret_token_123", "expires_in": 300}})
            ),
            ResponseEffect::StoreToken("secret_token_123".into())
        );
    }

    #[test]
    fn handle_response_logout_and_delete_user_clear_token() {
        assert_eq!(
            handle_response(Command::Logout, 200, &json!({"data": null})),
            ResponseEffect::ClearToken
        );
        assert_eq!(
            handle_response(Command::DeleteUser, 200, &json!({"data": null})),
            ResponseEffect::ClearToken
        );
    }

    #[test]
    fn handle_response_other_results_do_not_modify_token() {
        assert_eq!(
            handle_response(Command::List, 200, &json!({"data": ["note1"]})),
            ResponseEffect::Nothing
        );
        // 登录成功但响应体里没有令牌时，不能清空已有令牌
        assert_eq!(
            handle_response(Command::Login, 200, &json!({"data": {}})),
            ResponseEffect::Nothing
        );
        assert_eq!(
            handle_response(Command::Put, 413, &json!({"message": "too large"})),
            ResponseEffect::Nothing
        );
    }
}
