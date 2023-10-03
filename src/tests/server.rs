use std::net::{SocketAddr, TcpListener};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{self, Request, StatusCode};
use hyper::client::HttpConnector;
use hyper::{Client, Response};
use mpart_async::client::MultipartRequest;
use rstest::*;
use tokio::process::Command;
use uuid::Uuid;

use crate::config::{config, config_force_init};
use crate::constants::s3::INDEXER_SERVICE_BUCKET;
use crate::constants::sqs::{FAILED_INDEXER_QUEUE, START_INDEXER_QUEUE};
use crate::domain::models::indexer::{IndexerModel, IndexerStatus, IndexerType};
use crate::handlers::indexers::fail_indexer::fail_indexer;
use crate::handlers::indexers::utils::get_s3_script_key;
use crate::infra::repositories::indexer_repository::{IndexerRepository, Repository};
use crate::routes::app_router;
use crate::AppState;

async fn send_create_indexer_request(
    client: Client<HttpConnector>,
    script_path: &str,
    addr: SocketAddr,
) -> Response<Body> {
    let mut mpart = MultipartRequest::default();

    mpart.add_file("script.js", script_path);
    mpart.add_field("webhook_url", "https://webhook.site/bc2ca42e-a8b2-43cf-b95c-779fb1a6bbbb");

    let response = client
        .request(
            Request::builder()
                .method(http::Method::POST)
                .header(http::header::CONTENT_TYPE, format!("multipart/form-data; boundary={}", mpart.get_boundary()))
                .uri(format!("http://{}/v1/indexers", addr))
                .body(Body::wrap_stream(mpart))
                .unwrap(),
        )
        .await
        .unwrap();

    // assert the response of the request
    assert_eq!(response.status(), StatusCode::OK);
    response
}

