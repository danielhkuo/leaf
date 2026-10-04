//! `leaf` as it starts: what it says and how it exits when it cannot get
//! going. Only failures that happen before leaf listens or talks to Discord
//! are run here, so nothing leaves this machine.

#![allow(clippy::unwrap_used, reason = "tests may panic")]

use leaf_core::config::{CONFIG_FILE_NAME, R2Config, Tier1Config};

#[tokio::test]
async fn a_media_folder_that_cannot_be_made_stops_leaf_and_is_named() {
    let dir = tempfile::tempdir().unwrap();
    // A file where the folder's parent should be: the folder cannot exist.
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, b"not a directory").unwrap();
    let folder = blocker.join("media");
    Tier1Config {
        discord_token: "SECRET_TOKEN_AAA".to_owned(),
        client_id: "123".to_owned(),
        client_secret: "SECRET_CLIENT_BBB".to_owned(),
        public_url: "https://leaf.example.com".to_owned(),
        r2: R2Config {
            endpoint: leaf_core::media::local_endpoint(folder.to_str().unwrap()).unwrap(),
            bucket: String::new(),
            access_key_id: String::new(),
            secret_access_key: String::new(),
        },
    }
    .save(&dir.path().join(CONFIG_FILE_NAME))
    .unwrap();

    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_leaf"))
        .env("DATA_DIR", dir.path())
        .env("BIND_ADDR", "127.0.0.1:0")
        .env_remove("DEV_GUILD_ID")
        .env_remove("LOG_LEVEL")
        .kill_on_drop(true)
        .output()
        .await
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).unwrap();
    // The storage is a folder here: the failure is not put down to R2.
    assert!(stderr.contains("opening media storage"), "{stderr}");
    assert!(!stderr.contains("R2"), "{stderr}");
    assert!(stderr.contains(folder.to_str().unwrap()), "{stderr}");
    // Neither credential is repeated.
    assert!(!stderr.contains("SECRET_"), "{stderr}");
}
