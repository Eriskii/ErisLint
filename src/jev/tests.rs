// Copyright (C) 2026 Eriskii
// SPDX-License-Identifier: AGPL-3.0-only
// See LICENSE for the full license text.

use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    sync::atomic::{AtomicUsize, Ordering},
    time::Instant,
};

use serde_json::json;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

use super::*;

fn request() -> Request {
    Request {
        model: "jev-latest".into(),
        state: json!({ "name": "example", "body": "{ do_work(); }" }),
        questions: BTreeMap::from([(
            "quality".into(),
            Question::Choice {
                instructions: "Is this clear?".into(),
                criteria: BTreeMap::from([
                    ("clear".into(), "Clear".into()),
                    ("unclear".into(), "Unclear".into()),
                ]),
            },
        )]),
    }
}

fn answer() -> Value {
    json!({
        "model": "jev-test",
        "answers": {
            "quality": {
                "type": "choice", "choice": "clear", "confidence": 0.8,
                "probabilities": { "clear": 0.9, "unclear": 0.1 }
            }
        }
    })
}

async fn server(responses: Vec<ResponseTemplate>) -> MockServer {
    let server = MockServer::start().await;
    let next = AtomicUsize::new(0);
    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(move |_: &wiremock::Request| {
            responses[next
                .fetch_add(1, Ordering::Relaxed)
                .min(responses.len() - 1)]
            .clone()
        })
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn transient_http_failures_replay_the_same_authenticated_request() {
    futures_util::future::join_all([408, 429, 500, 502, 503, 504].map(|status| async move {
        let server = server(vec![
            ResponseTemplate::new(status),
            ResponseTemplate::new(200).set_body_json(answer()),
        ])
        .await;
        let client = JevClient::at_endpoint("test-key", &server.uri()).unwrap();
        let request = request();
        let response = client.evaluate(&request).await.unwrap();
        assert_eq!(response.model, "jev-test");
        let received = server.received_requests().await.unwrap();
        assert_eq!(received.len(), 2, "HTTP {status}");
        for attempt in received {
            assert_eq!(attempt.headers["authorization"], "Bearer test-key");
            assert_eq!(attempt.body_json::<Value>().unwrap(), json!(request));
        }
    }))
    .await;
}

#[tokio::test]
async fn persistent_failure_stops_after_four_attempts_even_with_retry_after() {
    let server = server(vec![
        ResponseTemplate::new(503).append_header("Retry-After", "0"),
    ])
    .await;
    let client = JevClient::at_endpoint("test-key", &server.uri()).unwrap();
    let error = client.evaluate(&request()).await.unwrap_err();
    let message = format!("{error:#}");
    assert!(message.contains("after 4 attempts"), "{message}");
    assert!(message.contains("503"), "{message}");
    assert_eq!(server.received_requests().await.unwrap().len(), 4);
}

#[tokio::test]
async fn permanent_http_errors_and_invalid_answers_are_not_retried() {
    let mut invalid = answer();
    invalid["answers"]["quality"]["choice"] = json!("undeclared");
    let mut replies: Vec<_> = [400, 401, 403, 404, 422, 501, 505]
        .map(ResponseTemplate::new)
        .into();
    replies.extend([
        ResponseTemplate::new(200).set_body_string("not json"),
        ResponseTemplate::new(200).set_body_json(json!({ "model": "jev-test", "answers": {} })),
        ResponseTemplate::new(200).set_body_json(invalid),
        ResponseTemplate::new(429).append_header("Retry-After", "61"),
    ]);
    for reply in replies {
        let server = server(vec![
            reply,
            ResponseTemplate::new(200).set_body_json(answer()),
        ])
        .await;
        let client = JevClient::at_endpoint("test-key", &server.uri()).unwrap();
        assert!(client.evaluate(&request()).await.is_err());
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }
}

#[tokio::test]
async fn retry_after_is_a_minimum_wait() {
    let server = server(vec![
        ResponseTemplate::new(429).append_header("Retry-After", "3"),
        ResponseTemplate::new(200).set_body_json(answer()),
    ])
    .await;
    let client = JevClient::at_endpoint("test-key", &server.uri()).unwrap();
    let started = Instant::now();
    client.evaluate(&request()).await.unwrap();
    assert!(started.elapsed() >= Duration::from_secs(3));
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[test]
fn retry_after_handles_dates_invalid_headers_and_excessive_delays() {
    let now = httpdate::parse_http_date("Wed, 21 Oct 2015 07:28:00 GMT").unwrap();
    for (header, expected) in [
        ("10", Some(Duration::from_secs(10))),
        (
            "Wed, 21 Oct 2015 07:28:15 GMT",
            Some(Duration::from_secs(15)),
        ),
        ("Wed, 21 Oct 2015 07:27:59 GMT", Some(Duration::ZERO)),
        (
            "999999999999999999999999",
            Some(Duration::from_secs(u64::MAX)),
        ),
        ("", None),
        ("invalid", None),
        ("-1", None),
        ("+1", None),
    ] {
        assert_eq!(
            retry_after(&HeaderValue::from_str(header).unwrap(), now),
            expected
        );
    }
    let failure = |delay| {
        anyhow::Error::new(HttpFailure {
            status: StatusCode::TOO_MANY_REQUESTS,
            retry_after: Some(Duration::from_secs(delay)),
        })
    };
    let backoff = Some(Duration::from_secs(4));
    assert_eq!(retry_delay(&failure(1), backoff), backoff);
    assert_eq!(retry_delay(&failure(60), backoff), Some(MAX_RETRY_DELAY));
    assert_eq!(retry_delay(&failure(61), backoff), None);
    assert_eq!(retry_delay(&failure(0), None), None);
}

#[tokio::test]
async fn timed_out_requests_can_recover() {
    let server = server(vec![
        ResponseTemplate::new(200)
            .set_body_json(answer())
            .set_delay(Duration::from_secs(1)),
        ResponseTemplate::new(200).set_body_json(answer()),
    ])
    .await;
    let mut client = JevClient::at_endpoint("test-key", &server.uri()).unwrap();
    client.client = Client::builder()
        .retry(reqwest::retry::never())
        .timeout(Duration::from_millis(250))
        .build()
        .unwrap();
    client.evaluate(&request()).await.unwrap();
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn interrupted_response_bodies_are_retried() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let body = answer().to_string();
    let responder = std::thread::spawn(move || {
        for response in [
            "HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\n{".to_owned(),
            format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            ),
        ] {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut reader = BufReader::new(&mut stream);
            let mut length = 0;
            loop {
                let mut line = String::new();
                assert_ne!(reader.read_line(&mut line).unwrap(), 0);
                if line == "\r\n" {
                    break;
                }
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap();
                }
            }
            reader.read_exact(&mut vec![0; length]).unwrap();
            stream.write_all(response.as_bytes()).unwrap();
        }
    });
    let client = JevClient::at_endpoint("test-key", &endpoint).unwrap();
    client.evaluate(&request()).await.unwrap();
    responder.join().unwrap();
}
