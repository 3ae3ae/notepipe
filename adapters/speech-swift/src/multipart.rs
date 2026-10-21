use anyhow::{Context, Result, bail, ensure};

pub struct Upload<'a> {
    pub audio: &'a [u8],
    pub model: Option<&'a str>,
    pub language: Option<&'a str>,
}

pub fn parse<'a>(content_type: &str, body: &'a [u8]) -> Result<Upload<'a>> {
    let mut content_type = content_type.split(';');
    ensure!(
        content_type
            .next()
            .unwrap_or_default()
            .trim()
            .eq_ignore_ascii_case("multipart/form-data"),
        "Expected multipart/form-data"
    );
    let boundary = content_type
        .find_map(|parameter| {
            let (name, value) = parameter.trim().split_once('=')?;
            name.eq_ignore_ascii_case("boundary")
                .then_some(value.trim().trim_matches('"'))
        })
        .context("Missing multipart boundary")?;
    ensure!(
        !boundary.is_empty()
            && boundary.len() <= 70
            && boundary.is_ascii()
            && !boundary.bytes().any(|c| c.is_ascii_control()),
        "Invalid multipart boundary"
    );
    let marker = format!("--{boundary}");
    let delimiter = format!("\r\n{marker}");
    let mut rest = body
        .strip_prefix(marker.as_bytes())
        .context("Invalid multipart opening boundary")?;
    let (mut audio, mut model, mut language) = (None, None, None);
    loop {
        if let Some(tail) = rest.strip_prefix(b"--") {
            ensure!(
                tail.is_empty() || tail == b"\r\n",
                "Invalid multipart closing boundary"
            );
            break;
        }
        rest = rest
            .strip_prefix(b"\r\n")
            .context("Malformed multipart delimiter")?;
        let header_end = rest
            .windows(4)
            .position(|bytes| bytes == b"\r\n\r\n")
            .context("Missing multipart headers")?;
        ensure!(header_end <= 16 * 1024, "Multipart headers too large");
        let headers = std::str::from_utf8(&rest[..header_end])?;
        let disposition = headers
            .lines()
            .find_map(|header| {
                let (name, value) = header.split_once(':')?;
                name.eq_ignore_ascii_case("Content-Disposition")
                    .then_some(value.trim())
            })
            .context("Missing Content-Disposition")?;
        let mut parameters = disposition.split(';');
        ensure!(
            parameters.next() == Some("form-data"),
            "Expected form-data disposition"
        );
        let name = parameters
            .find_map(|parameter| {
                let (name, value) = parameter.trim().split_once('=')?;
                name.eq_ignore_ascii_case("name")
                    .then_some(value.trim().trim_matches('"'))
            })
            .context("Missing multipart field name")?;
        let data = &rest[header_end + 4..];
        let end = data
            .windows(delimiter.len() + 2)
            .position(|bytes| {
                bytes.starts_with(delimiter.as_bytes())
                    && matches!(&bytes[delimiter.len()..], b"\r\n" | b"--")
            })
            .context("Missing multipart closing boundary")?;
        let value = &data[..end];
        match name {
            "file" => {
                ensure!(audio.is_none(), "Duplicate file field");
                audio = Some(value);
            }
            "model" => {
                ensure!(model.is_none(), "Duplicate model field");
                model = Some(std::str::from_utf8(value)?);
            }
            "language" => {
                ensure!(language.is_none(), "Duplicate language field");
                language = Some(std::str::from_utf8(value)?);
            }
            _ => bail!("Unknown multipart field: {name}"),
        }
        rest = &data[end + delimiter.len()..];
    }
    let audio = audio
        .filter(|bytes| !bytes.is_empty())
        .context("A WAV file is required")?;
    Ok(Upload {
        audio,
        model,
        language,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_options_and_binary_audio_without_treating_prefixes_as_delimiters() {
        let audio = b"RIFF\0\xff\r\n--test-not-a-boundary\r\nWAVE";
        let mut body = b"--test\r\nContent-Disposition: form-data; name=\"language\"\r\n\r\nko\r\n--test\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\nwhisper\r\n--test\r\nContent-Disposition: form-data; name=\"file\"; filename=\"audio.wav\"\r\nContent-Type: audio/wav\r\n\r\n".to_vec();
        body.extend(audio);
        body.extend(b"\r\n--test--\r\n");
        let upload = parse("multipart/form-data; boundary=\"test\"", &body).unwrap();
        assert_eq!(upload.audio, audio);
        assert_eq!(upload.language, Some("ko"));
        assert_eq!(upload.model, Some("whisper"));
        assert!(parse("application/json", &body).is_err());
        assert!(parse("multipart/form-data", &body).is_err());
        assert!(
            parse(
                "multipart/form-data; boundary=test",
                &body[..body.len() - 8]
            )
            .is_err()
        );
        let duplicate = b"--test\r\nContent-Disposition: form-data; name=\"file\"\r\n\r\na\r\n--test\r\nContent-Disposition: form-data; name=\"file\"\r\n\r\nb\r\n--test--\r\n";
        assert!(parse("multipart/form-data; boundary=test", duplicate).is_err());
        assert!(parse("multipart/form-data; boundary=test", b"--test--\r\n").is_err());
    }
}
