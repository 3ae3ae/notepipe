mod apple;
mod multipart;

use anyhow::{Result, ensure};
use clap::Parser;
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    path::Path,
};
use tiny_http::{Header, Method, Request, Response, Server};

const MAX_UPLOAD: u64 = 64 * 1024 * 1024;

#[derive(Parser)]
#[command(about = "Apple silicon transcription HTTP adapter")]
struct Args {
    #[arg(long, default_value = "127.0.0.1")]
    host: String,
    #[arg(long)]
    port: u16,
    #[arg(long, default_value = "qwen3-1.7b", value_parser = ["qwen3-1.7b", "qwen3-0.6b", "whisper"])]
    model: String,
}

fn error(status: u16, message: impl ToString) -> (u16, Value) {
    (status, json!({"error":{"message":message.to_string()}}))
}

fn handle(
    request: &mut Request,
    model: &str,
    transcribe: impl FnOnce(&Path, &str) -> Result<String>,
) -> (u16, Value) {
    if request.method() == &Method::Get && request.url() == "/health" {
        return (200, json!({"status":"ok"}));
    }
    if request.url() != "/v1/audio/transcriptions" {
        return error(404, "Unknown endpoint");
    }
    if request.method() != &Method::Post {
        return error(405, "Use POST for transcription");
    }
    if request
        .body_length()
        .is_some_and(|length| length as u64 > MAX_UPLOAD)
    {
        return error(413, "Audio upload exceeds 64 MiB");
    }
    let content_type = request
        .headers()
        .iter()
        .find(|header| header.field.equiv("Content-Type"))
        .map(|header| header.value.as_str().to_owned())
        .unwrap_or_default();
    let mut body = Vec::new();
    if let Err(error) = request
        .as_reader()
        .take(MAX_UPLOAD + 1)
        .read_to_end(&mut body)
    {
        return self::error(400, error);
    }
    if body.len() as u64 > MAX_UPLOAD {
        return error(413, "Audio upload exceeds 64 MiB");
    }
    let prepared = (|| -> Result<_> {
        let upload = multipart::parse(&content_type, &body)?;
        ensure!(
            upload
                .model
                .is_none_or(|requested| requested.is_empty() || requested == model),
            "Requested model does not match the loaded model"
        );
        let mut file = tempfile::Builder::new().suffix(".wav").tempfile()?;
        file.write_all(upload.audio)?;
        Ok((file, upload.language.unwrap_or("")))
    })();
    let (file, language) = match prepared {
        Ok(value) => value,
        Err(err) => return error(400, err),
    };
    match transcribe(file.path(), language) {
        Ok(text) => (200, json!({"text":text})),
        Err(err) => error(500, err),
    }
}

fn main() -> Result<()> {
    let args = Args::parse();
    let asr = apple::Asr::load(&args.model)?;
    let server = Server::http((args.host.as_str(), args.port))
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    eprintln!("Ready on {}:{}", args.host, args.port);
    for mut request in server.incoming_requests() {
        let (status, body) = handle(&mut request, &args.model, |wav, language| {
            asr.transcribe(wav, language)
        });
        let response = Response::from_string(body.to_string())
            .with_status_code(status)
            .with_header(Header::from_bytes("Content-Type", "application/json").unwrap());
        if let Err(error) = request.respond(response) {
            eprintln!("HTTP response failed: {error}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpStream;

    fn call(method: &str, path: &str, body: &[u8]) -> String {
        let server = Server::http("127.0.0.1:0").unwrap();
        let address = server.server_addr().to_ip().unwrap();
        let serving = std::thread::spawn(move || {
            let mut request = server.recv().unwrap();
            let (status, value) = handle(&mut request, "whisper", |wav, language| {
                assert_eq!(std::fs::read(wav).unwrap(), b"RIFF\0\xffWAVE");
                assert_eq!(language, "ko");
                Ok("Fixture transcript".into())
            });
            request
                .respond(Response::from_string(value.to_string()).with_status_code(status))
                .unwrap();
        });
        let mut stream = TcpStream::connect(address).unwrap();
        write!(stream, "{method} {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: multipart/form-data; boundary=test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
        stream.write_all(body).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        serving.join().unwrap();
        response
    }

    #[test]
    fn http_routes_return_json_and_pass_the_language_and_wav_to_the_model() {
        assert!(call("GET", "/health", b"").starts_with("HTTP/1.1 200"));
        let mut body = b"--test\r\nContent-Disposition: form-data; name=\"language\"\r\n\r\nko\r\n--test\r\nContent-Disposition: form-data; name=\"file\"; filename=\"audio.wav\"\r\n\r\nRIFF\0\xffWAVE".to_vec();
        body.extend(b"\r\n--test--\r\n");
        let response = call("POST", "/v1/audio/transcriptions", &body);
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert!(response.contains("\"text\":\"Fixture transcript\""));
        let response = call("POST", "/v1/audio/transcriptions", b"invalid");
        assert!(response.starts_with("HTTP/1.1 400"));
        assert!(response.contains("\"error\":{\"message\":"));
        assert!(call("GET", "/unknown", b"").starts_with("HTTP/1.1 404"));
    }

    #[test]
    fn port_is_required_and_models_are_explicit() {
        assert!(Args::try_parse_from(["adapter"]).is_err());
        assert!(Args::try_parse_from(["adapter", "--port", "8000", "--model", "unknown"]).is_err());
    }
}
