use cf_clairic::*;
use serde_json::{Value, json};
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

fn request() -> Request {
    Request {
        state: "Total 12".into(),
        fields: [("total".into(), Field::new(FieldType::Number))].into(),
        ..Request::default()
    }
}

async fn server(
    status: &str,
    body: &str,
    delay: Duration,
) -> (String, tokio::task::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/client/v4", listener.local_addr().unwrap());
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut data = Vec::new();
        loop {
            let mut buf = [0; 4096];
            let n = stream.read(&mut buf).await.unwrap();
            if n == 0 {
                break;
            }
            data.extend_from_slice(&buf[..n]);
            if let Some(end) = data.windows(4).position(|s| s == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&data[..end]).to_lowercase();
                let length: usize = headers
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length: "))
                    .unwrap()
                    .parse()
                    .unwrap();
                if data.len() >= end + 4 + length {
                    break;
                }
            }
        }
        tokio::time::sleep(delay).await;
        let _ = stream.write_all(response.as_bytes()).await;
        String::from_utf8(data).unwrap()
    });
    (base, task)
}

#[tokio::test]
async fn posts_request_and_preserves_nullable_metadata() {
    let body = json!({"success":true,"errors":[],"result":{
        "model":"clairic", "truncated":false,"usage":{"input_tokens":12,"output_tokens":0},
        "answers":{"total":{"type":"number","value":null,"guess":12,"confidence":0.8,"p_null":0.9},
        "prediction":{"type":"forecast","value":[1,2],"steps":[{"median":1}],"future_metadata":true}}
    }}).to_string();
    let (base, task) = server("200 OK", &body, Duration::ZERO).await;
    let client =
        Client::with_options("account", "test-token", &base, Duration::from_secs(2)).unwrap();
    let result = client.run(&request()).await.unwrap();
    assert!(result.answers["total"].value.is_null());
    assert_eq!(result.answers["total"].metadata["guess"], 12);
    assert_eq!(
        result.answers["prediction"].metadata["steps"][0]["median"],
        1
    );
    assert_eq!(result.usage.state_tokens, None);
    let raw = task.await.unwrap();
    assert!(raw.starts_with(
        "POST /client/v4/accounts/account/ai/run/@cf/cloudflare/clairic HTTP/1.1\r\n"
    ));
    assert!(raw.contains("authorization: Bearer test-token\r\n"));
    let sent: Value = serde_json::from_str(raw.split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(
        sent,
        json!({"model":"clairic","state":"Total 12","fields":{"total":{"type":"number"}}})
    );
    assert!(!format!("{client:?}").contains("test-token"));
    assert!(!format!("{client:?}").contains("account"));
}

#[tokio::test]
async fn distinguishes_api_and_malformed_responses() {
    for (status, body, expected) in [
        (
            "429 Too Many Requests",
            r#"{"errors":[{"code":1000,"message":"private upstream text"}]}"#,
            "api",
        ),
        ("502 Bad Gateway", "<html>bad gateway</html>", "api"),
        ("302 Found", "", "api"),
        (
            "200 OK",
            r#"{"success":false,"errors":[{"code":1,"message":"private upstream text"}]}"#,
            "api",
        ),
        ("200 OK", "{}", "decode"),
        ("200 OK", r#"{"success":true,"result":null}"#, "missing"),
    ] {
        let (base, task) = server(status, body, Duration::ZERO).await;
        let error = Client::with_options("account", "token", &base, Duration::from_secs(2))
            .unwrap()
            .run(&request())
            .await
            .unwrap_err();
        assert!(!error.to_string().contains("private upstream text"));
        match expected {
            "api" => assert!(matches!(error, Error::Api { .. })),
            "decode" => assert!(matches!(error, Error::Decode(_))),
            _ => assert!(matches!(error, Error::MissingResult)),
        }
        task.await.unwrap();
    }
}

#[tokio::test]
async fn timeout_and_local_validation() {
    let (base, task) = server("200 OK", "{}", Duration::from_millis(100)).await;
    let client =
        Client::with_options("account", "token", &base, Duration::from_millis(20)).unwrap();
    assert!(matches!(
        client.run(&request()).await,
        Err(Error::Transport(_))
    ));
    task.await.unwrap();
    assert!(matches!(
        client.run(&Request::default()).await,
        Err(Error::InvalidRequest(_))
    ));
    let mut empty = request();
    empty.state.clear();
    assert!(matches!(
        client.run(&empty).await,
        Err(Error::InvalidRequest(_))
    ));
}

#[test]
fn configuration_and_schema_variants() {
    assert!(Client::new("../other", "token").is_err());
    assert!(Client::new("account", "\nsecret").is_err());
    assert!(Client::new("account", "").is_err());
    for base in [
        "file:///tmp/test",
        "https://user:pass@example.com",
        "https://example.com?x=1",
        "https://example.com/#fragment",
    ] {
        assert!(Client::with_options("account", "token", base, Duration::from_secs(1)).is_err());
    }
    for kind in [
        "text", "number", "date", "time", "datetime", "duration", "list", "estimate", "forecast",
    ] {
        let field: Field = serde_json::from_value(json!({"type":kind})).unwrap();
        assert_eq!(serde_json::to_value(field).unwrap(), json!({"type":kind}));
    }
    let input = json!({"model":"clairic","state":"", "fields":{"forecast":{"type":"forecast","min":"2026-01-01","max":"2026-12-31","step":"month","scale":"linear","horizon":12,"interval":"month"}}, "images":[{"content_type":"image/png","base64":"eA=="},"data:image/webp;base64,eA=="],"options":{"include_quantiles":true,"return_state_text":true},"reference_date":"2026-01-01"});
    let decoded: Request = serde_json::from_value(input.clone()).unwrap();
    assert_eq!(serde_json::to_value(decoded).unwrap(), input);
}
