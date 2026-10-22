use std::io::Write;

pub(crate) struct MockAi {
    pub(crate) url: String,
    pub(crate) calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    pub(crate) bodies: std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
    pub(crate) headers: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl MockAi {
    pub(crate) fn respond_with(
        respond: impl Fn(usize, &serde_json::Value) -> Option<(u16, serde_json::Value)> + Send + 'static,
    ) -> Self {
        use std::{
            io::Read,
            sync::{
                Arc,
                atomic::{AtomicBool, AtomicUsize, Ordering},
            },
            time::Duration,
        };
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let bodies = Arc::new(std::sync::Mutex::new(Vec::new()));
        let received = bodies.clone();
        let headers = Arc::new(std::sync::Mutex::new(Vec::new()));
        let received_headers = headers.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let (count, shutdown) = (calls.clone(), stop.clone());
        let worker = std::thread::spawn(move || {
            while !shutdown.load(Ordering::SeqCst) {
                let Ok((mut stream, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                };
                // BSD may inherit the listener's nonblocking mode. The request
                // reader below uses blocking reads with a bounded timeout.
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut data = Vec::new();
                let mut buffer = [0u8; 4096];
                let (header_end, length) = loop {
                    let size = stream.read(&mut buffer).unwrap();
                    if size == 0 {
                        return;
                    }
                    data.extend_from_slice(&buffer[..size]);
                    if let Some(index) = data.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                        let header = String::from_utf8_lossy(&data[..index]);
                        let length = header
                            .lines()
                            .find_map(|line| {
                                line.split_once(':')
                                    .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                                    .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                            })
                            .unwrap();
                        break (index + 4, length);
                    }
                };
                while data.len() < header_end + length {
                    let size = stream.read(&mut buffer).unwrap();
                    if size == 0 {
                        return;
                    }
                    data.extend_from_slice(&buffer[..size]);
                }
                let number = count.fetch_add(1, Ordering::SeqCst);
                let body: serde_json::Value =
                    serde_json::from_slice(&data[header_end..header_end + length]).unwrap();
                received.lock().unwrap().push(body.clone());
                received_headers
                    .lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&data[..header_end]).into_owned());
                let Some((status, response)) = respond(number, &body) else {
                    continue;
                };
                let response = serde_json::to_vec(&response).unwrap();
                // A cancelled run may already have closed the connection.
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    response.len()
                );
                let _ = stream.write_all(&response);
            }
        });
        Self {
            url,
            calls,
            bodies,
            headers,
            stop,
            worker: Some(worker),
        }
    }
}
impl Drop for MockAi {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(worker) = self.worker.take() {
            let result = worker.join();
            if !std::thread::panicking() {
                result.unwrap();
            }
        }
    }
}
