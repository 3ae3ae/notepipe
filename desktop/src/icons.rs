//! Shared outline icons with retained Material and brand assets for other semantics.
use gpui::{AssetSource, SharedString, Styled};
use std::borrow::Cow;

/// One registration per icon: the macro embeds the asset in `ICONS` and
/// exposes the access function from the same file stem, so a new icon can
/// no longer be registered in the table without its function, or vice versa.
macro_rules! icon {
    ($($name:ident, $file:literal;)+) => {
        const ICONS: &[(&str, &[u8])] = &[
            $((concat!("icons/", $file, ".svg"), include_bytes!(concat!("../assets/material/", $file, ".svg"))),)+
        ];
        $(
            pub fn $name() -> gpui_component::Icon {
                gpui_component::Icon::default().path(concat!("icons/", $file, ".svg"))
            }
        )+
    };
}

/// Alternate names keep the same registered path without a second table entry.
macro_rules! icon_alias {
    ($($name:ident -> $target:ident;)+) => {
        $(pub fn $name() -> gpui_component::Icon { $target() })+
    };
}

/// Brand marks embed from their own asset directory but share the lookup table
/// discipline; the public wrappers add each platform's intrinsic color.
macro_rules! brand {
    ($($name:ident, $file:literal;)+) => {
        const BRAND_ICONS: &[(&str, &[u8])] = &[
            $((concat!("brands/", $file, ".svg"), include_bytes!(concat!("../assets/brands/", $file, ".svg"))),)+
        ];
        $(fn $name() -> gpui_component::Icon {
            gpui_component::Icon::default().path(concat!("brands/", $file, ".svg"))
        })+
    };
}

// Preserve public paths so component IconName and app wrappers share one style.
const OUTLINE_ICONS: &[(&str, &[u8])] = &[
    (
        "icons/book-open.svg",
        include_bytes!("../assets/outline/book-open.svg"),
    ),
    (
        "icons/plus.svg",
        include_bytes!("../assets/outline/plus.svg"),
    ),
    (
        "icons/refresh.svg",
        include_bytes!("../assets/outline/refresh.svg"),
    ),
    (
        "icons/settings.svg",
        include_bytes!("../assets/outline/settings.svg"),
    ),
    (
        "icons/folder.svg",
        include_bytes!("../assets/outline/folder.svg"),
    ),
    (
        "icons/folder-open.svg",
        include_bytes!("../assets/outline/folder-open.svg"),
    ),
    (
        "icons/arrow-left.svg",
        include_bytes!("../assets/outline/arrow-left.svg"),
    ),
    (
        "icons/chevron-up.svg",
        include_bytes!("../assets/outline/chevron-up.svg"),
    ),
    (
        "icons/chevron-down.svg",
        include_bytes!("../assets/outline/chevron-down.svg"),
    ),
    (
        "icons/chevron-left.svg",
        include_bytes!("../assets/outline/chevron-left.svg"),
    ),
    (
        "icons/chevron-right.svg",
        include_bytes!("../assets/outline/chevron-right.svg"),
    ),
    (
        "icons/close.svg",
        include_bytes!("../assets/outline/close.svg"),
    ),
    (
        "icons/circle-x.svg",
        include_bytes!("../assets/outline/circle-x.svg"),
    ),
    (
        "icons/file.svg",
        include_bytes!("../assets/outline/file.svg"),
    ),
    (
        "icons/check.svg",
        include_bytes!("../assets/outline/check.svg"),
    ),
    (
        "icons/external-link.svg",
        include_bytes!("../assets/outline/external-link.svg"),
    ),
    (
        "icons/circle-check.svg",
        include_bytes!("../assets/outline/circle-check.svg"),
    ),
    (
        "icons/info.svg",
        include_bytes!("../assets/outline/info.svg"),
    ),
    (
        "icons/triangle-alert.svg",
        include_bytes!("../assets/outline/triangle-alert.svg"),
    ),
    (
        "icons/link.svg",
        include_bytes!("../assets/outline/link.svg"),
    ),
    (
        "icons/search.svg",
        include_bytes!("../assets/outline/search.svg"),
    ),
    (
        "icons/download.svg",
        include_bytes!("../assets/outline/download.svg"),
    ),
    (
        "icons/schedule.svg",
        include_bytes!("../assets/outline/schedule.svg"),
    ),
    (
        "icons/arrow-forward.svg",
        include_bytes!("../assets/outline/arrow-forward.svg"),
    ),
    (
        "icons/file-upload.svg",
        include_bytes!("../assets/outline/file-upload.svg"),
    ),
    (
        "icons/content-copy.svg",
        include_bytes!("../assets/outline/content-copy.svg"),
    ),
    (
        "icons/edit.svg",
        include_bytes!("../assets/outline/edit.svg"),
    ),
    (
        "icons/article.svg",
        include_bytes!("../assets/outline/article.svg"),
    ),
    ("icons/mic.svg", include_bytes!("../assets/outline/mic.svg")),
    (
        "icons/computer.svg",
        include_bytes!("../assets/outline/computer.svg"),
    ),
    (
        "icons/cloud.svg",
        include_bytes!("../assets/outline/cloud.svg"),
    ),
    (
        "icons/auto-fix.svg",
        include_bytes!("../assets/outline/auto-fix.svg"),
    ),
    (
        "icons/code.svg",
        include_bytes!("../assets/outline/code.svg"),
    ),
    (
        "icons/palette.svg",
        include_bytes!("../assets/outline/palette.svg"),
    ),
    (
        "icons/storage.svg",
        include_bytes!("../assets/outline/storage.svg"),
    ),
    (
        "icons/tune.svg",
        include_bytes!("../assets/outline/tune.svg"),
    ),
    (
        "icons/save.svg",
        include_bytes!("../assets/outline/save.svg"),
    ),
    (
        "icons/science.svg",
        include_bytes!("../assets/outline/science.svg"),
    ),
    (
        "icons/login.svg",
        include_bytes!("../assets/outline/login.svg"),
    ),
    (
        "icons/logout.svg",
        include_bytes!("../assets/outline/logout.svg"),
    ),
    (
        "icons/qr-code.svg",
        include_bytes!("../assets/outline/qr-code.svg"),
    ),
    ("icons/sun.svg", include_bytes!("../assets/outline/sun.svg")),
    (
        "icons/moon.svg",
        include_bytes!("../assets/outline/moon.svg"),
    ),
    (
        "icons/restart.svg",
        include_bytes!("../assets/outline/restart.svg"),
    ),
    (
        "icons/shield.svg",
        include_bytes!("../assets/outline/shield.svg"),
    ),
    (
        "icons/sparkles.svg",
        include_bytes!("../assets/outline/sparkles.svg"),
    ),
    (
        "icons/server.svg",
        include_bytes!("../assets/outline/server.svg"),
    ),
    ("icons/tv.svg", include_bytes!("../assets/outline/tv.svg")),
];

