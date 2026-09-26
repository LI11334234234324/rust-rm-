use rm_server_sync::{
    Service,
    http::{create_app, with_service},
};
use rocket::http::{ContentType, Header, Status};
use rocket::local::blocking::Client;
use serde_json::{Value, json};

#[test]
fn app_uses_supplied_service() {
    let service = Service::default();
    let account = json!({"username": "alice", "password": "password1"});
    assert_eq!(service.handle("POST", "/users", &account, "").0, 201);
    let configured = Client::tracked(with_service(service)).unwrap();
    let fresh = Client::tracked(create_app()).unwrap();
    for (client, expected) in [(&configured, Status::Ok), (&fresh, Status::Unauthorized)] {
        assert_eq!(
            client
                .post("/sessions")
                .header(ContentType::JSON)
                .body(account.to_string())
                .dispatch()
                .status(),
            expected
        );
    }
}

#[test]
fn http_account_lifecycle() {
    let client = Client::tracked(create_app()).unwrap();
    let ping = client.get("/ping").dispatch();
    assert_eq!(ping.status(), Status::Ok);
    assert_eq!(ping.into_json::<Value>().unwrap(), json!({"data": "pong"}));
    let account = json!({"username": "alice", "password": "password1"}).to_string();
    assert_eq!(
        client
            .post("/users")
            .header(ContentType::JSON)
            .body(&account)
            .dispatch()
            .status(),
        Status::Created
    );
    let login = client
        .post("/sessions")
        .header(ContentType::JSON)
        .body(&account)
        .dispatch()
        .into_json::<Value>()
        .unwrap();
    let authorization = format!("Bearer {}", login["data"]["token"].as_str().unwrap());
    let texts = client
        .get("/texts")
        .header(Header::new("Authorization", authorization.clone()))
        .dispatch();
    assert_eq!(texts.status(), Status::Ok);
    assert_eq!(texts.into_json::<Value>().unwrap(), json!({"data": []}));
    assert_eq!(
        client.get("/texts").dispatch().status(),
        Status::Unauthorized
    );
    assert_eq!(
        client
            .delete("/sessions/current")
            .header(Header::new("Authorization", authorization.clone()))
            .dispatch()
            .status(),
        Status::Ok
    );
    assert_eq!(
        client
            .get("/texts")
            .header(Header::new("Authorization", authorization))
            .dispatch()
            .status(),
        Status::Unauthorized
    );
}

