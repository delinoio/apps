// SPDX-License-Identifier: Apache-2.0
//! Connect streaming invalidations are hints; only `PullChanges` advances the cursor.
use crate::{auth::Auth, protocol::strv};
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::Duration;
pub struct Watch {
    state: Arc<Mutex<Value>>,
    stop: Arc<AtomicBool>,
    hints: mpsc::Receiver<()>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl Watch {
    pub fn start(auth: &Auth, credential: Value, epoch: String, cursor: String) -> Self {
        let state = Arc::new(Mutex::new(json!({
        "credential":credential,"epoch":epoch,"cursor":cursor}
        )));
        let stop = Arc::new(AtomicBool::new(false));
        let (send, hints) = mpsc::sync_channel(1);
        let cancelled = auth.cancelled.clone();
        let worker_state = state.clone();
        let worker_stop = stop.clone();
        let worker = std::thread::spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };
            runtime.block_on(async move {
                let Ok(http) = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).timeout(Duration::from_secs(30)).build() else {
 return;
 }
;
                loop {
                    let state = worker_state.lock().unwrap().clone();
                    let stream = receive(&http, &state, &send);
                    let stopped = async {
                        while !worker_stop.load(Ordering::Relaxed) && !cancelled.load(Ordering::Relaxed) {
                            tokio::time::sleep(Duration::from_millis(25)).await;
                        }
                    }
;
                    tokio::select! {
                        () = stopped => break,
                        supported = stream => if !supported {
                            tracing::warn!("Lane live invalidations unavailable; polling remains active");
                            break;
                        }
                    }
                    for _ in 0..40 {
                        if worker_stop.load(Ordering::Relaxed) || cancelled.load(Ordering::Relaxed) {
 return;
 }
                        tokio::time::sleep(Duration::from_millis(25)).await;
                    }
                }
            }
);
        });
        Self {
            state,
            stop,
            hints,
            worker: Some(worker),
        }
    }
    pub fn update(&self, credential: Value, epoch: String, cursor: String) {
        *self.state.lock().unwrap() = json!({
        "credential":credential,"epoch":epoch,"cursor":cursor}
        );
    }
    pub fn notified(&self) -> bool {
        self.hints.try_recv().is_ok()
    }
}
impl Drop for Watch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
async fn receive(http: &reqwest::Client, state: &Value, hints: &mpsc::SyncSender<()>) -> bool {
    let body = json!({
    "epoch":state["epoch"],"cursor":state["cursor"]}
    )
    .to_string();
    let Ok(length) = u32::try_from(body.len()) else {
        return true;
    };
    let mut frame = vec![0];
    frame.extend_from_slice(&length.to_be_bytes());
    frame.extend_from_slice(body.as_bytes());
    let credential = &state["credential"];
    let response = http
        .post(format!(
            "{}/lane.v1.SyncService/WatchChanges",
            strv(credential, "endpoint")
        ))
        .header("Content-Type", "application/connect+json")
        .header("Connect-Protocol-Version", "1")
        .bearer_auth(strv(credential, "access_token"))
        .body(frame)
        .send()
        .await;
    let Ok(mut response) = response else {
        return true;
    };
    if matches!(response.status().as_u16(), 404 | 501) {
        return false;
    }
    if !response.status().is_success() {
        return true;
    }
    let mut buffered = Vec::new();
    while let Ok(Some(chunk)) = response.chunk().await {
        if buffered.len() + chunk.len() > 16 << 20 {
            return true;
        }
        buffered.extend_from_slice(&chunk);
        while buffered.len() >= 5 {
            let length = u32::from_be_bytes(buffered[1..5].try_into().unwrap()) as usize;
            if length > 16 << 20 {
                return true;
            }
            if buffered.len() < 5 + length {
                break;
            }
            let flags = buffered[0];
            let value = serde_json::from_slice::<Value>(&buffered[5..5 + length]);
            if flags == 2 {
                return !value.is_ok_and(|v| v["error"]["code"] == "unimplemented");
            }
            if flags != 0 || value.is_err() {
                return true;
            }
            let _ = hints.try_send(());
            buffered.drain(..5 + length);
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, Read, Write},
        net::TcpListener,
        time::Instant,
    };
    #[test]
    fn receives_framed_hint_and_cancels_stalled_stream() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut reader = std::io::BufReader::new(&mut stream);
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            assert!(line.starts_with("POST /lane.v1.SyncService/WatchChanges "));
            let mut length = 0;
            loop {
                line.clear();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" {
                    break;
                }
                if let Some(value) = line.to_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse::<usize>().unwrap();
                }
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            assert_eq!(body[0], 0);
            assert_eq!(
                u32::from_be_bytes(body[1..5].try_into().unwrap()) as usize,
                body.len() - 5
            );
            let request: Value = serde_json::from_slice(&body[5..]).unwrap();
            assert_eq!(request["cursor"], "cursor-1");
            let payload = b"{}";
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/connect+json\r\nContent-Length: 107\r\nConnection: close\r\n\r\n").unwrap();
            stream.write_all(&[0, 0, 0, 0, 2]).unwrap();
            stream.write_all(payload).unwrap();
            stream.flush().unwrap();
            assert_eq!(stream.read(&mut [0]).unwrap(), 0);
        });
        let watch = Watch::start(
            &Auth::default(),
            json!({"endpoint":endpoint,"access_token":"synthetic"}),
            "epoch".into(),
            "cursor-1".into(),
        );
        let deadline = Instant::now() + Duration::from_secs(3);
        while !watch.notified() {
            assert!(Instant::now() < deadline, "missing hint");
            std::thread::sleep(Duration::from_millis(5));
        }
        let stopped = Instant::now();
        drop(watch);
        assert!(stopped.elapsed() < Duration::from_secs(1));
        server.join().unwrap();
    }
}
