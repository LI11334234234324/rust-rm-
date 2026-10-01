use reqwest::{Method, blocking::Client};
use serde_json::{Value, json};
use std::io::BufRead;

/// 读取一段多行文本，直到单独一行的 `.` 为止。
///
/// 输入约定：
/// - 单独一行的 `.` 结束输入，不计入正文；一上来就输入 `.` 表示空文本。
/// - 正文最后多打一个空行表示结尾换行：`hello`、空行、`.` 得到 `"hello\n"`；
///   整段都是空行时空行数即换行数，空行、`.` 得到 `"\n"`。
/// - 以 `..` 开头的行脱去一个点，所以输入 `..` 得到 `"."`、输入 `...x` 得到
///   `"..x"`；其他行原样保留，`.env` 就是 `.env`。
/// - 输入结束（EOF）同样结束输入。
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

        // 以 ".." 开头的行只去掉第一个点，字面量 "." 因此写成 ".."
        let content = if trimmed.starts_with("..") {
            &trimmed[1..]
        } else {
            trimmed
        };
        lines.push(content.to_string());
    }

    if lines.iter().all(|s| s.is_empty()) {
        Ok("\n".repeat(lines.len()))
    } else {
        Ok(lines.join("\n"))
    }
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
        path: format!("/texts/{}", encode_path_segment(name)),
        body: Some(json!({
            "text": text
        })),
    }
}

pub fn build_get_request(name: &str) -> RequestSpec {
    RequestSpec {
        method: Method::GET,
        path: format!("/texts/{}", encode_path_segment(name)),
        body: None,
    }
}

