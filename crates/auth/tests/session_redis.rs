//! `RedisSessionStore` 对真实 Redis 的生命周期测试。

use cicada_auth::{RedisSessionStore, Session, SessionStore, hash_refresh};
use cicada_infra::Cache;

fn redis_url() -> String {
    std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://localhost:6379/0".to_string())
}

#[tokio::test]
#[ignore = "需要真实 Redis"]
async fn redis_session_lifecycle() {
    let cache = Cache::connect(&redis_url()).await.unwrap();
    cache.ping().await.unwrap();
    let store = RedisSessionStore::new(cache);

    let uid = 424_242_i64;
    store.revoke_user(uid).await.unwrap();

    let jti = format!("test-{}", uuid::Uuid::new_v4().simple());
    let session = Session {
        user_id: uid,
        tenant_id: 1,
        username: "tester".to_string(),
        refresh_hash: hash_refresh("refresh-1"),
        ip: "127.0.0.1".to_string(),
        user_agent: "test".to_string(),
        login_time: 1,
        last_seen: 1,
    };

    store.create(&jti, &session, 60).await.unwrap();

    let got = store.get(&jti).await.unwrap().expect("会话应存在");
    assert_eq!(got, session);
    assert!(got.accepts_refresh("refresh-1"));
    assert!(!got.accepts_refresh("refresh-2"));
    assert_eq!(store.count_user(uid).await.unwrap(), 1);
    assert!(
        store
            .list_all()
            .await
            .unwrap()
            .iter()
            .any(|(id, _)| id == &jti),
        "全局在线索引应包含该会话"
    );

    store.revoke(&jti).await.unwrap();
    assert!(store.get(&jti).await.unwrap().is_none());
    assert_eq!(store.count_user(uid).await.unwrap(), 0);
    assert!(
        !store
            .list_all()
            .await
            .unwrap()
            .iter()
            .any(|(id, _)| id == &jti),
        "强退后全局在线索引应移除该会话"
    );
}
