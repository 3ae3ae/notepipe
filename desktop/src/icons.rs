//! Embedded Material Icons, including the component library's standard icon paths.
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
        if path == "images/course2md.png" {
            return Ok(Some(Cow::Borrowed(include_bytes!(
                "../assets/128x128@2x.png"
            ))));
        }
        if let Some((_, bytes)) = ICONS
            .iter()
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
            ICONS
                .iter()
                .chain(BRAND_ICONS)
                .filter(|(name, _)| name.starts_with(path))
                .map(|(name, _)| SharedString::from(*name)),
        );
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
