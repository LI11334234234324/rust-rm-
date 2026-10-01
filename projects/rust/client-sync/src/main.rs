use clap::Parser;
use reqwest::blocking::Client;
use rm_client_sync::{Command, ResponseEffect};
use std::io::{self, Write};
use std::time::Duration;

#[derive(Parser)]
struct Args {
    #[arg(long, default_value = "http://127.0.0.1:7878", value_parser = reqwest::Url::parse)]
    url: reqwest::Url,
}

fn input(prompt: &str) -> io::Result<String> {
    print!("{prompt}");
    io::stdout().flush()?;
    let mut line = String::new();
    if io::stdin().read_line(&mut line)? == 0 {
        return Err(io::ErrorKind::UnexpectedEof.into());
    }
    Ok(line.trim_end_matches(['\r', '\n']).to_owned())
}

fn prompt_text() -> io::Result<String> {
    println!("Enter text (single '.' on a line to end):");
    let stdin = io::stdin();
    let mut reader = stdin.lock();
    rm_client_sync::read_text(&mut reader)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let client = Client::builder()
        .timeout(Duration::from_secs(12))
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let mut token = String::new();
    loop {
        let command = match input(
            "ping / register / login / logout / list / echo / delete-user / put / get / delete / q > ",
        ) {
            Ok(command) => command,
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => break,
            Err(error) => return Err(error.into()),
        };

        let Some(command) = Command::parse(&command) else {
            println!("Unknown command.");
            continue;
        };

        let spec = match command {
            Command::Quit => break,
            Command::Ping => rm_client_sync::build_ping_request(),
            Command::List => rm_client_sync::build_list_request(),
            Command::Logout => rm_client_sync::build_logout_request(),
            Command::Register => {
                let username = input("username: ")?;
                let password = read_password()?;
                rm_client_sync::build_register_request(&username, &password)
            }
            Command::Login => {
                let username = input("username: ")?;
                let password = read_password()?;
                rm_client_sync::build_login_request(&username, &password)
            }
            Command::Echo => {
                let text = prompt_text()?;
                rm_client_sync::build_echo_request(&text)
            }
            Command::Put => {
                let name = input("name:")?;
                let text = prompt_text()?;
                rm_client_sync::build_put_request(&name, &text)
            }
            Command::Get => {
                let name = input("name:")?;
                rm_client_sync::build_get_request(&name)
            }
            Command::Delete => {
                let name = input("name:")?;
                rm_client_sync::build_delete_request(&name)
            }
            Command::DeleteUser => rm_client_sync::build_delete_user_request(),
        };

        let result = rm_client_sync::exchange(
            &client,
            args.url.as_str(),
            spec.method,
            &spec.path,
            &token,
            spec.body.as_ref(),
        );

        match result {
            Ok((status, value)) => {
                println!("{status} {value}");
                match rm_client_sync::handle_response(command, status, &value) {
                    ResponseEffect::StoreToken(next) => token = next,
                    ResponseEffect::ClearToken => token.clear(),
                    ResponseEffect::RequireRelogin => {
                        println!("Please log in again.");
                        token.clear();
                    }
                    ResponseEffect::Nothing => {}
                }
            }
            Err(error) => eprintln!("Request failed: {}", describe(&error)),
        }
    }
    Ok(())
}

/// 读密码：终端上走 `rpassword`（不回显）；stdin 被重定向时改从 stdin 读一行。
/// 不这样分流的话，`... | rm-client-sync` 会在 `CONIN$` 上静默挂住，登录没法脚本化。
fn read_password() -> io::Result<String> {
    use std::io::IsTerminal;
    if io::stdin().is_terminal() {
        return rpassword::prompt_password("password: ").map_err(io::Error::other);
    }
    print!("password: ");
    io::stdout().flush()?;
    let mut line = String::new();
    if io::stdin().read_line(&mut line)? == 0 {
        return Err(io::ErrorKind::UnexpectedEof.into());
    }
    Ok(line.trim_end_matches(['\r', '\n']).to_owned())
}

/// reqwest 的 `Display` 只有顶层摘要（"error sending request for url (...)"），
/// 真正的原因（连接被拒 / 超时 / DNS）在 `source` 链的更深处，这里把整条链接上。
fn describe(error: &reqwest::Error) -> String {
    let mut causes = Vec::new();
    let mut current = std::error::Error::source(error);
    while let Some(cause) = current {
        causes.push(cause.to_string());
        current = cause.source();
    }
    if causes.is_empty() {
        error.to_string()
    } else {
        format!("{error} ({})", causes.join(" -> "))
    }
}
