use gate_db::bootstrap;
use rusqlite::Connection;
pub fn legacy() -> Connection {
    let mut conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(include_str!("../fixtures/legacy-api-keys.sql"))
        .unwrap();
    bootstrap::run(&mut conn).unwrap();
    let limits = gate_core::quota_limits::GlobalLimits::from_legacy([
        150.0, 9999.0, 150.0, 200.0, 50.0, 10000.0,
    ]);
    conn.execute(
        "INSERT INTO site_settings(key,value) VALUES('quota_modes_v1',?)",
        [serde_json::to_string(&limits).unwrap()],
    )
    .unwrap();

    conn.execute_batch(
        "INSERT INTO site_settings(key,value) VALUES('v5_access_mode','restricted');
         INSERT INTO site_settings(key,value) VALUES('v5_authorized_discord_ids','[\"900000000000000001\"]');
         INSERT INTO api_keys(id,name,token,created_at,discord_id,image_model_scope,rpm)
         VALUES(1,'synthetic','nai-synthetic-policy',1,'900000000000000001','all',10);"
    ).unwrap();
    conn
}