pub fn build_delete_request(name: &str) -> RequestSpec {
    RequestSpec {
        method: Method::DELETE,
        path: format!("/texts/{}", encode_path_segment(name)),
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
    if status == 401 && command != Command::Login {
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

/// 把一个文本名编码成 URL 路径段。
///
/// 名字是直接拼进请求路径的，不编码就会改变请求的目标：`a?b` 被当成"路径 `/texts/a`
/// 加查询串 `b`"，`a#b` 的 `#b` 被当成片段丢掉，`a\b` 被 URL 规范归一成 `a/b`，
/// `%41` 到服务端会被当成另一个名字。于是 `put a#b` 会打印 200，写进去的却是 `a`
/// ——静默写到另一个文本上。
///
/// 规则：只有协议允许的名字字符（`A-Z a-z 0-9 _ -`）原样保留，其余字节一律百分号
/// 编码。合法名字因此完全不受影响；非法名字会原样送到服务端，由服务端判 400
/// （客户端不替服务端做字段校验）。
///
/// 残留一处：名字恰好是 `.` 或 `..` 时，URL 规范把它们的任何写法（含 `%2E`）都当
/// 点段消除，请求最终落到 `/texts/` 或 `/` 上（服务端 404）。两者都不是合法名字，
/// 也不会碰到别的文本，所以这里只记一笔，不做本地特判。
fn encode_path_segment(name: &str) -> String {
    let mut encoded = String::with_capacity(name.len());
    for byte in name.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
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
    fn reads_single_newline() {
        let mut input = Cursor::new("\n.\n");
        assert_eq!(read_text(&mut input).unwrap(), "\n");
    }

    #[test]
    fn reads_two_newlines() {
        let mut input = Cursor::new("\n\n.\n");
        assert_eq!(read_text(&mut input).unwrap(), "\n\n");
    }

    #[test]
    fn reads_dot_as_body_line() {
        // 输入 ".." 应该被脱壳为 "." 作为正文
        let mut input = Cursor::new("first\n..\nlast\n.\n");
        assert_eq!(read_text(&mut input).unwrap(), "first\n.\nlast");
    }

    #[test]
    fn reads_single_escaped_dot_line() {
        let mut input = Cursor::new("..\n.\n");
        assert_eq!(read_text(&mut input).unwrap(), ".");
    }

    #[test]
    fn preserves_dot_prefixed_regular_lines() {
        // 只有以 ".." 开头的行才脱壳，普通隐藏文件名原样保留
        let mut input = Cursor::new(".env\n.gitignore\n.\n");
        assert_eq!(read_text(&mut input).unwrap(), ".env\n.gitignore");
    }

    #[test]
    fn unescapes_doubled_dot_prefixes() {
        // 字面量 ".." 仍然可以表达：多打一个点
        let mut input = Cursor::new("..gitignore\n...\n.\n");
        assert_eq!(read_text(&mut input).unwrap(), ".gitignore\n..");
    }

    #[test]
    fn preserves_trailing_newline() {
        // 在 "." 前面敲空行，应该保留末尾换行
        let mut input = Cursor::new("hello\n\n.\n");
        assert_eq!(read_text(&mut input).unwrap(), "hello\n");
    }

    #[test]
    fn reads_multiline_without_trailing_newline() {
        let mut input = Cursor::new("hello\nworld\n.\n");
        assert_eq!(read_text(&mut input).unwrap(), "hello\nworld");
    }

    #[test]
    fn reads_multiline_with_trailing_newline() {
        let mut input = Cursor::new("hello\nworld\n\n.\n");
        assert_eq!(read_text(&mut input).unwrap(), "hello\nworld\n");
    }

    #[test]
    fn reads_until_eof_without_dot() {
        let mut input = Cursor::new("line1\nline2");
        assert_eq!(read_text(&mut input).unwrap(), "line1\nline2");
    }

    #[test]
    fn handles_crlf_line_endings() {
        let mut input = Cursor::new("hello\r\n..\r\nworld\r\n.\r\n");
        assert_eq!(read_text(&mut input).unwrap(), "hello\n.\nworld");
    }

    #[test]
    fn handles_crlf_empty_and_trailing_newlines() {
        let mut input = Cursor::new("\r\n.\r\n");
        assert_eq!(read_text(&mut input).unwrap(), "\n");

        let mut input2 = Cursor::new("hello\r\n\r\n.\r\n");
        assert_eq!(read_text(&mut input2).unwrap(), "hello\n");
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

    #[test]
    fn a_failed_login_does_not_discard_a_valid_token() {
        // 回归：401 一律判成 RequireRelogin，于是"用错密码再登录一次"会把当前
        // 有效的令牌清掉——服务端只对**成功**的登录替换令牌，旧令牌仍然有效。
        assert_eq!(
            handle_response(
                Command::Login,
                401,
                &json!({"message": "Invalid username or password"})
            ),
            ResponseEffect::Nothing
        );
        // 受保护请求上的 401 仍然要求重新登录
        assert_eq!(
            handle_response(Command::List, 401, &Value::Null),
            ResponseEffect::RequireRelogin
        );
    }

    #[test]
    fn text_names_are_encoded_so_they_cannot_retarget_another_text() {
        // 回归：URL 结构字符曾原样进入路径，`put a?b` 会写到 `a` 上（200 却改了别的文本）
        assert_eq!(build_put_request("a?b", "x").path, "/texts/a%3Fb");
        assert_eq!(build_get_request("a#b").path, "/texts/a%23b");
        assert_eq!(build_delete_request("a\\b").path, "/texts/a%5Cb");
        assert_eq!(build_get_request("a/b").path, "/texts/a%2Fb");
        assert_eq!(build_put_request("a b", "x").path, "/texts/a%20b");
        // 百分号本身也要编码，否则服务端的解码结果会和你输入的不一致
        assert_eq!(build_get_request("%41").path, "/texts/%2541");
        // `..` 在 builder 这一层是编码好的；但 URL 规范会把点段的任何写法消除，
        // 所以真正发出的路径是 `/`（见 `encode_path_segment` 的"残留一处"）
        assert_eq!(build_delete_request("..").path, "/texts/%2E%2E");
        // 非 ASCII 名字按字节编码
        assert_eq!(build_get_request("名").path, "/texts/%E5%90%8D");
        // 协议允许的名字不受影响
        for name in ["note", "note-1_2", "ABC"] {
            assert_eq!(build_put_request(name, "x").path, format!("/texts/{name}"));
        }
    }
}
