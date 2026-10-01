use rm_server_async::Service;
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

    // 旧登录读完凭据后被暂停，此时账号被注销并同名重注册（换密码）
    let stale = service.read_credentials("alice").expect("alice exists");

    let delete = service.handle("DELETE", "/users/me", &Value::Null, &old_auth);
    assert_eq!(delete.0, 200);
    let account1 = json!({"username":"alice","password":"password2"});
    assert_eq!(service.handle("POST", "/users", &account1, "").0, 201);

    // 用旧快照提交登录必须失败：旧密码和新账号的密码都不能拿到令牌
    assert_eq!(service.login_with("alice", "password1", &stale).0, 401);
    assert_eq!(service.login_with("alice", "password2", &stale).0, 401);

    let login1 = service.handle("POST", "/sessions", &account1, "");
    assert_eq!(login1.0, 200);
}

#[test]
fn concurrent_text_operations_racing_with_user_deletion() {
    let service = std::sync::Arc::new(Service::default());
    let account = json!({"username": "alice", "password": "password1"});
    assert_eq!(service.handle("POST", "/users", &account, "").0, 201);
    let login = service.handle("POST", "/sessions", &account, "");
    assert_eq!(login.0, 200);
    let auth = format!("Bearer {}", login.1["data"]["token"].as_str().unwrap());

    assert_eq!(
        service
            .handle(
                "PUT",
                "/texts/init",
                &json!({"text": "initial content"}),
                &auth
            )
            .0,
        200
    );

    let start_barrier = std::sync::Arc::new(std::sync::Barrier::new(9));
    let mut workers = Vec::new();

    // 8 worker threads repeatedly performing PUT and GET operations
    for i in 0..8 {
        let s = service.clone();
        let a = auth.clone();
        let b = start_barrier.clone();
        workers.push(std::thread::spawn(move || {
            b.wait();
            let mut results = Vec::new();
            for j in 0..10 {
                let name = format!("text_{i}_{j}");
                let put_status = s
                    .handle(
                        "PUT",
                        &format!("/texts/{name}"),
                        &json!({"text": "data"}),
                        &a,
                    )
                    .0;
                let get_status = s
                    .handle("GET", &format!("/texts/{name}"), &Value::Null, &a)
                    .0;
                results.push((put_status, get_status));
            }
            results
        }));
    }

    // 1 worker thread deleting the account concurrently
    let s_del = service.clone();
    let a_del = auth.clone();
    let b_del = start_barrier.clone();
    let delete_worker = std::thread::spawn(move || {
        b_del.wait();
        s_del.handle("DELETE", "/users/me", &Value::Null, &a_del).0
    });

    let del_status = delete_worker.join().unwrap();
    assert_eq!(del_status, 200);

    for w in workers {
        let results = w.join().unwrap();
        for (put_status, get_status) in results {
            assert!(
                put_status == 200 || put_status == 401,
                "Unexpected put_status {put_status}"
            );
            assert!(
                get_status == 200 || get_status == 404 || get_status == 401,
                "Unexpected get_status {get_status}"
            );
        }
    }

    // Post-condition: old token is permanently revoked
    assert_eq!(service.handle("GET", "/texts", &Value::Null, &auth).0, 401);

    // Re-register alice: state must be clean and not have any leaked texts
    assert_eq!(service.handle("POST", "/users", &account, "").0, 201);
    let new_login = service.handle("POST", "/sessions", &account, "");
    assert_eq!(new_login.0, 200);
    let new_auth = format!("Bearer {}", new_login.1["data"]["token"].as_str().unwrap());
    assert_eq!(
        service.handle("GET", "/texts", &Value::Null, &new_auth),
        (200, json!({"data": []}))
    );
}

