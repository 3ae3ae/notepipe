"""Download one online media item for the notepipe source contract."""
import argparse
from contextlib import redirect_stdout
import json
import os
from pathlib import Path
import sys
from urllib.parse import parse_qs, urlparse
from xml.etree.ElementTree import ParseError

import yt_dlp


CAPTION_EXTENSIONS = {*yt_dlp.utils.MEDIA_EXTENSIONS.subtitles, "ssa", "ttml", "dfxp", "tt", "srv1", "srv2", "srv3", "json3"}


def original_captions(info):
    captions = info.get("automatic_captions") or {}
    language = info.get("language")
    if not language:
        originals = [key[:-5] for key in captions if key.endswith("-orig")]
        if len(originals) == 1:
            language = originals[0]
    if not language:
        return {}
    formats = captions.get(f"{language}-orig") or captions.get(language) or []
    # A translation can share the media's language code; exclude translated URLs too.
    formats = [entry for entry in formats if "tlang" not in parse_qs(urlparse(entry.get("url", "")).query)]
    return {language: formats} if formats else {}


class YoutubeDL(yt_dlp.YoutubeDL):
    def process_ie_result(self, ie_result, download=True, extra_info=None):
        media_language = ie_result.get("language")
        if ie_result.get("_type") in {"playlist", "multi_video", "compat_list"} or "entries" in ie_result:
            raise ValueError("pass the URL of a single item")
        if ie_result.get("_type", "video") == "video":
            self.subtitle_tracks = {}
            for field, automatic, tracks in [
                ("subtitles", False, ie_result.get("subtitles") or {}),
                ("automatic_captions", True, original_captions(ie_result)),
            ]:
                ie_result[field] = {}
                for language, formats in tracks.items():
                    formats = [entry for entry in formats if entry.get("ext") in CAPTION_EXTENSIONS]
                    if language == "live_chat" or not formats:
                        continue
                    # Distinct keys preserve both manual and automatic tracks in one language.
                    key = f"{language}-auto" if automatic else language
                    ie_result[field][key] = formats
                    # Post-processors can replace subtitle dicts, so keep identity separately.
                    self.subtitle_tracks[key] = {"language": language, "automatic": automatic}
        result = super().process_ie_result(ie_result, download=download, extra_info=extra_info)
        if result and media_language and not result.get("language"):
            result["language"] = media_language
        return result


def relative_file(filename, out):
    path = Path(filename).resolve()
    relative = path.relative_to(out)
    if not path.is_file():
        raise ValueError(f"yt-dlp did not produce {relative}")
    return relative.as_posix()


def download(url, out, extra_args, output):
    def emit(value):
        print(json.dumps(value, ensure_ascii=False, allow_nan=False), file=output, flush=True)

    def progress(event):
        total = event.get("total_bytes") or event.get("total_bytes_estimate")
        if event.get("status") == "finished":
            emit({"progress": 1.0})
        elif event.get("status") == "downloading" and total:
            emit({"progress": min(1.0, max(0.0, event.get("downloaded_bytes", 0) / total))})

    if urlparse(url).scheme not in {"http", "https"}:
        raise ValueError("expected an http(s) URL")
    out = Path(out).resolve()
    out.mkdir(parents=True, exist_ok=True)
    if any(out.iterdir()):
        raise ValueError("--out must be an empty directory")

    # yt-dlp can print even with quiet enabled (e.g. --print); reserve stdout for JSON.
    with redirect_stdout(sys.stderr):
        options = yt_dlp.parse_options(extra_args).ydl_opts
        if not options.get("format"):
            options["format"] = "bestvideo+bestaudio/best/bestaudio"
        options.update(
            outtmpl={"default": "media.%(ext)s"},
            paths={"home": str(out), "temp": str(out)},
            noplaylist=True,
            extract_flat=False,
            progress_hooks=[progress],
            logtostderr=True,
            noprogress=True,
            simulate=False,
            skip_download=False,
            listformats=False,
            listsubtitles=False,
            list_thumbnails=False,
            writesubtitles=True,
            writeautomaticsub=True,
            allsubtitles=True,
            subtitleslangs=["all"],
            subtitlesformat="srt/vtt/best",
        )
        with YoutubeDL(options) as downloader:
            info = downloader.extract_info(url, download=True)
            if not info:
                raise ValueError("yt-dlp did not return a media item")
            downloads = info.get("requested_downloads") or [info]
            if len(downloads) != 1:
                raise ValueError("select a single merged media format")
            downloaded = {**info, **downloads[0]}
            tracks = downloaded.get("requested_subtitles") or {}
            for key, track in list(tracks.items()):
                if track.get("ext") in {"srt", "vtt"} or not track.get("filepath"):
                    continue
                subtitle_info = {
                    "requested_subtitles": {key: track},
                    "__files_to_move": {track["filepath"]: track["filepath"]},
                }
                try:
                    converter = yt_dlp.postprocessor.FFmpegSubtitlesConvertorPP(downloader, format="srt")
                    downloader.run_pp(converter, subtitle_info)
                except (yt_dlp.utils.PostProcessingError, OSError, ValueError, ParseError) as error:
                    downloader.report_warning(f"Skipping subtitle {key}: {error}")
                    del tracks[key]
                else:
                    tracks[key] = subtitle_info["requested_subtitles"][key]
    filename = downloaded.get("filepath")
    if not filename:
        raise ValueError("yt-dlp did not produce a media file")
    result = {
        "media": relative_file(filename, out),
        "url": info.get("webpage_url") or url,
        "subtitles": [
            {
                "path": relative_file(track["filepath"], out),
                **downloader.subtitle_tracks[key],
            }
            for key, track in (downloaded.get("requested_subtitles") or {}).items()
            if track.get("ext") in {"srt", "vtt"} and track.get("filepath") and Path(track["filepath"]).is_file()
        ],
    }
    for source, target in [("title", "title"), ("uploader", "author"), ("duration", "duration"), ("language", "language")]:
        if info.get(source) is not None:
            result[target] = info[source]
    if info.get("extractor_key") == "Youtube":
        result["seek_url"] = f"https://www.youtube.com/watch?v={info['id']}&t={{t}}s"
    emit(result)


def main(argv=None):
    sys.stdout.flush()
    with os.fdopen(os.dup(1), "w", encoding="utf-8") as output:
        # Child processes inherit fd 1; only the duplicated descriptor carries JSON.
        os.dup2(2, 1)
        parser = argparse.ArgumentParser(description="Download one media item with yt-dlp")
        parser.add_argument("url")
        parser.add_argument("--out", required=True)
        parser.epilog = "Pass yt-dlp options after -- (also supported before URL --out DIR in [download].adapter)."
        argv = list(sys.argv[1:] if argv is None else argv)
        if argv[:1] == ["--"]:
            # The core appends URL --out DIR to the configured command.
            adapter_args, extra_args = argv[-3:], argv[1:-3]
        else:
            separator = argv.index("--") if "--" in argv else len(argv)
            adapter_args, extra_args = argv[:separator], argv[separator + 1:]
        args = parser.parse_args(adapter_args)
        try:
            download(args.url, args.out, extra_args, output)
        except (yt_dlp.utils.DownloadError, OSError, ValueError) as error:
            parser.exit(1, f"{error}\n")
        except KeyboardInterrupt:
            parser.exit(130, "Download interrupted\n")
