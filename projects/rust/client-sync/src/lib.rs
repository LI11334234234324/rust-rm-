use reqwest::{Method, blocking::Client};
use serde_json::Value;
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
}