#[test]
fn concurrent_text_operations_racing_with_login_replacement() {
    let service = std::sync::Arc::new(Service::default());
    let account = json!({"username": "alice", "password": "password1"});
    assert_eq!(service.handle("POST", "/users", &account, "").0, 201);
    let login = service.handle("POST", "/sessions", &account, "");
    assert_eq!(login.0, 200);
    let old_auth = format!("Bearer {}", login.1["data"]["token"].as_str().unwrap());

    let start_barrier = std::sync::Arc::new(std::sync::Barrier::new(5));
    let mut workers = Vec::new();

    // 4 workers running operations with old_auth
    for i in 0..4 {
        let s = service.clone();
        let a = old_auth.clone();
        let b = start_barrier.clone();
        workers.push(std::thread::spawn(move || {
            b.wait();
            let mut statuses = Vec::new();
            for j in 0..10 {
                let status = s
                    .handle(
                        "PUT",
                        &format!("/texts/note_{i}_{j}"),
                        &json!({"text": "content"}),
                        &a,
                    )
                    .0;
                statuses.push(status);
            }
            statuses
        }));
    }

    // 1 worker concurrently logging in to replace token
    let s_log = service.clone();
    let acc_log = account.clone();
    let b_log = start_barrier.clone();
    let login_worker = std::thread::spawn(move || {
        b_log.wait();
        s_log.handle("POST", "/sessions", &acc_log, "")
    });

    let (login_status, login_body) = login_worker.join().unwrap();
    assert_eq!(login_status, 200);
    let new_auth = format!("Bearer {}", login_body["data"]["token"].as_str().unwrap());
    assert_ne!(old_auth, new_auth);

    for w in workers {
        let statuses = w.join().unwrap();
        for s in statuses {
            assert!(s == 200 || s == 401, "Unexpected status {s}");
        }
    }

    // Now old_auth MUST be revoked (401)
    assert_eq!(
        service.handle("GET", "/texts", &Value::Null, &old_auth).0,
        401
    );

    // new_auth MUST work and be able to read/write texts
    assert_eq!(
        service.handle("GET", "/texts", &Value::Null, &new_auth).0,
        200
    );
    assert_eq!(
        service
            .handle(
                "PUT",
                "/texts/new_note",
                &json!({"text": "after replacement"}),
                &new_auth
            )
            .0,
        200
    );
}

#[test]
fn text_list_is_sorted_and_tracks_creation_and_deletion() {
    let service = Service::default();
    let account = json!({"username":"alice","password":"password1"});
    assert_eq!(service.handle("POST", "/users", &account, "").0, 201);
    let login = service.handle("POST", "/sessions", &account, "");
    assert_eq!(login.0, 200);
    let auth = format!("Bearer {}", login.1["data"]["token"].as_str().unwrap());

    for (name, text) in [("b", "second"), ("a", "first"), ("C", "third")] {
        assert_eq!(
            service
                .handle(
                    "PUT",
                    &format!("/texts/{name}"),
                    &json!({"text": text}),
                    &auth
                )
                .0,
            200
        );
    }
    // 按名称升序、区分大小写：'C' < 'a' < 'b'
    assert_eq!(
        service.handle("GET", "/texts", &Value::Null, &auth),
        (200, json!({"data": ["C", "a", "b"]}))
    );

    // 同名覆盖读回新内容但不改变列表
    assert_eq!(
        service
            .handle("PUT", "/texts/a", &json!({"text": "first again"}), &auth)
            .0,
        200
    );
    assert_eq!(
        service.handle("GET", "/texts/a", &Value::Null, &auth),
        (200, json!({"data": "first again"}))
    );
    assert_eq!(
        service.handle("GET", "/texts", &Value::Null, &auth),
        (200, json!({"data": ["C", "a", "b"]}))
    );

    // 删除后名称从列表消失
    assert_eq!(
        service.handle("DELETE", "/texts/a", &Value::Null, &auth).0,
        200
    );
    assert_eq!(
        service.handle("GET", "/texts", &Value::Null, &auth),
        (200, json!({"data": ["C", "b"]}))
    );

    // 获取、删除不存在的文本，以及重复删除，都返回 404
    assert_eq!(
        service.handle("GET", "/texts/a", &Value::Null, &auth).0,
        404
    );
    assert_eq!(
        service.handle("DELETE", "/texts/a", &Value::Null, &auth).0,
        404
    );
    assert_eq!(
        service
            .handle("DELETE", "/texts/missing", &Value::Null, &auth)
            .0,
        404
    );

    // 全部删除后列表为空
    for name in ["C", "b"] {
        assert_eq!(
            service
                .handle("DELETE", &format!("/texts/{name}"), &Value::Null, &auth)
                .0,
            200
        );
    }
    assert_eq!(
        service.handle("GET", "/texts", &Value::Null, &auth),
        (200, json!({"data": []}))
    );
}

