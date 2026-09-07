#![allow(clippy::expect_used, clippy::unwrap_used)]

use crate::{LiveChainClient, LiveChainRpc};
use serde_json::json;
use wiremock::{
    matchers::{body_partial_json, method},
    Mock, MockServer, ResponseTemplate,
};

#[tokio::test]
async fn finalized_height_uses_the_finalized_hash_not_the_optimistic_header() {
    let server = MockServer::start().await;
    let hash = format!("0x{}", "a".repeat(64));
    Mock::given(method("POST"))
        .and(body_partial_json(
            json!({"method":"chain_getFinalizedHead","params":[]}),
        ))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"jsonrpc":"2.0","id":1,"result":hash})),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(body_partial_json(
            json!({"method":"chain_getHeader","params":[hash]}),
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"jsonrpc":"2.0","id":1,"result":{"number":"0x168"}})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let endpoint = server.uri();
    assert_eq!(
        tokio::task::spawn_blocking(move || LiveChainRpc::connect(&endpoint)
            .unwrap()
            .finalized_height())
        .await
        .unwrap()
        .unwrap(),
        360
    );
}

#[tokio::test]
async fn malformed_finality_and_unclosed_windows_fail_without_tip_fallback() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(body_partial_json(
            json!({"method":"chain_getFinalizedHead"}),
        ))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"jsonrpc":"2.0","id":1,"result":null})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let endpoint = server.uri();
    assert!(
        tokio::task::spawn_blocking(move || LiveChainRpc::connect(&endpoint)
            .unwrap()
            .finalized_height())
        .await
        .unwrap()
        .is_err()
    );
    server.reset().await;
    let hash = format!("0x{}", "b".repeat(64));
    Mock::given(method("POST"))
        .and(body_partial_json(
            json!({"method":"chain_getFinalizedHead"}),
        ))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"jsonrpc":"2.0","id":1,"result":hash})),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(body_partial_json(
            json!({"method":"chain_getHeader","params":[hash]}),
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"jsonrpc":"2.0","id":1,"result":{"number":"0x167"}})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let endpoint = server.uri();
    assert!(
        tokio::task::spawn_blocking(move || LiveChainClient::connect(&endpoint)
            .unwrap()
            .finalized_snapshot(360))
        .await
        .unwrap()
        .is_err()
    );
}

#[tokio::test]
async fn epoch_timestamp_and_metagraph_use_the_closed_boundary_hash() {
    use crate::storage::{storage_key, storage_map_key_u16};
    let server = MockServer::start().await;
    let hash = format!("0x{}", "d".repeat(64));
    for (rpc, params, result) in [
        ("chain_getFinalizedHead", json!([]), json!(hash)),
        ("chain_getHeader", json!([hash]), json!({"number":"0x2d0"})),
        ("chain_getBlockHash", json!(["0x168"]), json!(hash)),
        (
            "state_getStorage",
            json!([
                format!(
                    "0x{}",
                    hex::encode(storage_map_key_u16(
                        "SubtensorModule",
                        "SubnetEpochIndex",
                        1
                    ))
                ),
                hash
            ]),
            json!(format!("0x{}", hex::encode(7_u64.to_le_bytes()))),
        ),
        (
            "state_getStorage",
            json!([
                format!("0x{}", hex::encode(storage_key("Timestamp", "Now"))),
                hash
            ]),
            json!(format!("0x{}", hex::encode(1_000_000_u64.to_le_bytes()))),
        ),
        (
            "state_getStorage",
            json!([
                format!(
                    "0x{}",
                    hex::encode(storage_map_key_u16(
                        "SubtensorModule",
                        "SubnetOwnerHotkey",
                        1
                    ))
                ),
                hash
            ]),
            json!(format!("0x{}", "cc".repeat(32))),
        ),
        (
            "state_getStorage",
            json!([
                format!(
                    "0x{}",
                    hex::encode(storage_map_key_u16("SubtensorModule", "ValidatorPermit", 1))
                ),
                hash
            ]),
            json!("0x00"),
        ),
    ] {
        Mock::given(method("POST"))
            .and(body_partial_json(json!({"method":rpc,"params":params})))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"jsonrpc":"2.0","id":1,"result":result})),
            )
            .expect(1)
            .mount(&server)
            .await;
    }
    Mock::given(method("POST"))
        .and(body_partial_json(json!({"method":"state_getKeysPaged"})))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"jsonrpc":"2.0","id":1,"result":[]})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let endpoint = server.uri();
    let snapshot = tokio::task::spawn_blocking(move || {
        LiveChainClient::connect(&endpoint)
            .unwrap()
            .finalized_snapshot(360)
    })
    .await
    .unwrap()
    .unwrap();
    assert_eq!(snapshot.block, 360);
    assert_eq!(snapshot.chain_epoch, 7);
    assert_eq!(snapshot.timestamp_ms, 1_000_000);
    for request in server.received_requests().await.unwrap() {
        let body: serde_json::Value = request.body_json().unwrap();
        if body["method"] == "state_getKeysPaged" {
            assert_eq!(
                body["params"].as_array().unwrap().last().unwrap(),
                &json!(hash)
            );
        }
    }
}

#[tokio::test]
async fn missing_boundary_hash_never_activates_the_legacy_tip_sentinel() {
    let server = MockServer::start().await;
    let hash = format!("0x{}", "d".repeat(64));
    for (rpc, params, result) in [
        ("chain_getFinalizedHead", json!([]), json!(hash)),
        ("chain_getHeader", json!([hash]), json!({"number":"0x2d0"})),
        (
            "chain_getBlockHash",
            json!(["0x168"]),
            json!(format!("0x{}", "0".repeat(64))),
        ),
    ] {
        Mock::given(method("POST"))
            .and(body_partial_json(json!({"method":rpc,"params":params})))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"jsonrpc":"2.0","id":1,"result":result})),
            )
            .expect(1)
            .mount(&server)
            .await;
    }
    let endpoint = server.uri();
    assert!(
        tokio::task::spawn_blocking(move || LiveChainClient::connect(&endpoint)
            .unwrap()
            .finalized_snapshot(360))
        .await
        .unwrap()
        .is_err()
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 3);
}