pub fn sparkles() -> gpui_component::Icon {
    gpui_component::Icon::default().path("icons/sparkles.svg")
}
pub fn server() -> gpui_component::Icon {
    gpui_component::Icon::default().path("icons/server.svg")
}
pub fn tv() -> gpui_component::Icon {
    gpui_component::Icon::default().path("icons/tv.svg")
}

pub struct Assets;
icon! {
    book_open, "book-open";
    plus, "plus";
    task, "task";
    refresh, "refresh";
    settings, "settings";
    folder, "folder";
    folder_open, "folder-open";
    arrow_left, "arrow-left";
    chevron_up, "chevron-up";
    chevron_down, "chevron-down";
    chevron_left, "chevron-left";
    chevron_right, "chevron-right";
    close, "close";
    circle_x, "circle-x";
    file, "file";
    check, "check";
    eye, "eye";
    eye_off, "eye-off";
    minus, "minus";
    external_link, "external-link";
    circle_check, "circle-check";
    info, "info";
    triangle_alert, "triangle-alert";
    window_close, "window-close";
    window_maximize, "window-maximize";
    window_minimize, "window-minimize";
    window_restore, "window-restore";
    loader, "loader";
    loader_circle, "loader-circle";
    link, "link";
    movie, "movie";
    search, "search";
    more_horiz, "more-horiz";
    download, "download";
    play_arrow, "play-arrow";
    schedule, "schedule";
    image, "image";
    format_list_bulleted, "format-list-bulleted";
    arrow_forward, "arrow-forward";
    pause, "pause";
    file_upload, "file-upload";
    content_copy, "content-copy";
    history, "history";
    edit, "edit";
    create_new_folder, "create-new-folder";
    delete, "delete";
    zoom_in, "zoom-in";
    zoom_out, "zoom-out";
    fit_screen, "fit-screen";
    article, "article";
    subtitles, "subtitles";
    mic, "mic";
    computer, "computer";
    cloud, "cloud";
    auto_fix, "auto-fix";
    code, "code";
    web, "web";
    palette, "palette";
    storage, "storage";
    tune, "tune";
    save, "save";
    science, "science";
    login, "login";
    logout, "logout";
    qr_code, "qr-code";
    sun, "sun";
    contrast, "contrast";
    home, "home";
    arrow_up, "arrow-up";
    arrow_down, "arrow-down";
    ellipsis, "ellipsis";
    stop, "stop";
    checklist, "checklist";
    dashboard, "dashboard";
    shield, "shield";
    grid_view, "grid-view";
    summarize, "summarize";
    moon, "moon";
    restart, "restart";
}
brand!(youtube_mark, "youtube"; bilibili_mark, "bilibili";);
icon_alias! {
    check_circle -> circle_check;
    error -> circle_x;
    warning -> triangle_alert;
    microphone -> mic;
    add -> plus;
    toc -> format_list_bulleted;
}

impl AssetSource for Assets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        if path == "licenses/lucide.txt" {
            return Ok(Some(Cow::Borrowed(include_bytes!(
                "../assets/outline/LICENSE"
            ))));
        }
        if path == "images/course2md.png" {
            return Ok(Some(Cow::Borrowed(include_bytes!(
                "../assets/128x128@2x.png"
            ))));
        }
        if let Some((_, bytes)) = OUTLINE_ICONS
            .iter()
            .chain(ICONS)
            .chain(BRAND_ICONS)
            .find(|(name, _)| *name == path)
        {
            return Ok(Some(Cow::Borrowed(bytes)));
        }
        gpui_kit_assets::Assets.load(path)
    }
    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        let mut paths = gpui_kit_assets::Assets.list(path)?;
        paths.extend(
            OUTLINE_ICONS
                .iter()
                .chain(ICONS)
                .chain(BRAND_ICONS)
                .filter(|(name, _)| name.starts_with(path))
                .map(|(name, _)| SharedString::from(*name)),
        );
        if "licenses/lucide.txt".starts_with(path) {
            paths.push("licenses/lucide.txt".into());
        }
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}

// Brand colors are intrinsic to the platform marks, independent of UI themes.
pub fn youtube() -> gpui_component::Icon {
    youtube_mark().text_color(gpui::rgb(0xff0000))
}

pub fn bilibili() -> gpui_component::Icon {
    bilibili_mark().text_color(gpui::rgb(0x00a1d6))
}
