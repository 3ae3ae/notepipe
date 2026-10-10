//! Generation defaults page: speech/AI model, languages and vision switches.
use super::*;

impl Desktop {
    pub(super) fn generation_settings_page(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let value = self.preferences.generation();
        let provider = value.options.provider.map(|p| p.as_str()).unwrap_or("");
        let language = match value.preferred_subtitle_languages.as_slice() {
            [] => "",
            [one] if ["zh-Hans", "zh-Hant", "en", "ja"].contains(&one.as_str()) => one.as_str(),
            _ => "custom",
        };
        let languages = group("language-settings", "文字来源")
            .child(settings_row(
                "subtitle-language-label",
                icons::subtitles(),
                "字幕语言",
                "",
                self.setting_choices("default-subtitle-language", "字幕语言")
                    .options([
                        ("", "自动"),
                        ("zh-Hans", "简体"),
                        ("zh-Hant", "繁體"),
                        ("en", "英语"),
                        ("ja", "日语"),
                    ])
                    .full_width()
                    .selected(language.to_owned())
                    .on_change(cx.listener(|this, selected: &SharedString, window, cx| {
                        let mut next = this.generation_edit_base();
                        next.preferred_subtitle_languages = if selected.is_empty() {
                            Vec::new()
                        } else {
                            vec![selected.to_string()]
                        };
                        next.subtitle_languages_draft = None;
                        if this.commit_generation(next, cx) {
                            this.settings_ui.language_details_open = false;
                            this.settings_ui.inputs[&EditField::Languages]
                                .update(cx, |input, cx| {
                                    input.set_value(selected.clone(), window, cx)
                                });
                        }
                    })),
            ))
            // The custom order and its editor stay inside the 字幕语言 item.
            .detail(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .px(rems(20. / 14.))
                    .pb(rems(10. / 14.))
                    .gap_3()
                    .items_center()
                    .flex_wrap()
                    .child(
                        quiet("toggle-language-details")
                            // Optical alignment: the label starts on the row's text edge.
                            .ml(rems(-12. / 14.))
                            .icon(if self.settings_ui.language_details_open {
                                icons::chevron_up()
                            } else {
                                icons::chevron_down()
                            })
                            .label("自定义语言优先级")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.settings_ui.language_details_open =
                                    !this.settings_ui.language_details_open;
                                cx.notify();
                            })),
                    )
                    .when(language == "custom", |row| {
                        row.child(
                            text(
                                "subtitle-custom-summary",
                                value.preferred_subtitle_languages.join(" → "),
                            )
                            .text_size(TEXT_AUX)
                            .text_color(color(MUTED)),
                        )
                    }),
            )
            .detail(crate::motion::disclosure(
                "subtitle-language-details",
                self.settings_ui.language_details_open || value.subtitle_languages_draft.is_some(),
                v_flex()
                    .px(rems(20. / 14.))
                    .pb(rems(16. / 14.))
                    .child(
                        h_flex()
                            .w_full()
                            .min_w_0()
                            .gap_3()
                            .flex_wrap()
                            .child(
                                div().flex_1().min_w(rems(16.)).child(
                                    text_input(&self.settings_ui.inputs[&EditField::Languages])
                                        .w_full()
                                        .aria_label("语言优先级"),
                                ),
                            )
                            .child(
                                outline_pill("apply-languages")
                                    .icon(icons::check_circle())
                                    .label("应用")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.apply_language_preferences(cx)
                                    })),
                            ),
                    ),
                window,
                cx,
            ));
        let mut recognition = group("asr-default-settings", "语音识别").child(settings_row(
            "asr-method-label",
            icons::microphone(),
            "识别方式",
            if provider == "api" {
                "音频会发送到所选服务"
            } else {
                ""
            },
            self.setting_choices("default-asr-device", "默认语音识别方式")
                .options([("local", "本机识别"), ("api", "在线语音服务")])
                .full_width()
                .selected(if provider == "api" { "api" } else { "local" })
                .on_change(cx.listener(|this, selected: &SharedString, _, cx| {
                    let mut next = this.generation_edit_base();
                    next.select_recognition_location(selected.as_ref() == "api");
                    this.commit_generation(next, cx);
                })),
        ));
        if provider == "api" {
            recognition = recognition.child(self.service_picker(ServicePurpose::Speech, false, cx));
        } else {
            recognition =
                recognition
                    .child(self.local_model_picker(cx))
                    .detail(crate::motion::disclosure(
                        "asr-advanced-settings",
                        self.settings_ui.asr_details_open,
                        settings_row(
                            "asr-hardware-heading",
                            icons::computer(),
                            "本机引擎",
                            "",
                            self.setting_choices("default-asr-hardware", "本机引擎")
                                .options(
                                    [
                                        ("", "自动"),
                                        ("coreml", "Apple 原生"),
                                        ("gpu", "GPU"),
                                        ("cpu", "CPU"),
                                        ("npu", "Intel NPU"),
                                    ]
                                    .into_iter()
                                    .filter(|(id, _)| {
                                        *id != "coreml"
                                            || cfg!(target_os = "macos")
                                            || provider == "coreml"
                                    })
                                    .filter(|(id, _)| {
                                        *id != "npu"
                                            || !cfg!(target_os = "macos")
                                            || provider == "npu"
                                    }),
                                )
                                .full_width()
                                .selected(provider.to_owned())
                                .on_change(cx.listener(|this, id: &SharedString, _, cx| {
                                    let mut next = this.generation_edit_base();
                                    next.select_provider(match id.as_ref() {
                                        "coreml" => Some(course2md::config::AsrProvider::Coreml),
                                        "gpu" => Some(course2md::config::AsrProvider::Gpu),
                                        "cpu" => Some(course2md::config::AsrProvider::Cpu),
                                        "npu" => Some(course2md::config::AsrProvider::Npu),
                                        _ => None,
                                    });
                                    this.commit_generation(next, cx);
                                })),
                        ),
                        window,
                        cx,
                    ));
            let model_panel = self.default_model_readiness_panel(window, cx);
            recognition = recognition.detail(crate::motion::disclosure(
                "default-model-management",
                self.settings_ui.model_details_open,
                model_panel,
                window,
                cx,
            ));
        }
        let mut ai = group("ai-default-settings", "AI 校对与摘要")
            .child(self.service_picker(ServicePurpose::Ai, false, cx))
            .child(
                self.setting_preference(
                    icons::auto_fix(),
                    "AI 校对",
                    "",
                    Switch::new("default-ai-proofread")
                        .checked(value.ai_proofread)
                        .on_click(cx.listener(|this, enabled, _, cx| {
                            let mut next = this.generation_edit_base();
                            next.ai_proofread = *enabled;
                            this.commit_generation(next, cx);
                        })),
                ),
            )
            .child(
                self.setting_preference(
                    icons::summarize(),
                    "生成摘要",
                    "",
                    Switch::new("default-ai-summary")
                        .checked(value.ai_summary)
                        .on_click(cx.listener(|this, enabled, _, cx| {
                            let mut next = this.generation_edit_base();
                            next.ai_summary = *enabled;
                            this.commit_generation(next, cx);
                        })),
                ),
            );
        let mut ai_options = v_flex().gap_3();
        if value.ai_proofread {
            ai_options = ai_options.child(
                self.setting_preference(
                    icons::image(),
                    "发送截图辅助校对",
                    "",
                    Switch::new("default-ai-vision")
                        .checked(value.vision)
                        .on_click(cx.listener(|this, enabled, _, cx| {
                            let mut next = this.generation_edit_base();
                            next.vision = *enabled;
                            this.commit_generation(next, cx);
                        })),
                ),
            );
        }
        ai = ai
            .detail(crate::motion::disclosure(
                "ai-default-options",
                value.needs_ai(),
                ai_options,
                window,
                cx,
            ));
        // The rule only applies to proofreading, so it appears with it.
        if value.ai_proofread {
            ai = ai
            // As on the canvas, the rule is a list item showing its current value.
            .child(settings_row(
                "proofread-rules-label",
                icons::edit(),
                "自定义校对规则",
                "",
                quiet("show-proofread-rules")
                    .mr(-crate::theme::QUIET_INSET)
                    .label(if value.prompt.is_some() { "已自定义" } else { "使用标准规则" })
                    .icon(if self.settings_ui.show_prompt {
                        icons::chevron_up()
                    } else {
                        icons::chevron_down()
                    })
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.settings_ui.show_prompt = !this.settings_ui.show_prompt;
                        // The standard rule is edited in place rather than printed beside an empty box.
                        if this.settings_ui.show_prompt
                            && this.settings_ui.prompt.read(cx).value().trim().is_empty()
                        {
                            this.settings_ui.prompt.update(cx, |input, cx| {
                                input.set_value(course2md::llm::DEFAULT_PROMPT, window, cx)
                            });
                        }
                        cx.notify();
                    })),
            ))
            .detail(crate::motion::disclosure(
                "proofread-rules-content",
                self.settings_ui.show_prompt,
                v_flex()
                    .px(rems(20. / 14.))
                    .pb(rems(16. / 14.))
                    .gap_3()
                    .child(
                        Textarea::new(&self.settings_ui.prompt)
                            .h(px(160.))
                            .w_full()
                            .aria_label("自定义校对规则"),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .flex_wrap()
                            .child(
                                outline_pill("apply-proofread-rules")
                                    .icon(icons::save())
                                    .label("应用校对规则")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        let prompt = this
                                            .settings_ui
                                            .prompt
                                            .read(cx)
                                            .value()
                                            .trim()
                                            .to_owned();
                                        let mut next = this.generation_edit_base();
                                        next.prompt = (!prompt.is_empty()
                                            && prompt != course2md::llm::DEFAULT_PROMPT.trim())
                                        .then_some(prompt);
                                        next.prompt_draft = None;
                                        this.commit_generation(next, cx);
                                    })),
                            )
                            .child(
                                quiet("restore-proofread-rules")
                                    .icon(icons::refresh())
                                    .label("恢复标准规则")
                                    .disabled(value.prompt.is_none())
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        let mut next = this.generation_edit_base();
                                        next.prompt = None;
                                        next.prompt_draft = None;
                                        if this.commit_generation(next, cx) {
                                            this.settings_ui.prompt.update(cx, |input, cx| {
                                                input.set_value(
                                                    course2md::llm::DEFAULT_PROMPT,
                                                    window,
                                                    cx,
                                                )
                                            });
                                        }
                                    })),
                            ),
                    ),
                window,
                cx,
            ));
        }
        let selected = value.options.formats.clone().unwrap_or_default();
        v_flex()
            .w_full()
            .min_w_0()
            .flex_shrink_0()
            .gap_6()
            .child(languages)
            .child(recognition)
            .child(ai)
            .child(
                group("export-default-settings", "导出与离线保存")
                    .child(settings_row(
                        "export-format-label",
                        icons::download(),
                        "同时导出",
                        "",
                        h_flex().gap_3().flex_wrap().children(
                            [
                                (course2md::config::OutputFormat::Md, "Markdown 包"),
                                (course2md::config::OutputFormat::Html, "网页文件"),
                                (course2md::config::OutputFormat::Json, "JSON 数据"),
                            ]
                            .into_iter()
                            .enumerate()
                            .map(|(index, (format, label))| {
                                Checkbox::new(("default-export", index))
                                    .label(label)
                                    .checked(selected.contains(&format))
                                    .min_h(rems(2.6))
                                    .items_center()
                                    .on_click(cx.listener(move |this, enabled, _, cx| {
                                        let mut next = this.generation_edit_base();
                                        let formats =
                                            next.options.formats.get_or_insert_with(Vec::new);
                                        if !enabled {
                                            formats.retain(|value| *value != format);
                                        } else if !formats.contains(&format) {
                                            formats.push(format);
                                        }
                                        this.commit_generation(next, cx);
                                    }))
                            }),
                        ),
                    ))
                    .child(
                        self.setting_preference(
                            icons::movie(),
                            "保留视频供离线播放",
                            "",
                            Switch::new("default-keep-video")
                                .checked(value.options.keep_video.unwrap_or(false))
                                .on_click(cx.listener(|this, enabled, _, cx| {
                                    let mut next = this.generation_edit_base();
                                    next.options.keep_video = Some(*enabled);
                                    this.commit_generation(next, cx);
                                })),
                        ),
                    ),
            )
            .into_any_element()
    }

    fn local_model_picker(&self, cx: &mut Context<Self>) -> Div {
        use course2md::config::AsrProvider;
        let preferences = self.preferences.generation();
        let provider = preferences.options.provider;
        let selected = preferences
            .options
            .asr_model
            .as_deref()
            .unwrap_or(course2md::config::DEFAULT_ASR_MODEL);
        let mut models = vec![(course2md::config::DEFAULT_ASR_MODEL, "Qwen3 1.7B")];
        if provider == Some(AsrProvider::Coreml)
            || (provider.is_none() && cfg!(target_os = "macos"))
            || provider == Some(AsrProvider::Npu)
        {
            models.extend([("qwen3-0.6b", "Qwen3 0.6B"), ("whisper", "Whisper")]);
        }
        if provider == Some(AsrProvider::Npu) {
            models.extend([
                ("whisper-tiny", "Whisper Tiny"),
                ("whisper-base", "Whisper Base"),
                ("whisper-small", "Whisper Small"),
            ]);
        }
        let known = models.iter().any(|(id, _)| *id == selected);
        let picker = if models.len() == 1 {
            // 只有一个可选模型时不是「选择器」：展示静态值，不伪装可选择（settings#2）
            let (_, label) = models[0];
            settings_value("default-local-model-single", label.to_owned())
                .into_any_element()
        } else if models.len() > 3 {
            let current = selected.to_owned();
            let label = models
                .iter()
                .find(|(id, _)| *id == selected)
                .map(|(_, label)| *label)
                .unwrap_or("自定义模型");
            let entity = cx.entity().downgrade();
            self.reveal_setting(
                "default-local-model-menu-reveal",
                control("default-local-model-menu")
                    .w_full()
                    .label(label)
                    .child(Icon::new(IconName::ChevronDown).size_4())
                    .dropdown_menu(move |menu, _, _| {
                        models.iter().fold(menu, |menu, (id, label)| {
                            let id = (*id).to_owned();
                            let entity = entity.clone();
                            menu.item(PopupMenuItem::new(*label).checked(id == current).on_click(
                                move |_, window, cx| {
                                    let _ =
                                        entity.update(cx, |this, cx| {
                                            let mut next = this.generation_edit_base();
                                            next.options.asr_model = Some(id.clone());
                                            next.local_model_draft = None;
                                            if this.commit_generation(next, cx) {
                                                this.settings_ui.inputs[&EditField::LocalModel]
                                                    .update(cx, |input, cx| {
                                                        input.set_value(id.clone(), window, cx);
                                                    });
                                            }
                                        });
                                },
                            ))
                        })
                    }),
            )
            .into_any_element()
        } else {
            self.setting_choices("default-local-model", "默认识别模型")
                .options(models)
                .full_width()
                .selected(selected.to_owned())
                .on_change(cx.listener(move |this, id: &SharedString, window, cx| {
                    let mut next = this.generation_edit_base();
                    next.options.asr_model = Some(id.to_string());
                    next.local_model_draft = None;
                    if this.commit_generation(next, cx) {
                        this.settings_ui.inputs[&EditField::LocalModel]
                            .update(cx, |input, cx| input.set_value(id.clone(), window, cx));
                    }
                }))
                .into_any_element()
        };
        let (settled, conclusion) = self.default_model_conclusion();
        // Status and its actions span the item below the label/control row,
        // starting on the label ruler like every other setting's help.
        let model_footer = h_flex()
            .w_full()
            .min_w_0()
            .px(rems(20. / 14.))
            .pb(rems(12. / 14.))
            .gap_3()
            .items_center()
            .flex_wrap()
            .child(
                h_flex()
                    .flex_1()
                    .min_w(rems(12.))
                    .gap_2()
                    .items_center()
                    // A routine state needs no sentence; only a problem is named here.
                    .when(!settled, |status| {
                        status
                            .child(
                                icons::warning()
                                    .size(rems(16. / 14.))
                                    .text_color(color(WARNING)),
                            )
                            .child(
                                text("model-readiness-conclusion", conclusion)
                                    .text_size(TEXT_AUX)
                                    .text_color(color(WARNING)),
                            )
                    }),
            )
            .child(
                quiet("toggle-model-details")
                    .icon(if self.settings_ui.model_details_open {
                        icons::chevron_up()
                    } else {
                        icons::download()
                    })
                    .label(if self.settings_ui.model_details_open {
                        "收起模型管理"
                    } else {
                        "管理模型与下载"
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.settings_ui.model_details_open = !this.settings_ui.model_details_open;
                        cx.notify();
                    })),
            )
            .child(
                quiet("toggle-asr-advanced")
                    .mr(-crate::theme::QUIET_INSET)
                    .icon(icons::tune())
                    .label(if self.settings_ui.asr_details_open {
                        "收起高级设置"
                    } else {
                        "高级识别设置"
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.settings_ui.asr_details_open = !this.settings_ui.asr_details_open;
                        cx.notify();
                    })),
            );
        let mut view = v_flex()
            .w_full()
            .min_w_0()
            .child(settings_row("local-model-heading", icons::storage(), "识别模型", "", picker))
            .child(model_footer);
        if !known && provider != Some(AsrProvider::Npu) {
            view = view.child(text("unavailable-fixed-model", format!("{selected} 不适用于当前识别方式，请重新选择")).text_sm().text_color(color(DANGER)));
        }
        if provider == Some(AsrProvider::Npu) {
            view = view
                .child(self.setting_field(EditField::LocalModel, "NPU 模型 ID 或仓库 ID", cx))
                .child(
                    control("apply-custom-npu-model")
                        .icon(icons::check_circle())
                        .label("应用模型选择")
                        .self_start()
                        .on_click(cx.listener(|this, _, _, cx| {
                            let model = this
                                .setting_value(EditField::LocalModel, cx)
                                .trim()
                                .to_owned();
                            let mut next = this.generation_edit_base();
                            next.options.asr_model = (!model.is_empty()).then_some(model);
                            next.local_model_draft = None;
                            this.commit_generation(next, cx);
                        })),
                );
        }
        if let Some(environment) = &self.environment {
            let ready = match provider {
                Some(AsrProvider::Coreml) => environment.apple && environment.engine,
                Some(AsrProvider::Gpu) => {
                    environment.gpu.is_some() && environment.llama && environment.engine
                }
                Some(AsrProvider::Cpu) => environment.llama && environment.engine,
                Some(AsrProvider::Npu) => environment.npu && environment.engine,
                _ => true,
            };
            if !ready {
                view = view.child(text("selected-device-unavailable", "所选识别方式的运行环境未就绪，详见「应用」诊断").text_sm().text_color(color(WARNING)));
            }
        }
        view
    }
}
