# notepipe-source-ytdlp

A [notepipe](https://github.com/3ae3ae/notepipe) source adapter that downloads media from any site
[yt-dlp](https://github.com/yt-dlp/yt-dlp) supports, so notepipe can take URLs.

## Install

```sh
uv tool install notepipe --with notepipe-source-ytdlp
```

notepipe finds the adapter by its name; no configuration is needed. ffmpeg is required to merge formats and convert
subtitles.

## Behavior

- Your yt-dlp configuration files apply. Extra yt-dlp options go after `--`:
  ```toml
  [source]
  command = ["notepipe-source-ytdlp", "--", "--cookies-from-browser", "firefox", "-f", "bv*[height<=720]+ba/b"]
  ```
- Subtitles: manual subtitles in every language and automatic captions only in the media's original language, never
  machine-translated ones; all are delivered as SRT or VTT.
- Playlist URLs are rejected; pass the URL of a single item.
- For YouTube, notes link timestamps to the matching position in the video.
