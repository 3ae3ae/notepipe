//! Product identity and build provenance, kept separate from operational settings.
use super::*;
use crate::settings_ui::settings_detail_group;
use crate::theme::*;

impl Desktop {
    pub fn about_page(&self, _: &mut Context<Self>) -> AnyElement {
        let commit = env!("COURSE2MD_DESKTOP_COMMIT");
        let version = if commit.is_empty() {
            env!("CARGO_PKG_VERSION").to_owned()
        } else {
            format!("{} · 构建 {commit}", env!("CARGO_PKG_VERSION"))
        };
        v_flex()
            .w_full()
            .min_w_0()
            .gap_4()
            .text_color(color(INK))
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .items_center()
                    .gap_4()
                    .child(img("images/course2md.png").size(rems(64. / 14.)).flex_shrink_0())
                    .child(
                        v_flex()
                            .min_w_0()
                            .flex_1()
                            .gap_3()
                            .child(
                                v_flex()
                                    .min_w_0()
                                    .gap_1()
                                    .child(
                                        accessible_text("about-name", "course2md")
                                            .role(Role::Heading)
                                            .text_size(TEXT_TITLE)
                                            .font_weight(FontWeight::SEMIBOLD),
                                    )
                                    .child(
                                        accessible_text("about-build", version)
                                            .min_w_0()
                                            .whitespace_normal()
                                            .text_size(TEXT_AUX)
                                            .text_color(color(MUTED)),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .w_full()
                                    .min_w_0()
                                    .gap_2()
                                    .flex_wrap()
                                    .child(
                                        quiet("about-project")
                                            .icon(icons::external_link())
                                            .label("项目主页")
                                            .on_click(|_, _, cx| {
                                                cx.open_url("https://github.com/mizorewww/course2md");
                                            }),
                                    )
                                    .child(
                                        quiet("about-issue")
                                            .icon(icons::edit())
                                            .label("发送反馈")
                                            .on_click(|_, _, cx| {
                                                cx.open_url("https://github.com/mizorewww/course2md/issues");
                                            }),
                                    ),
                            ),
                    ),
            )
            .child(
                settings_detail_group("about-license-title", icons::code(), "开源许可")
                    .child(
                        h_flex()
                            .w_full()
                            .min_w_0()
                            .gap_2()
                            .flex_wrap()
                            .child(
                                quiet("about-license")
                                    .icon(icons::external_link())
                                    .label("course2md · MIT")
                                    .on_click(|_, _, cx| {
                                        cx.open_url(
                                            "https://github.com/mizorewww/course2md/blob/main/LICENSE",
                                        );
                                    }),
                            )
                            .child(
                                quiet("about-icons-license")
                                    .icon(icons::external_link())
                                    .label("Material Icons · Apache 2.0")
                                    .on_click(|_, _, cx| {
                                        cx.open_url(
                                            "https://github.com/google/material-design-icons/blob/master/LICENSE",
                                        );
                                    }),
                            ),
                    ),
            )
            .into_any_element()
    }
}
