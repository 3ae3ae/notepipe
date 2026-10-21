from contextlib import redirect_stderr, redirect_stdout
from copy import deepcopy
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))
import notepipe_source_ytdlp as adapter


URL = "https://example.org/watch/clip"


def subtitles(*extensions, translated=False):
    return [
        {"ext": ext, "url": f"https://example.org/sub.{ext}" + ("?tlang=en" if translated else "")}
        for ext in extensions
    ]


class AdapterTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.out = Path(self.temp.name).resolve() / "download"
        self.info = {
            "id": "clip",
            "title": "A sample — 예시",
            "uploader": "Sample author",
            "duration": 12.5,
            "language": "ko",
            "webpage_url": "https://example.org/canonical/clip",
            "extractor": "test",
            "extractor_key": "Test",
            "formats": [
                {"format_id": "audio", "url": "https://example.org/audio.m4a", "ext": "m4a", "vcodec": "none", "acodec": "aac"},
                {"format_id": "combined", "url": "https://example.org/combined.mp4", "ext": "mp4", "vcodec": "h264", "acodec": "aac", "height": 360},
                {"format_id": "video", "url": "https://example.org/video.mp4", "ext": "mp4", "vcodec": "h264", "acodec": "none", "height": 720},
            ],
            "subtitles": {"en": subtitles("vtt", "srt"), "ko": subtitles("vtt")},
            "automatic_captions": {
                "ko-orig": subtitles("vtt"),
                "ko": subtitles("vtt"),
                "en": subtitles("vtt", translated=True),
                "ja": subtitles("vtt", translated=True),
            },
        }
        self.options = None
        self.selected = []

        def extract(downloader, url, download=True):
            self.assertEqual(url, URL)
            self.assertTrue(download)
            self.options = downloader.params
            print("yt-dlp diagnostic")
            return downloader.process_ie_result(deepcopy(self.info), download=download)

        def process(downloader, info):
            filename = Path(downloader.prepare_filename(info))
            filename.write_bytes(b"simulated media")
            info["filepath"] = str(filename)
            files_to_move = {}
            for language, track in (info.get("requested_subtitles") or {}).items():
                path = filename.with_suffix(f".{language}.{track['ext']}")
                path.write_text("simulated subtitles")
                track["filepath"] = str(path)
                files_to_move[str(path)] = str(path)
            new_info, files_to_move = downloader.pre_process(info, "before_dl", files_to_move)
            info.update(new_info)
            for event in [
                {"status": "downloading", "downloaded_bytes": 25, "total_bytes": 100},
                {"status": "downloading", "downloaded_bytes": 50, "total_bytes_estimate": 100},
                {"status": "downloading", "downloaded_bytes": 150, "total_bytes_estimate": 100},
                {"status": "downloading", "downloaded_bytes": 1},
                {"status": "finished"},
            ]:
                for hook in downloader.params["progress_hooks"]:
                    hook(event)
            info.update(downloader.post_process(str(filename), info, files_to_move))
            self.selected.append(deepcopy(info))

        self.addCleanup(patch.stopall)
        patch.object(adapter.yt_dlp.YoutubeDL, "extract_info", extract).start()
        patch.object(adapter.yt_dlp.YoutubeDL, "process_info", process).start()
        patch.object(adapter.yt_dlp.YoutubeDL, "urlopen", side_effect=AssertionError("network access in offline test")).start()
        cookiejar = adapter.yt_dlp.cookies.YoutubeDLCookieJar()
        patch.object(adapter.yt_dlp.YoutubeDL, "cookiejar", property(lambda _: cookiejar)).start()
        # Exercise real option parsing while isolating tests from personal/system config.
        real_parse = adapter.yt_dlp.parse_options
        self.parse = patch.object(adapter.yt_dlp, "parse_options", side_effect=lambda args: real_parse(["--ignore-config", *args])).start()

    def call_adapter(self, args):
        sys.stdout.flush()
        sys.stderr.flush()
        saved_stdout, saved_stderr = os.dup(1), os.dup(2)
        with tempfile.TemporaryFile(mode="w+", encoding="utf-8") as stdout, tempfile.TemporaryFile(mode="w+", encoding="utf-8") as stderr:
            try:
                os.dup2(stdout.fileno(), 1)
                os.dup2(stderr.fileno(), 2)
                with redirect_stdout(stdout), redirect_stderr(stderr):
                    adapter.main(args)
            finally:
                os.dup2(saved_stdout, 1)
                os.dup2(saved_stderr, 2)
                os.close(saved_stdout)
                os.close(saved_stderr)
                stdout.seek(0)
                stderr.seek(0)
                self.protocol = stdout.read()
                self.diagnostics = stderr.read()

    def run_adapter(self, *extra, prefix=False):
        args = [URL, "--out", str(self.out)]
        if extra:
            args = ["--", *extra, *args] if prefix else [*args, "--", *extra]
        self.call_adapter(args)
        return [json.loads(line) for line in self.protocol.splitlines()]

    def test_result_progress_subtitles_and_default_merged_format(self):
        lines = self.run_adapter()
        self.assertEqual(lines[:-1], [{"progress": n} for n in [0.25, 0.5, 1.0, 1.0]])
        result = lines[-1]
        self.assertEqual(result, {
            "media": "media.mp4",
            "title": "A sample — 예시",
            "author": "Sample author",
            "duration": 12.5,
            "language": "ko",
            "url": "https://example.org/canonical/clip",
            "subtitles": [
                {"path": "media.en.srt", "language": "en", "automatic": False},
                {"path": "media.ko.vtt", "language": "ko", "automatic": False},
                {"path": "media.ko-auto.vtt", "language": "ko", "automatic": True},
            ],
        })
        self.assertEqual([f["format_id"] for f in self.selected[0]["requested_formats"]], ["video", "audio"])
        for name in [result["media"], *(track["path"] for track in result["subtitles"])]:
            self.assertTrue((self.out / name).is_file())
        self.assertIn("yt-dlp diagnostic", self.diagnostics)

    def test_subtitle_identity_survives_conversion(self):
        converter = adapter.yt_dlp.postprocessor.FFmpegSubtitlesConvertorPP

        def convert(processor, source, target, options):
            Path(target).write_text("converted subtitles")

        with patch.object(converter, "run_ffmpeg", convert):
            result = self.run_adapter("--convert-subs", "srt")[-1]
        self.assertEqual(result["subtitles"], [
            {"path": "media.en.srt", "language": "en", "automatic": False},
            {"path": "media.ko.srt", "language": "ko", "automatic": False},
            {"path": "media.ko-auto.srt", "language": "ko", "automatic": True},
        ])
        for track in self.selected[0]["requested_subtitles"].values():
            self.assertNotIn("_notepipe_language", track)
            self.assertNotIn("_notepipe_automatic", track)
            self.assertEqual(Path(track["filepath"]).suffix, ".srt")

    def test_fallback_subtitles_are_converted_to_srt(self):
        self.info["subtitles"] = {"en": subtitles("json3"), "ko": subtitles("vtt", "ass")}
        self.info["automatic_captions"] = {"ko-orig": subtitles("json3")}

        def convert(processor, source, target, options):
            self.assertEqual(Path(source).suffix, ".json3")
            self.assertEqual(options, ["-f", "srt"])
            Path(target).write_text("converted subtitles")

        converter = adapter.yt_dlp.postprocessor.FFmpegSubtitlesConvertorPP
        with patch.object(converter, "run_ffmpeg", convert):
            result = self.run_adapter()[-1]
        self.assertEqual(result["subtitles"], [
            {"path": "media.en.srt", "language": "en", "automatic": False},
            {"path": "media.ko.vtt", "language": "ko", "automatic": False},
            {"path": "media.ko-auto.srt", "language": "ko", "automatic": True},
        ])
        for track in result["subtitles"]:
            self.assertTrue((self.out / track["path"]).is_file())

    def test_nonconvertible_subtitle_is_dropped(self):
        converter = adapter.yt_dlp.postprocessor.FFmpegSubtitlesConvertorPP
        for ext in ["srv3", "ttml"]:
            with self.subTest(ext=ext):
                self.out = Path(self.temp.name).resolve() / ext
                self.info["subtitles"]["en"] = subtitles(ext)
                with patch.object(converter, "run_ffmpeg", side_effect=adapter.yt_dlp.utils.PostProcessingError("cannot convert")):
                    result = self.run_adapter()[-1]
                self.assertEqual(result["subtitles"], [
                    {"path": "media.ko.vtt", "language": "ko", "automatic": False},
                    {"path": "media.ko-auto.vtt", "language": "ko", "automatic": True},
                ])
                self.assertIn("Skipping subtitle en:", self.diagnostics)

    def test_child_stdout_is_sent_to_diagnostics(self):
        extract = adapter.YoutubeDL.extract_info

        def extract_with_child(downloader, *args, **kwargs):
            subprocess.run([sys.executable, "-c", "print('child stdout diagnostic')"], check=True)
            return extract(downloader, *args, **kwargs)

        with patch.object(adapter.YoutubeDL, "extract_info", extract_with_child):
            result = self.run_adapter()[-1]
        self.assertEqual(result["media"], "media.mp4")
        self.assertIn("child stdout diagnostic", self.diagnostics)

    def test_non_caption_tracks_are_excluded_before_download(self):
        self.info["subtitles"].update({
            "live_chat": [
                {**track, "protocol": "youtube_live_chat_replay"}
                for track in subtitles("json", "vtt")
            ],
            "comments": subtitles("json"),
            "thumbnails": subtitles("mhtml"),
        })
        self.info["subtitles"]["en"].extend(subtitles("json"))
        result = self.run_adapter()[-1]
        self.assertEqual(set(self.selected[0]["requested_subtitles"]), {"en", "ko", "ko-auto"})
        self.assertEqual(set(self.selected[0]["subtitles"]), {"en", "ko"})
        self.assertEqual([track["ext"] for track in self.selected[0]["subtitles"]["en"]], ["vtt", "srt"])
        self.assertEqual(len(result["subtitles"]), 3)
        self.assertFalse(list(self.out.glob("*.json")))
        self.assertFalse(list(self.out.glob("*.mhtml")))

    def test_media_language_is_preserved_without_automatic_captions(self):
        self.info["subtitles"] = {"en": subtitles("vtt")}
        self.info["automatic_captions"] = {}
        self.assertEqual(self.run_adapter()[-1]["language"], "ko")

    def test_audio_only_fallback_and_optional_metadata(self):
        self.info["formats"] = self.info["formats"][:1]
        for field in ["title", "uploader", "duration", "language", "webpage_url", "subtitles", "automatic_captions"]:
            self.info.pop(field)
        result = self.run_adapter()[-1]
        self.assertEqual(result["media"], "media.m4a")
        self.assertEqual(result["url"], URL)
        self.assertEqual(result["subtitles"], [])
        for field in ["author", "duration", "language"]:
            self.assertNotIn(field, result)

    def test_passthrough_options_in_both_command_positions(self):
        for prefix in [False, True]:
            with self.subTest(prefix=prefix):
                self.out = Path(self.temp.name).resolve() / str(prefix)
                self.run_adapter("--cookies-from-browser", "chrome", "-f", "bestaudio", prefix=prefix)
                self.parse.assert_called_with(["--cookies-from-browser", "chrome", "-f", "bestaudio"])
                self.assertEqual(self.options["cookiesfrombrowser"][0], "chrome")
                self.assertEqual(self.options["format"], "bestaudio")
                self.assertEqual(self.selected[-1]["format_id"], "audio")

    def test_config_is_honored_except_contract_overrides(self):
        config = Path(self.temp.name) / "yt-dlp.conf"
        config.write_text(
            "-f bestaudio\n--socket-timeout 7\n--no-playlist\n"
            "-o elsewhere/%(title)s.%(ext)s\n-P elsewhere\n"
            "--skip-download\n--simulate\n--no-write-subs\n--sub-langs en\n"
        )
        self.run_adapter("--config-locations", str(config))
        self.assertEqual(self.options["format"], "bestaudio")
        self.assertEqual(self.options["socket_timeout"], 7)
        self.assertTrue(self.options["noplaylist"])
        self.assertEqual(self.options["paths"], {"home": str(self.out), "temp": str(self.out)})
        self.assertEqual(self.options["outtmpl"]["default"], "media.%(ext)s")
        self.assertFalse(self.options["simulate"])
        self.assertFalse(self.options["skip_download"])
        self.assertTrue(self.options["writesubtitles"])
        self.assertTrue(self.options["writeautomaticsub"])
        self.assertEqual(self.options["subtitleslangs"], ["all"])

    def test_playlist_is_rejected_before_download(self):
        for kind in ["playlist", "multi_video", "compat_list"]:
            with self.subTest(kind=kind):
                self.info = {"_type": kind, "entries": []}
                with self.assertRaises(SystemExit) as raised:
                    self.call_adapter([URL, "--out", str(self.out)])
                self.assertEqual(raised.exception.code, 1)
                self.assertIn("pass the URL of a single item", self.diagnostics)
                self.assertEqual(self.protocol, "")
                self.assertEqual(self.selected, [])

    def test_original_caption_selection(self):
        for info, expected in [
            ({"automatic_captions": {"ja-orig": subtitles("vtt"), "en": subtitles("vtt", translated=True)}}, {"ja"}),
            ({"language": "fr", "automatic_captions": {"fr": subtitles("vtt"), "en": subtitles("vtt", translated=True)}}, {"fr"}),
            ({"language": "en", "automatic_captions": {"en": subtitles("vtt", translated=True)}}, set()),
            ({"automatic_captions": {"en": subtitles("vtt"), "fr": subtitles("vtt")}}, set()),
            ({"automatic_captions": {"en-orig": subtitles("vtt"), "fr-orig": subtitles("vtt")}}, set()),
            ({"language": "ja", "automatic_captions": {"en-orig": subtitles("vtt")}}, set()),
        ]:
            with self.subTest(info=info):
                self.assertEqual(set(adapter.original_captions(info)), expected)

    def test_nonempty_output_directory_is_rejected(self):
        self.out.mkdir()
        existing = self.out / "existing.txt"
        existing.write_text("keep")
        with self.assertRaises(SystemExit) as raised:
            self.call_adapter([URL, "--out", str(self.out)])
        self.assertEqual(raised.exception.code, 1)
        self.assertEqual(existing.read_text(), "keep")
        self.assertIsNone(self.options)

    def test_paths_must_remain_inside_output_directory(self):
        outside = Path(self.temp.name) / "outside.mp4"
        outside.write_bytes(b"media")
        with self.assertRaises(ValueError):
            adapter.relative_file(outside, self.out)
        self.out.mkdir()
        (self.out / "link.mp4").symlink_to(outside)
        with self.assertRaises(ValueError):
            adapter.relative_file(self.out / "link.mp4", self.out)

    def test_download_failure_exits_nonzero_with_diagnostics(self):
        with patch.object(adapter.YoutubeDL, "extract_info", side_effect=adapter.yt_dlp.utils.DownloadError("download failed")):
            with self.assertRaises(SystemExit) as raised:
                self.call_adapter([URL, "--out", str(self.out)])
        self.assertEqual(raised.exception.code, 1)
        self.assertIn("download failed", self.diagnostics)
        self.assertEqual(self.protocol, "")


if __name__ == "__main__":
    unittest.main()
