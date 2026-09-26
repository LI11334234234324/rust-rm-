use rm_server_sync::Service;
use serde_json::{Value, json};

#[test]
fn input_validation_and_baseline() {
    let service = Service::default();
    assert_eq!(
        service.handle("GET", "/ping", &Value::Null, ""),
        (200, json!({"data":"pong"}))
    );
    for body in [
        Value::Null,
        json!([]),
        json!({"username":true,"password":"password1"}),
        json!({"username":"a/b","password":"password1"}),
    ] {
        assert_eq!(service.handle("POST", "/users", &body, "").0, 400);
    }
    assert_eq!(service.handle("GET", "/texts", &Value::Null, "").0, 401);
    assert_eq!(service.handle("GET", "/missing", &Value::Null, "").0, 404);
}

#[test]
fn concurrent_registration_has_one_winner() {
    let service = std::sync::Arc::new(Service::default());
    let workers: Vec<_> = (0..4)
        .map(|_| {
            let service = service.clone();
            std::thread::spawn(move || {
                service
                    .handle(
                        "POST",
                        "/users",
                        &json!({"username":"alice","password":"password1"}),
                        "",
                    )
                    .0
            })
        })
        .collect();
    let statuses: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect();
    assert_eq!(statuses.iter().filter(|&&s| s == 201).count(), 1);
    assert_eq!(statuses.iter().filter(|&&s| s == 409).count(), 3);
}

#[test]
fn stale_login_does_not_affect_re_registed_user() {
    let service = std::sync::Arc::new(Service::default());
    let account = json!({"username":"alice","password":"password1"});
    let reg = service.handle("POST", "/users", &account, "");
    assert_eq!(reg.0, 201);
    let login = service.handle("POST", "/sessions", &account, "");
    assert_eq!(login.0, 200);
    let old_auth = format!("Bearer {}", login.1["data"]["token"].as_str().unwrap());

    let service_clone = service.clone();
    let old_account = account.clone();
    let worker = std::thread::spawn(move || {
        service_clone
            .handle("POST", "/sessions", &old_account, "")
            .0
    });
    let delete = service.handle("DELETE", "/users/me", &Value::Null, &old_auth);
    assert_eq!(delete.0, 200);
    let account1 = json!({"username":"alice","password":"password2"});
    let reg1 = service.handle("POST", "/users", &account1, "");
    assert_eq!(reg1.0, 201);
    let state_status = worker.join().unwrap();
    assert_eq!(state_status, 401);
    let login1 = service.handle("POST", "/sessions", &account1, "");
    assert_eq!(login1.0, 200);
}