#[test]
fn http_input_and_routing() {
    let client = Client::tracked(create_app()).unwrap();
    for body in [b"not JSON".to_vec(), vec![0xff], b"NaN".to_vec()] {
        assert_eq!(
            client
                .post("/users")
                .header(ContentType::JSON)
                .body(body)
                .dispatch()
                .status(),
            Status::BadRequest
        );
    }
    let exact = format!("{{}}{}", " ".repeat(524_288 - 2));
    assert_eq!(
        client
            .post("/users")
            .header(ContentType::JSON)
            .body(&exact)
            .dispatch()
            .status(),
        Status::BadRequest
    );
    assert_eq!(
        client
            .post("/users")
            .header(ContentType::JSON)
            .body(format!("{exact} "))
            .dispatch()
            .status(),
        Status::PayloadTooLarge
    );
    assert_eq!(
        client
            .post("/users")
            .header(ContentType::JSON)
            .body(r#"{"username":true,"password":"password1"}"#)
            .dispatch()
            .status(),
        Status::BadRequest
    );
    assert_eq!(client.get("/missing").dispatch().status(), Status::NotFound);
    assert_eq!(
        client.get("/echo").dispatch().status(),
        Status::MethodNotAllowed
    );
    assert_eq!(
        client.patch("/ping").dispatch().status(),
        Status::MethodNotAllowed
    );
}

#[test]
fn http_echo() {
    let client = Client::tracked(create_app()).unwrap();
    let res = client
        .post("/echo")
        .header(ContentType::JSON)
        .body(json!({"text": "Hello\nWorld! 🚀"}).to_string())
        .dispatch();
    assert_eq!(res.status(), Status::Ok);
    assert_eq!(
        res.into_json::<Value>().unwrap(),
        json!({"data": "Hello\nWorld! 🚀"})
    );

    let res = client
        .post("/echo")
        .header(ContentType::JSON)
        .body(r#"{"text":""}"#)
        .dispatch();
    assert_eq!(res.status(), Status::Ok);
    assert_eq!(res.into_json::<Value>().unwrap(), json!({"data": ""}));

    for body in [
        r#"{}"#,
        r#"{"text": 123}"#,
        r#"{"text": true}"#,
        r#"{"text": "hi", "extra": "field"}"#,
    ] {
        let res = client
            .post("/echo")
            .header(ContentType::JSON)
            .body(body)
            .dispatch();
        assert_eq!(res.status(), Status::BadRequest);
    }

    let large = "a".repeat(65_537);
    let res = client
        .post("/echo")
        .header(ContentType::JSON)
        .body(json!({"text": large}).to_string())
        .dispatch();
    assert_eq!(res.status(), Status::PayloadTooLarge);

    let exact = "a".repeat(65_536);
    let res = client
        .post("/echo")
        .header(ContentType::JSON)
        .body(json!({"text": exact}).to_string())
        .dispatch();
    assert_eq!(res.status(), Status::Ok);
}

#[test]
fn http_text_lifecycle_and_isolation() {
    let client = Client::tracked(create_app()).unwrap();
    let alice_account = json!({"username": "alice", "password": "password1"}).to_string();
    assert_eq!(
        client
            .post("/users")
            .header(ContentType::JSON)
            .body(&alice_account)
            .dispatch()
            .status(),
        Status::Created
    );
    let login_alice = client
        .post("/sessions")
        .header(ContentType::JSON)
        .body(&alice_account)
        .dispatch()
        .into_json::<Value>()
        .unwrap();
    let alice_token = format!("Bearer {}", login_alice["data"]["token"].as_str().unwrap());

    let bob_account = json!({"username": "bob", "password": "password1"}).to_string();
    assert_eq!(
        client
            .post("/users")
            .header(ContentType::JSON)
            .body(&bob_account)
            .dispatch()
            .status(),
        Status::Created
    );
    let login_bob = client
        .post("/sessions")
        .header(ContentType::JSON)
        .body(&bob_account)
        .dispatch()
        .into_json::<Value>()
        .unwrap();
    let bob_token = format!("Bearer {}", login_bob["data"]["token"].as_str().unwrap());

    // 1. Alice creates text "note"
    let res = client
        .put("/texts/note")
        .header(Header::new("Authorization", alice_token.clone()))
        .header(ContentType::JSON)
        .body(json!({"text": "alice note"}).to_string())
        .dispatch();
    assert_eq!(res.status(), Status::Ok);
    assert_eq!(res.into_json::<Value>().unwrap(), json!({"data": null}));

    // 2. Alice reads text "note"
    let res = client
        .get("/texts/note")
        .header(Header::new("Authorization", alice_token.clone()))
        .dispatch();
    assert_eq!(res.status(), Status::Ok);
    assert_eq!(
        res.into_json::<Value>().unwrap(),
        json!({"data": "alice note"})
    );

    // 3. Bob tries to read "note" -> 404 (user isolation)
    let res = client
        .get("/texts/note")
        .header(Header::new("Authorization", bob_token.clone()))
        .dispatch();
    assert_eq!(res.status(), Status::NotFound);

    // 4. Bob creates his own "note"
    let res = client
        .put("/texts/note")
        .header(Header::new("Authorization", bob_token.clone()))
        .header(ContentType::JSON)
        .body(json!({"text": "bob note"}).to_string())
        .dispatch();
    assert_eq!(res.status(), Status::Ok);

    // 5. Check isolation: Alice and Bob have different contents
    let res_alice = client
        .get("/texts/note")
        .header(Header::new("Authorization", alice_token.clone()))
        .dispatch();
    assert_eq!(
        res_alice.into_json::<Value>().unwrap(),
        json!({"data": "alice note"})
    );

    let res_bob = client
        .get("/texts/note")
        .header(Header::new("Authorization", bob_token.clone()))
        .dispatch();
    assert_eq!(
        res_bob.into_json::<Value>().unwrap(),
        json!({"data": "bob note"})
    );

    // 6. Alice deletes "note"
    let res = client
        .delete("/texts/note")
        .header(Header::new("Authorization", alice_token.clone()))
        .dispatch();
    assert_eq!(res.status(), Status::Ok);

    // 7. Alice gets 404 on "note", but Bob's "note" still exists
    assert_eq!(
        client
            .get("/texts/note")
            .header(Header::new("Authorization", alice_token.clone()))
            .dispatch()
            .status(),
        Status::NotFound
    );
    assert_eq!(
        client
            .get("/texts/note")
            .header(Header::new("Authorization", bob_token.clone()))
            .dispatch()
            .status(),
        Status::Ok
    );

    // 8. Invalid name -> 400
    assert_eq!(
        client
            .get("/texts/invalid!name")
            .header(Header::new("Authorization", alice_token.clone()))
            .dispatch()
            .status(),
        Status::BadRequest
    );
}

#[test]
fn unimplemented_routes_are_absent() {
    use rocket::http::Method;
    let client = Client::tracked(create_app()).unwrap();
    assert_eq!(
        client.req(Method::Delete, "/users/me").dispatch().status(),
        Status::Unauthorized
    );
    assert_eq!(
        client.get("/unknown_route").dispatch().status(),
        Status::NotFound
    );
    for path in [
        "/ping",
        "/users",
        "/sessions",
        "/sessions/current",
        "/texts",
        "/users/me",
    ] {
        assert_eq!(
            client.patch(path).dispatch().status(),
            Status::MethodNotAllowed
        );
    }
}

#[test]
fn http_user_deletion_lifecycle_and_cleanup() {
    let client = Client::tracked(create_app()).unwrap();
    let alice = json!({"username": "alice", "password": "password1"}).to_string();
    let bob = json!({"username": "bob", "password": "password1"}).to_string();

    // 1. 注册并登录 Alice 和 Bob
    assert_eq!(
        client
            .post("/users")
            .header(ContentType::JSON)
            .body(&alice)
            .dispatch()
            .status(),
        Status::Created
    );
    assert_eq!(
        client
            .post("/users")
            .header(ContentType::JSON)
            .body(&bob)
            .dispatch()
            .status(),
        Status::Created
    );

    let alice_login = client
        .post("/sessions")
        .header(ContentType::JSON)
        .body(&alice)
        .dispatch()
        .into_json::<Value>()
        .unwrap();
    let alice_token = format!("Bearer {}", alice_login["data"]["token"].as_str().unwrap());

    let bob_login = client
        .post("/sessions")
        .header(ContentType::JSON)
        .body(&bob)
        .dispatch()
        .into_json::<Value>()
        .unwrap();
    let bob_token = format!("Bearer {}", bob_login["data"]["token"].as_str().unwrap());

    // 2. 两人各自创建同名笔记 "shared_name"
    let res = client
        .put("/texts/shared_name")
        .header(Header::new("Authorization", alice_token.clone()))
        .header(ContentType::JSON)
        .body(json!({"text": "alice text"}).to_string())
        .dispatch();
    assert_eq!(res.status(), Status::Ok);

    let res = client
        .put("/texts/shared_name")
        .header(Header::new("Authorization", bob_token.clone()))
        .header(ContentType::JSON)
        .body(json!({"text": "bob text"}).to_string())
        .dispatch();
    assert_eq!(res.status(), Status::Ok);

    // 3. Alice 注销自己的账号
    let del_res = client
        .delete("/users/me")
        .header(Header::new("Authorization", alice_token.clone()))
        .dispatch();
    assert_eq!(del_res.status(), Status::Ok);
    assert_eq!(del_res.into_json::<Value>().unwrap(), json!({"data": null}));

    // 4. Alice 的旧令牌立即彻底失效（在所有受保护端点均报 401）
    assert_eq!(
        client
            .get("/texts")
            .header(Header::new("Authorization", alice_token.clone()))
            .dispatch()
            .status(),
        Status::Unauthorized
    );
    assert_eq!(
        client
            .get("/texts/shared_name")
            .header(Header::new("Authorization", alice_token.clone()))
            .dispatch()
            .status(),
        Status::Unauthorized
    );
    assert_eq!(
        client
            .delete("/users/me")
            .header(Header::new("Authorization", alice_token.clone()))
            .dispatch()
            .status(),
        Status::Unauthorized
    );

    // 5. 隔壁租户 Bob 毫发无损
    let bob_res = client
        .get("/texts/shared_name")
        .header(Header::new("Authorization", bob_token.clone()))
        .dispatch();
    assert_eq!(bob_res.status(), Status::Ok);
    assert_eq!(
        bob_res.into_json::<Value>().unwrap(),
        json!({"data": "bob text"})
    );

    // 6. Alice 可以同名重新注册
    let reg_again = client
        .post("/users")
        .header(ContentType::JSON)
        .body(&alice)
        .dispatch();
    assert_eq!(reg_again.status(), Status::Created);

    let new_login = client
        .post("/sessions")
        .header(ContentType::JSON)
        .body(&alice)
        .dispatch()
        .into_json::<Value>()
        .unwrap();
    let new_alice_token = format!("Bearer {}", new_login["data"]["token"].as_str().unwrap());

    // 7. 新注册的 Alice 数据白纸一张（没有历史残留）
    let new_texts = client
        .get("/texts")
        .header(Header::new("Authorization", new_alice_token.clone()))
        .dispatch();
    assert_eq!(new_texts.status(), Status::Ok);
    assert_eq!(new_texts.into_json::<Value>().unwrap(), json!({"data": []}));

    assert_eq!(
        client
            .get("/texts/shared_name")
            .header(Header::new("Authorization", new_alice_token))
            .dispatch()
            .status(),
        Status::NotFound
    );
}

#[test]
fn http_token_ttl_and_expiry() {
    let service = Service::new(1);
    let client = Client::tracked(with_service(service)).unwrap();
    let account = json!({"username":"alice", "password":"password1"}).to_string();
    assert_eq!(
        client
            .post("/users")
            .header(ContentType::JSON)
            .body(&account)
            .dispatch()
            .status(),
        Status::Created
    );
    let login = client
        .post("/sessions")
        .header(ContentType::JSON)
        .body(&account)
        .dispatch()
        .into_json::<Value>()
        .unwrap();
    assert_eq!(login["data"]["expires_in"], 1);
    let authorization = format!("Bearer {}", login["data"]["token"].as_str().unwrap());
    let res = client
        .get("/texts")
        .header(Header::new("Authorization", authorization.clone()))
        .dispatch();
    assert_eq!(res.status(), Status::Ok);
    let put_res = client
        .put("/texts/secret")
        .header(Header::new("Authorization", authorization.clone()))
        .header(ContentType::JSON)
        .body(json!({"text": "top secret"}).to_string())
        .dispatch();
    assert_eq!(put_res.status(), Status::Ok);

    std::thread::sleep(std::time::Duration::from_millis(1100));
    assert_eq!(
        client
            .get("/texts")
            .header(Header::new("Authorization", authorization.clone()))
            .dispatch()
            .status(),
        Status::Unauthorized
    );
    assert_eq!(
        client
            .get("/texts/secret")
            .header(Header::new("Authorization", authorization.clone()))
            .dispatch()
            .status(),
        Status::Unauthorized
    );
    assert_eq!(
        client
            .delete("/users/me")
            .header(Header::new("Authorization", authorization.clone()))
            .dispatch()
            .status(),
        Status::Unauthorized
    );
    let new_login = client
        .post("/sessions")
        .header(ContentType::JSON)
        .body(&account)
        .dispatch()
        .into_json::<Value>()
        .unwrap();
    let new_auth = format!("Bearer {}", new_login["data"]["token"].as_str().unwrap());

    // 拿着新令牌读取之前存的 secret，数据完好无损！
    let read_res = client
        .get("/texts/secret")
        .header(Header::new("Authorization", new_auth))
        .dispatch();
    assert_eq!(read_res.status(), Status::Ok);
    assert_eq!(
        read_res.into_json::<Value>().unwrap(),
        json!({"data": "top secret"})
    );
}