#[test]
fn text_limits_and_field_validation_apply_to_put() {
    let service = Service::default();
    let account = json!({"username":"alice","password":"password1"});
    assert_eq!(service.handle("POST", "/users", &account, "").0, 201);
    let login = service.handle("POST", "/sessions", &account, "");
    assert_eq!(login.0, 200);
    let auth = format!("Bearer {}", login.1["data"]["token"].as_str().unwrap());

    // 65_536 字节正好可写入
    let exact = "a".repeat(65_536);
    assert_eq!(
        service
            .handle("PUT", "/texts/exact", &json!({"text": exact}), &auth)
            .0,
        200
    );
    // 超出 1 字节返回 413，且不会写入
    let too_long = "a".repeat(65_537);
    assert_eq!(
        service
            .handle("PUT", "/texts/too_long", &json!({"text": too_long}), &auth)
            .0,
        413
    );
    assert_eq!(
        service
            .handle("GET", "/texts/too_long", &Value::Null, &auth)
            .0,
        404
    );
    // 上限按 UTF-8 字节计：22_000 个汉字是 66_000 字节
    let wide = "好".repeat(22_000);
    assert_eq!(
        service
            .handle("PUT", "/texts/wide", &json!({"text": wide}), &auth)
            .0,
        413
    );

    // 名称不合法返回 400（超长、非允许字符），路径形态不合法返回 404
    let long_name = "n".repeat(65);
    assert_eq!(
        service
            .handle(
                "PUT",
                &format!("/texts/{long_name}"),
                &json!({"text": "x"}),
                &auth
            )
            .0,
        400
    );
    assert_eq!(
        service
            .handle("PUT", "/texts/bad.name", &json!({"text": "x"}), &auth)
            .0,
        400
    );
    assert_eq!(
        service
            .handle("PUT", "/texts/", &json!({"text": "x"}), &auth)
            .0,
        404
    );
    assert_eq!(
        service
            .handle("PUT", "/texts/note", &json!({"text": 1}), &auth)
            .0,
        400
    );
    assert_eq!(
        service
            .handle(
                "PUT",
                "/texts/note",
                &json!({"text": "x", "extra": 1}),
                &auth
            )
            .0,
        400
    );
    assert_eq!(
        service.handle("PUT", "/texts/note", &json!({}), &auth).0,
        400
    );
}

#[test]
fn password_length_counts_unicode_scalar_values() {
    let service = Service::default();

    // 7 个标量值（28 字节）：按标量值不足 8 返回 400，按字节计会误判为合法
    let short = json!({"username":"alice","password":"😀".repeat(7)});
    assert_eq!(service.handle("POST", "/users", &short, "").0, 400);

    // 8 个标量值（32 字节）为下界
    let pass = json!({"username":"alice","password":"😀".repeat(8)});
    assert_eq!(service.handle("POST", "/users", &pass, "").0, 201);

    // 128 个标量值为上界
    let max = json!({"username":"bob","password":"a".repeat(128)});
    assert_eq!(service.handle("POST", "/users", &max, "").0, 201);
    let over = json!({"username":"carol","password":"a".repeat(129)});
    assert_eq!(service.handle("POST", "/users", &over, "").0, 400);
}

#[test]
fn absurd_token_ttl_is_clamped_instead_of_poisoning_the_service() {
    // 回归：`--token-ttl-seconds` 原来只挡 0，u64::MAX 会让 `Instant::now() + ttl`
    // 在登录时溢出 panic。panic 落在持有 users 锁的临界区里，会把互斥锁毒化，
    // 之后每个触及状态的请求都 500（只有不碰锁的 /ping 还活着，进程也不退出）。
    let service = Service::new(u64::MAX);
    assert_eq!(service.token_ttl_seconds, 365 * 24 * 60 * 60);

    let account = json!({"username":"alice","password":"password1"});
    assert_eq!(service.handle("POST", "/users", &account, "").0, 201);
    let login = service.handle("POST", "/sessions", &account, "");
    assert_eq!(login.0, 200);
    // `expires_in` 报的是钳过之后的值，与真实 deadline 一致
    assert_eq!(login.1["data"]["expires_in"], json!(365 * 24 * 60 * 60));
    assert_eq!(login.1["data"]["token"].as_str().unwrap().len(), 64);

    let auth = format!("Bearer {}", login.1["data"]["token"].as_str().unwrap());
    assert_eq!(
        service.handle("GET", "/texts", &Value::Null, &auth),
        (200, json!({"data": []}))
    );
    // 锁没有被毒化：后续注册和登录仍然正常
    let bob = json!({"username":"bob","password":"password1"});
    assert_eq!(service.handle("POST", "/users", &bob, "").0, 201);
    assert_eq!(service.handle("POST", "/sessions", &bob, "").0, 200);
}

#[test]
fn a_poisoned_lock_does_not_brick_the_service() {
    // 回归：4 处 `.lock().unwrap()` 会把"一次 panic"升级成"此后每个触及状态的请求
    // 都 500"——互斥锁中毒不会自愈，进程却还活着。现在用 `into_inner` 恢复。
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
