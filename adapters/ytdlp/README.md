# notepipe-source-ytdlp

A [notepipe](https://github.com/3ae3ae/notepipe) source adapter that downloads media from any site
[yt-dlp](https://github.com/yt-dlp/yt-dlp) supports, so notepipe can take URLs.

## Install

```sh
uv tool install "notepipe[ytdlp]"
```

The configuration notepipe creates already uses it in its download stage. ffmpeg is required to merge formats and
convert subtitles.

## Behavior

- Your yt-dlp configuration files apply. Extra yt-dlp options go after `--`:
  ```toml
  [stages.download]
  type = "download"
  command = "notepipe-source-ytdlp"
  args = ["--", "--cookies-from-browser", "firefox", "-f", "bv*[height<=720]+ba/b"]
  ```
- Subtitles: manual subtitles in every language and automatic captions only in the media's original language, never
  machine-translated ones; all are delivered as SRT or VTT.
- Playlist URLs are rejected; pass the URL of a single item.
- For YouTube, notes link timestamps to the matching position in the video.
