from contextlib import redirect_stderr, redirect_stdout
from copy import deepcopy
import io
import json
from pathlib import Path
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
            self.selected.append(info)
            filename = Path(downloader.prepare_filename(info))
            filename.write_bytes(b"simulated media")
            info["filepath"] = str(filename)
            for language, track in (info.get("requested_subtitles") or {}).items():
                path = filename.with_suffix(f".{language}.{track['ext']}")
                path.write_text("simulated subtitles")
                track["filepath"] = str(path)
            for event in [
                {"status": "downloading", "downloaded_bytes": 25, "total_bytes": 100},
                {"status": "downloading", "downloaded_bytes": 50, "total_bytes_estimate": 100},
                {"status": "downloading", "downloaded_bytes": 150, "total_bytes_estimate": 100},
                {"status": "downloading", "downloaded_bytes": 1},
                {"status": "finished"},
            ]:
                for hook in downloader.params["progress_hooks"]:
                    hook(event)

        self.addCleanup(patch.stopall)
        patch.object(adapter.yt_dlp.YoutubeDL, "extract_info", extract).start()
        patch.object(adapter.yt_dlp.YoutubeDL, "process_info", process).start()
        patch.object(adapter.yt_dlp.YoutubeDL, "urlopen", side_effect=AssertionError("network access in offline test")).start()
        cookiejar = adapter.yt_dlp.cookies.YoutubeDLCookieJar()
        patch.object(adapter.yt_dlp.YoutubeDL, "cookiejar", property(lambda _: cookiejar)).start()
        # Exercise real option parsing while isolating tests from personal/system config.
        real_parse = adapter.yt_dlp.parse_options
        self.parse = patch.object(adapter.yt_dlp, "parse_options", side_effect=lambda args: real_parse(["--ignore-config", *args])).start()

    def run_adapter(self, *extra, prefix=False):
        args = [URL, "--out", str(self.out)]
        if extra:
            args = ["--", *extra, *args] if prefix else [*args, "--", *extra]
        stdout, stderr = io.StringIO(), io.StringIO()
        with redirect_stdout(stdout), redirect_stderr(stderr):
            adapter.main(args)
        self.diagnostics = stderr.getvalue()
        return [json.loads(line) for line in stdout.getvalue().splitlines()]

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
                stderr, stdout = io.StringIO(), io.StringIO()
                with redirect_stderr(stderr), redirect_stdout(stdout), self.assertRaises(SystemExit) as raised:
                    adapter.main([URL, "--out", str(self.out)])
                self.assertEqual(raised.exception.code, 1)
                self.assertIn("pass the URL of a single item", stderr.getvalue())
                self.assertEqual(stdout.getvalue(), "")
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
        with redirect_stderr(io.StringIO()), self.assertRaises(SystemExit) as raised:
            adapter.main([URL, "--out", str(self.out)])
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
            stderr, stdout = io.StringIO(), io.StringIO()
            with redirect_stderr(stderr), redirect_stdout(stdout), self.assertRaises(SystemExit) as raised:
                adapter.main([URL, "--out", str(self.out)])
        self.assertEqual(raised.exception.code, 1)
        self.assertIn("download failed", stderr.getvalue())
        self.assertEqual(stdout.getvalue(), "")


if __name__ == "__main__":
    unittest.main()
