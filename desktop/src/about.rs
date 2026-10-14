//! Product identity and build provenance, kept separate from operational settings.
use super::*;
use crate::theme::*;

impl Desktop {
    pub fn about_page(&self, _: &mut Context<Self>) -> AnyElement {
        let commit = env!("COURSE2MD_DESKTOP_COMMIT");
        let version = if commit.is_empty() {
            env!("CARGO_PKG_VERSION").to_owned()
        } else {
            format!("{} · 构建 {commit}", env!("CARGO_PKG_VERSION"))
        };
        v_flex().w_full().min_w_0().gap(px(20.)).text_color(color(INK))
            .child(h_flex().w_full().min_w_0().items_center().gap(px(16.))
                .child(img("images/course2md.png").size(px(56.)).flex_shrink_0())
                .child(v_flex().flex_1().min_w_0().gap_1()
                    .child(accessible_text("about-name", "course2md").role(Role::Heading).text_size(TEXT_TITLE).font_weight(FontWeight::SEMIBOLD))
                    .child(accessible_text("about-build", version).text_size(TEXT_AUX).text_color(color(MUTED))))
                .child(h_flex().gap_2().items_center().flex_wrap()
                    .child(quiet("about-project").icon(icons::external_link()).label("项目主页")
                        .on_click(|_, _, cx| cx.open_url("https://github.com/mizorewww/course2md")))
                    .child(quiet("about-issue").icon(icons::edit()).label("发送反馈")
                        .on_click(|_, _, cx| cx.open_url("https://github.com/mizorewww/course2md/issues")))))
            .child(div().w_full().h(px(1.)).bg(color(HAIRLINE)))
            .child(v_flex().w_full().min_w_0().gap_3()
                .child(accessible_text("about-license-title", "开源许可"))
                .child(h_flex().gap_2().flex_wrap()
                    .child(outline_pill("about-license").icon(icons::code()).label("course2md · MIT")
                        .on_click(|_, _, cx| cx.open_url("https://github.com/mizorewww/course2md/blob/main/LICENSE")))
                    .child(outline_pill("about-icons-license").icon(icons::code()).label("Material Icons · Apache 2.0")
                        .on_click(|_, _, cx| cx.open_url("https://github.com/google/material-design-icons/blob/master/LICENSE")))))
            .into_any_element()
    }
}