async fn send_start_indexer_request(client: Client<HttpConnector>, id: Uuid, addr: SocketAddr) {
    let response = client
        .request(
            Request::builder()
                .method(http::Method::POST)
                .uri(format!("http://{}/v1/indexers/start/{}", addr, id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    // assert the response of the request
    assert_eq!(response.status(), StatusCode::OK);
}

async fn send_stop_indexer_request(client: Client<HttpConnector>, id: Uuid, addr: SocketAddr) {
    let response = client
        .request(
            Request::builder()
                .method(http::Method::POST)
                .uri(format!("http://{}/v1/indexers/stop/{}", addr, id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    // assert the response of the request
    assert_eq!(response.status(), StatusCode::OK);
}

async fn assert_queue_contains_message_with_indexer_id(queue_url: &str, indexer_id: Uuid) {
    let config = config().await;
    let message = config.sqs_client().receive_message().queue_url(queue_url).send().await.unwrap();
    assert_eq!(message.messages.clone().unwrap().len(), 1);
    let message = message.messages().unwrap().get(0).unwrap();
    assert_eq!(message.body().unwrap(), indexer_id.to_string());
}

async fn get_indexer(id: Uuid) -> IndexerModel {
    let config = config().await;
    let repository = IndexerRepository::new(config.pool());
    repository.get(id).await.unwrap()
}

async fn is_process_running(process_id: i64) -> bool {
    Command::new("ps")
        // Silence  stdout and stderr
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .args([
            "-p",
            process_id.to_string().as_str(),
        ])
        .spawn()
        .expect("Could not check the indexer status")
        .wait()
        .await
        .unwrap()
        .success()
}

#[fixture]
async fn setup_server() -> SocketAddr {
    config_force_init().await;
    let config = config().await;
    let state = AppState { pool: Arc::clone(config.pool()) };
    let app = app_router(state.clone()).with_state(state);

    let listener = TcpListener::bind("0.0.0.0:0".parse::<SocketAddr>().unwrap()).unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::Server::from_tcp(listener).unwrap().serve(app.into_make_service()).await.unwrap();
    });

    addr
}

#[rstest]
#[tokio::test]
async fn not_found(#[future] setup_server: SocketAddr) {
    let addr = setup_server.await;

    let client = hyper::Client::new();

    let response = client
        .request(Request::builder().uri(format!("http://{}/does-not-exist", addr)).body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = hyper::body::to_bytes(response.into_body()).await.unwrap();
    assert_eq!(&body[..], b"The requested resource was not found");
}

#[rstest]
#[tokio::test]
async fn health(#[future] setup_server: SocketAddr) {
    let addr = setup_server.await;

    let client = hyper::Client::new();

    let response = client
        .request(Request::builder().uri(format!("http://{}/health", addr)).body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = hyper::body::to_bytes(response.into_body()).await.unwrap();
    assert!(body.is_empty());
}

#[rstest]
#[tokio::test]
async fn create_indexer(#[future] setup_server: SocketAddr) {
    let addr = setup_server.await;

    let client = hyper::Client::new();
    let config = config().await;

    // clear the sqs queue
    config.sqs_client().purge_queue().queue_url(START_INDEXER_QUEUE).send().await.unwrap();

    // Create indexer
    let response = send_create_indexer_request(client.clone(), "./src/tests/scripts/test.js", addr).await;

    let body = hyper::body::to_bytes(response.into_body()).await.unwrap();
    let body: IndexerModel = serde_json::from_slice(&body).unwrap();

    assert_eq!(body.status, IndexerStatus::Created);
    assert_eq!(body.indexer_type, IndexerType::Webhook);
    assert_eq!(body.target_url, "https://webhook.site/bc2ca42e-a8b2-43cf-b95c-779fb1a6bbbb");

    // check if the file exists on S3
    assert!(
        config
            .s3_client()
            .get_object()
            .bucket(INDEXER_SERVICE_BUCKET)
            .key(get_s3_script_key(body.id))
            .send()
            .await
            .is_ok()
    );

    // check if the message is present on the queue
    assert_queue_contains_message_with_indexer_id(START_INDEXER_QUEUE, body.id).await;

    // check indexer is present in DB in created state
    let indexer = get_indexer(body.id).await;
    assert_eq!(indexer.id, body.id);
    assert_eq!(indexer.status, IndexerStatus::Created);
}

#[rstest]
#[tokio::test]
async fn start_indexer(#[future] setup_server: SocketAddr) {
    let addr = setup_server.await;

    let client = hyper::Client::new();
    let config = config().await;

    // clear the sqs queue
    config.sqs_client().purge_queue().queue_url(START_INDEXER_QUEUE).send().await.unwrap();

    // Create indexer
    let response = send_create_indexer_request(client.clone(), "./src/tests/scripts/test.js", addr).await;

    let body = hyper::body::to_bytes(response.into_body()).await.unwrap();
    let body: IndexerModel = serde_json::from_slice(&body).unwrap();

    // start the indexer
    send_start_indexer_request(client.clone(), body.id, addr).await;

    // check indexer is present in DB in running state
    let indexer = get_indexer(body.id).await;
    assert_eq!(indexer.id, body.id);
    assert_eq!(indexer.status, IndexerStatus::Running);

    // check the process is actually up
    assert!(is_process_running(indexer.process_id.unwrap()).await,);
}

#[rstest]
#[tokio::test]
async fn failed_running_indexer(#[future] setup_server: SocketAddr) {
    let addr = setup_server.await;

    let client = hyper::Client::new();
    let config = config().await;

    // clear the sqs queue
    config.sqs_client().purge_queue().queue_url(FAILED_INDEXER_QUEUE).send().await.unwrap();

    // Create indexer
    let response = send_create_indexer_request(client.clone(), "./src/tests/scripts/broken_indexer.js", addr).await;

    let body = hyper::body::to_bytes(response.into_body()).await.unwrap();
    let body: IndexerModel = serde_json::from_slice(&body).unwrap();

    // start the indexer
    send_start_indexer_request(client.clone(), body.id, addr).await;

    // sleep for 2 seconds to let the indexer fail
    tokio::time::sleep(Duration::from_secs(2)).await;

    // check if the message is present on the queue
    assert_queue_contains_message_with_indexer_id(FAILED_INDEXER_QUEUE, body.id).await;

    // fail the indexer
    assert!(fail_indexer(body.id).await.is_ok());

    // check indexer is present in DB in failed running state state
    let indexer = get_indexer(body.id).await;
    assert_eq!(indexer.id, body.id);
    assert_eq!(indexer.status, IndexerStatus::FailedRunning);

    // check the process has exited
    assert!(!is_process_running(indexer.process_id.unwrap()).await);
}

#[rstest]
#[tokio::test]
async fn stop_indexer(#[future] setup_server: SocketAddr) {
    let addr = setup_server.await;

    let client = hyper::Client::new();
    let config = config().await;

    // Create indexer
    let response = send_create_indexer_request(client.clone(), "./src/tests/scripts/test.js", addr).await;

    let body = hyper::body::to_bytes(response.into_body()).await.unwrap();
    let body: IndexerModel = serde_json::from_slice(&body).unwrap();

    // start the indexer
    send_start_indexer_request(client.clone(), body.id, addr).await;

    // stop the indexer
    send_stop_indexer_request(client.clone(), body.id, addr).await;

    // check indexer is present in DB in created state
    let indexer = get_indexer(body.id).await;
    assert_eq!(indexer.id, body.id);
    assert_eq!(indexer.status, IndexerStatus::Stopped);
}

#[rstest]
#[tokio::test]
async fn failed_stop_indexer(#[future] setup_server: SocketAddr) {
    let addr = setup_server.await;

    let client = hyper::Client::new();
    let config = config().await;

    // Create indexer
    let response = send_create_indexer_request(client.clone(), "./src/tests/scripts/test.js", addr).await;

    let body = hyper::body::to_bytes(response.into_body()).await.unwrap();
    let body: IndexerModel = serde_json::from_slice(&body).unwrap();

    // start the indexer
    send_start_indexer_request(client.clone(), body.id, addr).await;

    // kill indexer so stop fails
    let indexer = get_indexer(body.id).await;
    assert!(
        Command::new("kill")
        // Silence  stdout and stderr
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .args([
            indexer.process_id.unwrap().to_string().as_str(),
        ])
        .spawn()
        .expect("Could not stop the webhook indexer")
        .wait()
        .await
        .unwrap()
        .success()
    );

    // stop the indexer
    send_stop_indexer_request(client.clone(), body.id, addr).await;

    // check indexer is present in DB in created state
    let indexer = get_indexer(body.id).await;
    assert_eq!(indexer.id, body.id);
    assert_eq!(indexer.status, IndexerStatus::FailedStopping);
}
