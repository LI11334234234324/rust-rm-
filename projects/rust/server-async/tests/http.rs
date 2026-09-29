use rm_server_async::http::create_app;
use rocket::http::{ContentType, Header, Status};
use rocket::local::blocking::Client;
use serde_json::{Value, json};

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
fn unimplemented_routes_are_absent() {
    let client = Client::tracked(create_app()).unwrap();
    assert_eq!(
        client.delete("/users/me").dispatch().status(),
        Status::Unauthorized
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
fn http_echo() {
    let client = Client::tracked(create_app()).unwrap();
    let input1 = json!({"text": "Hello\nWorld!🚀"});
    let response1 = client
        .post("/echo")
        .header(ContentType::JSON)
        .body(input1.to_string())
        .dispatch();
    assert_eq!(response1.status(), Status::Ok);
    assert_eq!(
        response1.into_json::<Value>().unwrap(),
        json!({"data": "Hello\nWorld!🚀"})
    );

    let input2 = json!({"text": ""});
    let response2 = client
        .post("/echo")
        .header(ContentType::JSON)
        .body(input2.to_string())
        .dispatch();
    assert_eq!(response2.status(), Status::Ok);
    assert_eq!(response2.into_json::<Value>().unwrap(), json!({"data": ""}));

    let input3 = json!({"text": 123});
    let response3 = client
        .post("/echo")
        .header(ContentType::JSON)
        .body(input3.to_string())
        .dispatch();
    assert_eq!(response3.status(), Status::BadRequest);

    let input4 = json!({"text": "a".repeat(65_537)});
    let response4 = client
        .post("/echo")
        .header(ContentType::JSON)
        .body(input4.to_string())
        .dispatch();
    assert_eq!(response4.status(), Status::PayloadTooLarge);

    let input5 = json!({"text": &"a".repeat(65_536)});
    let response5 = client
        .post("/echo")
        .header(ContentType::JSON)
        .body(input5.to_string())
        .dispatch();
    assert_eq!(response5.status(), Status::Ok);
}
#[test]
fn http_text_lifecycle_and_isolation() {
    let client = Client::tracked(create_app()).unwrap();
    let alice = json!({"username": "alice", "password": "password1"}).to_string();
    assert_eq!(
        client
            .post("/users")
            .body(&alice)
            .header(ContentType::JSON)
            .dispatch()
            .status(),
        Status::Created
    );
    let login1 = client
        .post("/sessions")
        .body(&alice)
        .header(ContentType::JSON)
        .dispatch()
        .into_json::<Value>()
        .unwrap();
    let alice_token = format!("Bearer {}", login1["data"]["token"].as_str().unwrap());
    let put_res1 = client
        .put("/texts/note")
        .header(Header::new("Authorization", alice_token.clone()))
        .header(ContentType::JSON)
        .body(json!({"text": "Hello, World!"}).to_string())
        .dispatch();
    assert_eq!(put_res1.status(), Status::Ok);
    assert_eq!(
        put_res1.into_json::<Value>().unwrap(),
        json!({"data": null})
    );
    let get_res1 = client
        .get("/texts/note")
        .header(Header::new("Authorization", alice_token.clone()))
        .dispatch();
    assert_eq!(
        get_res1.into_json::<Value>().unwrap(),
        json!({"data": "Hello, World!"})
    );
    let bob = json!({"username": "bob", "password": "password2"}).to_string();
    assert_eq!(
        client
            .post("/users")
            .body(&bob)
            .header(ContentType::JSON)
            .dispatch()
            .status(),
        Status::Created
    );
    let login2 = client
        .post("/sessions")
        .body(&bob)
        .header(ContentType::JSON)
        .dispatch()
        .into_json::<Value>()
        .unwrap();
    let bob_token = format!("Bearer {}", login2["data"]["token"].as_str().unwrap());
    assert_eq!(
        client
            .get("/texts/note")
            .header(Header::new("Authorization", bob_token.clone()))
            .dispatch()
            .status(),
        Status::NotFound
    );
    assert_eq!(
        client
            .put("/texts/note")
            .header(Header::new("Authorization", bob_token.clone()))
            .header(ContentType::JSON)
            .body(json!({"text": "Hello, Bob!"}).to_string())
            .dispatch()
            .status(),
        Status::Ok
    );
    assert_eq!(
        client
            .delete("/texts/note")
            .header(Header::new("Authorization", alice_token.clone()))
            .dispatch()
            .status(),
        Status::Ok
    );
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
    assert_eq!(
        client.get("/texts/note").dispatch().status(),
        Status::Unauthorized
    );
    assert_eq!(
        client
            .get("/texts/bad!name")
            .header(Header::new("Authorization", alice_token.clone()))
            .dispatch()
            .status(),
        Status::BadRequest
    );
}
#[test]
fn http_user_deletion_lifecycle_and_cleanup() {
    let client = Client::tracked(create_app()).unwrap();
    let alice = json!({"username": "alice", "password": "password1"}).to_string();
    assert_eq!(
        client
            .post("/users")
            .body(&alice)
            .header(ContentType::JSON)
            .dispatch()
            .status(),
        Status::Created
    );
    let login1 = client
        .post("/sessions")
        .body(&alice)
        .header(ContentType::JSON)
        .dispatch()
        .into_json::<Value>()
        .unwrap();
    let alice_token = format!("Bearer {}", login1["data"]["token"].as_str().unwrap());
    let bob = json!({"username": "bob", "password": "password2"}).to_string();
    assert_eq!(
        client
            .post("/users")
            .body(&bob)
            .header(ContentType::JSON)
            .dispatch()
            .status(),
        Status::Created
    );
    let login2 = client
        .post("/sessions")
        .body(&bob)
        .header(ContentType::JSON)
        .dispatch()
        .into_json::<Value>()
        .unwrap();
    let bob_token = format!("Bearer {}", login2["data"]["token"].as_str().unwrap());
    assert_eq!(
        client
            .put("/texts/note")
            .header(Header::new("Authorization", bob_token.clone()))
            .header(ContentType::JSON)
            .body(json!({"text": "bob text"}).to_string())
            .dispatch()
            .status(),
        Status::Ok
    );
    assert_eq!(
        client
            .put("/texts/note")
            .header(Header::new("Authorization", alice_token.clone()))
            .header(ContentType::JSON)
            .body(json!({"text": "alice text"}).to_string())
            .dispatch()
            .status(),
        Status::Ok
    );
    let del_res = client
        .delete("/users/me")
        .header(Header::new("Authorization", alice_token.clone()))
        .dispatch();
    assert_eq!(del_res.status(), Status::Ok);
    assert_eq!(del_res.into_json::<Value>().unwrap(), json!({"data": null}));
    assert_eq!(
        client
            .get("/texts/note")
            .header(Header::new("Authorization", alice_token.clone()))
            .dispatch()
            .status(),
        Status::Unauthorized
    );
    assert_eq!(
        client
            .get("/texts/note")
            .header(Header::new("Authorization", bob_token.clone()))
            .dispatch()
            .status(),
        Status::Ok
    );
    let bob_res = client
        .get("/texts/note")
        .header(Header::new("Authorization", bob_token.clone()))
        .dispatch();
    assert_eq!(bob_res.status(), Status::Ok);

    assert_eq!(
        client
            .post("/users")
            .body(&alice)
            .header(ContentType::JSON)
            .dispatch()
            .status(),
        Status::Created
    );
    let new_login = client
        .post("/sessions")
        .body(&alice)
        .header(ContentType::JSON)
        .dispatch()
        .into_json::<Value>()
        .unwrap();
    let new_alice_token = format!("Bearer {}", new_login["data"]["token"].as_str().unwrap());

    let texts = client
        .get("/texts")
        .header(Header::new("Authorization", new_alice_token.clone()))
        .dispatch();
    assert_eq!(texts.status(), Status::Ok);
    assert_eq!(texts.into_json::<Value>().unwrap(), json!({"data": []}));

    assert_eq!(
        client
            .get("/texts/note")
            .header(Header::new("Authorization", new_alice_token))
            .dispatch()
            .status(),
        Status::NotFound
    );
}
