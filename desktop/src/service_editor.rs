//! Service editor: picker, editor dialog, connection testing and publishing.
use super::*;

impl Desktop {
    /// Ordinary service editing with explicit Save and Cancel actions.
    pub(super) fn service_editor_card(&self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let Some(editor) = &self.settings_ui.editor else {
            return v_flex();
        };
        let purpose = editor.draft.protocol.purpose();
        let kind = if purpose == ServicePurpose::Speech {
            "语音服务"
        } else {
            "AI 服务"
        };
        let title = if editor.draft.based_on.is_some() {
            format!("编辑 {kind}")
        } else {
            format!("添加 {kind}")
        };
        v_flex()
            .w_full()
            .h_full()
            .min_h_0()
            .gap_3()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .flex_shrink_0()
                    .child(
                        (if purpose == ServicePurpose::Speech {
                            icons::microphone()
                        } else {
                            icons::science()
                        })
                        .size(px(20.))
                        .text_color(color(ACCENT_STRONG)),
                    )
                    .child(
                        text("service-editor-inline-title", title)
                            .text_size(TEXT_TITLE)
                            .font_weight(FontWeight::SEMIBOLD),
                    ),
            )
            .child(self.service_editor_content(true, window, cx))
    }
    pub fn task_service_picker(&self, purpose: ServicePurpose, cx: &mut Context<Self>) -> Div {
        self.service_picker(purpose, true, cx)
    }
    pub fn selected_task_service(&self, purpose: ServicePurpose) -> Option<ServiceVersion> {
        let fixed = self
            .workspace
            .as_ref()
            .and_then(|w| w.state.draft())
            .and_then(|draft| match purpose {
                ServicePurpose::Speech => draft.asr_service.clone(),
                ServicePurpose::Ai => draft.ai_service.clone(),
            });
        let defaults = self.preferences.default_refs();
        let id = fixed.or(match purpose {
            ServicePurpose::Speech => defaults.asr,
            ServicePurpose::Ai => defaults.llm,
        })?;
        self.preferences.version(&id).cloned()
    }
    pub(super) fn service_picker(
        &self,
        purpose: ServicePurpose,
        current_task: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let refs = self.preferences.default_refs();
        let current = if current_task {
            self.selected_task_service(purpose).map(|v| v.id)
        } else {
            match purpose {
                ServicePurpose::Speech => refs.asr,
                ServicePurpose::Ai => refs.llm,
            }
        };
        let editor_version = current
            .as_deref()
            .and_then(|id| self.preferences.version(id))
            .filter(|version| {
                !self
                    .preferences
                    .service_retired_in_snapshot(&version.service_id)
            })
            .map(|version| version.id.clone());
        let latest: BTreeMap<String, ServiceVersion> = self
            .preferences
            .latest_versions()
            .into_iter()
            .filter(|(_, v)| {
                v.config.protocol.purpose() == purpose
                    && !self.preferences.service_retired_in_snapshot(&v.service_id)
            })
            .collect();
        let mut view = v_flex().w_full().min_w_0().gap_2();
        let current_version = current
            .as_deref()
            .and_then(|id| self.preferences.version(id))
            .filter(|version| {
                !self
                    .preferences
                    .service_retired_in_snapshot(&version.service_id)
            })
            .cloned();
        let old_current = current_version
            .as_ref()
            .filter(|version| {
                latest
                    .get(&version.service_id)
                    .is_some_and(|latest| latest.id != version.id)
            })
            .cloned();
        let mut choices = Vec::new();
        for version in latest.into_values() {
            if let Some(old) = old_current
                .as_ref()
                .filter(|old| old.service_id == version.service_id)
            {
                choices.push(old.clone());
            }
            choices.push(version);
        }
        let mut controls = h_flex().w_full().min_w_0().items_start().gap_2();
        if !choices.is_empty() {
            let index = purpose as usize * 2 + current_task as usize;
            let name = current_version
                .as_ref()
                .map(|version| version.config.name.clone())
                .unwrap_or_else(|| "选择服务".into());
            let detail = current_version.as_ref().map(|version| {
                let state = if old_current.is_some() {
                    format!(" · 当前使用旧配置 v{}", version.number)
                } else {
                    String::new()
                };
                format!("{}{state}", version.config.model)
            });
            let menu_choices = choices
                .into_iter()
                .map(|version| {
                    let state = if let Some(old) = old_current
                        .as_ref()
                        .filter(|old| old.service_id == version.service_id)
                    {
                        if old.id == version.id {
                            format!("当前使用 · 旧配置 v{}", version.number)
                        } else {
                            let changed = version.changes_since(old);
                            format!(
                                "新版 v{} · {}",
                                version.number,
                                if changed.is_empty() {
                                    "配置内容相同".to_owned()
                                } else {
                                    format!("{}已更新", changed.join("、"))
                                }
                            )
                        }
                    } else if current.as_ref() == Some(&version.id) {
                        "当前使用".into()
                    } else {
                        String::new()
                    };
                    (version, state)
                })
                .collect::<Vec<_>>();
            let entity = cx.entity().downgrade();
            let picker = control(("service-choice", index))
                .w_full()
                .min_w_0()
                .h_auto()
                .py_2()
                .accessibility_label(format!(
                    "{}：{}{}",
                    if purpose == ServicePurpose::Speech {
                        "语音服务"
                    } else {
                        "AI 服务"
                    },
                    name,
                    detail
                        .as_ref()
                        .map(|detail| format!("，{detail}"))
                        .unwrap_or_default()
                ))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .items_start()
                        .font_weight(FontWeight::NORMAL)
                        .child(
                            text(("service-choice-name", index), name)
                                .w_full()
                                .whitespace_normal(),
                        )
                        .when_some(detail, |view, detail| {
                            view.child(
                                text(("service-choice-detail", index), detail)
                                    .w_full()
                                    .whitespace_normal()
                                    .text_size(TEXT_AUX)
                                    .text_color(color(MUTED)),
                            )
                        }),
                )
                .child(Icon::new(IconName::ChevronDown).size_4().flex_shrink_0())
                .dropdown_menu(move |menu, window, _| {
                    let width = (window.bounds().size.width - px(48.)).min(px(560.));
                    menu_choices.iter().fold(
                        menu.min_w(width.min(px(360.)))
                            .max_w(width)
                            .scrollable(true),
                        |menu, (version, state)| {
                            let id = version.id.clone();
                            let name = version.config.name.clone();
                            let detail =
                                format!("{} · {}", version.config.model, version.config.endpoint);
                            let state = state.clone();
                            let entity = entity.clone();
                            menu.item(
                                PopupMenuItem::element(move |_, _| {
                                    v_flex()
                                        .flex_1()
                                        .min_w_0()
                                        .py_1()
                                        .gap_1()
                                        .whitespace_normal()
                                        .child(div().child(name.clone()))
                                        .child(div().text_size(TEXT_AUX).child(detail.clone()))
                                        .when(!state.is_empty(), |view| {
                                            view.child(
                                                div().text_size(TEXT_AUX).child(state.clone()),
                                            )
                                        })
                                })
                                .checked(current.as_ref() == Some(&id))
                                .on_click(move |_, _, cx| {
                                    let _ = entity.update(cx, |this, cx| {
                                        let selectable =
                                            this.preferences.version(&id).is_some_and(|version| {
                                                version.config.protocol.purpose() == purpose
                                                    && !this
                                                        .preferences
                                                        .is_service_retired(&version.service_id)
                                            });
                                        if selectable {
                                            this.bind_service(
                                                purpose,
                                                Some(id.clone()),
                                                current_task,
                                                cx,
                                            );
                                        }
                                    });
                                }),
                            )
                        },
                    )
                });
            controls = controls.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(self.reveal_setting(("service-choice-reveal", index), picker)),
            );
        }
        controls = controls.child(
            control((
                "configure-service",
                purpose as usize * 2 + current_task as usize,
            ))
            .icon(icons::settings())
            .ghost()
            .label(match (purpose, editor_version.is_some()) {
                (ServicePurpose::Speech, true) => "编辑",
                (ServicePurpose::Speech, false) => "添加语音服务",
                (ServicePurpose::Ai, true) => "编辑",
                (ServicePurpose::Ai, false) => "添加 AI 服务",
            })
            .self_start()
            .on_click(cx.listener(move |this, _, window, cx| {
                if current_task {
                    this.open_task_service_editor(purpose, window, cx)
                } else {
                    this.open_settings_service_editor(purpose, editor_version.clone(), window, cx);
                }
            })),
        );
        view = view.child(controls);
        if current_task
            && self
                .workspace
                .as_ref()
                .and_then(|w| w.state.draft())
                .is_some_and(|draft| match purpose {
                    ServicePurpose::Speech => draft.asr_service.is_some(),
                    ServicePurpose::Ai => draft.ai_service.is_some(),
                })
        {
            view = view.child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        text(
                            ("service-only-this-note", purpose as usize),
                            "仅用于这次笔记",
                        )
                        .text_sm()
                        .text_color(color(MUTED)),
                    )
                    .child(
                        control(("inherit-default-service", purpose as usize))
                            .ghost()
                            .icon(icons::refresh())
                            .label("恢复默认")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.bind_service(purpose, None, true, cx);
                            })),
                    ),
            );
        }
        if let Some((message, true, _)) = self.settings_ui.feedback.get(&PreferenceGroup::Services)
        {
            view = view.child(
                text(("task-service-error", purpose as usize), message.clone())
                    .text_sm()
                    .text_color(color(DANGER)),
            );
        }
        if current_task {
            view
        } else {
            settings_row(
                ("default-service-label", purpose as usize),
                if purpose == ServicePurpose::Speech {
                    icons::microphone()
                } else {
                    icons::science()
                },
                if purpose == ServicePurpose::Speech {
                    "语音服务"
                } else {
                    "AI 服务"
                },
                if purpose == ServicePurpose::Ai && !self.preferences.generation().needs_ai() {
                    "开启校对或摘要后，文字会发送到所选服务。"
                } else if purpose == ServicePurpose::Ai {
                    "校对与摘要会把文字发送到所选服务。"
                } else {
                    ""
                },
                view,
            )
        }
    }
    fn bind_service(
        &mut self,
        purpose: ServicePurpose,
        id: Option<String>,
        current_task: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let result = if current_task {
            self.workspace
                .as_mut()
                .ok_or_else(|| anyhow!("当前笔记信息暂时不可用"))
                .and_then(|workspace| {
                    workspace.transaction(|state| {
                        let draft = state.draft_mut().context("找不到当前笔记")?;
                        match purpose {
                            ServicePurpose::Speech => draft.asr_service = id,
                            ServicePurpose::Ai => draft.ai_service = id,
                        };
                        Ok(())
                    })
                })
        } else {
            self.preferences.set_default_service(purpose, id.as_deref())
        };
        match result {
            Ok(()) => {
                self.set_settings_feedback(
                    PreferenceGroup::Services,
                    "服务选择已保存".into(),
                    false,
                );
                self.advance_conversion_when_ready(cx);
                cx.notify();
                true
            }
            Err(error) => {
                self.set_settings_feedback(
                    PreferenceGroup::Services,
                    format!("服务选择尚未保存：{error:#}"),
                    true,
                );
                cx.notify();
                false
            }
        }
    }

    pub(crate) fn open_task_service_editor(
        &mut self,
        purpose: ServicePurpose,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.save_current_draft(cx) {
            return;
        }
        let Some(target) = self
            .workspace
            .as_ref()
            .and_then(|w| w.state.draft())
            .map(|d| d.id.clone())
        else {
            return;
        };
        let draft = self
            .selected_task_service(purpose)
            .filter(|version| {
                !self
                    .preferences
                    .service_retired_in_snapshot(&version.service_id)
            })
            .map(|v| ServiceDraft::from_version(&v))
            .unwrap_or_else(|| ServiceDraft::new(purpose));
        self.open_service_draft(draft, Some(target), None, window, cx);
    }
    pub(crate) fn open_reprocess_service_editor(
        &mut self,
        task_id: String,
        components: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(task) = self
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.state.tasks.iter().find(|task| task.id == task_id))
        else {
            self.workspace_error = Some("原任务已不可用，尚未修改服务或创建补做任务。".into());
            cx.notify();
            return;
        };
        let draft = task
            .plan
            .ai_service
            .as_deref()
            .and_then(|id| self.preferences.version(id))
            .filter(|version| {
                !self
                    .preferences
                    .service_retired_in_snapshot(&version.service_id)
            })
            .map(ServiceDraft::from_version)
            .unwrap_or_else(|| ServiceDraft::new(ServicePurpose::Ai));
        self.open_service_draft(draft, None, Some((task_id, components)), window, cx);
    }
    pub(crate) fn open_settings_service_editor(
        &mut self,
        purpose: ServicePurpose,
        version: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let draft = version
            .as_deref()
            .and_then(|id| self.preferences.version(id))
            .map(ServiceDraft::from_version)
            .unwrap_or_else(|| ServiceDraft::new(purpose));
        self.open_service_draft(draft, None, None, window, cx)
    }
    pub(crate) fn open_service_draft(
        &mut self,
        draft: ServiceDraft,
        target: Option<String>,
        repair_task: Option<(String, Vec<String>)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if let Some(editor) = &self.settings_ui.editor {
            if editor.target.is_some() || editor.repair_task.is_some() {
                return false;
            }
            if editor.draft.id == draft.id
                && editor.target == target
                && editor.repair_task == repair_task
            {
                self.settings_tab = 1;
                self.navigate(Page::Settings, cx);
                self.prepare_settings_view(window, cx);
                self.scrolls[Page::Settings as usize].set_offset(point(px(0.), px(0.)));
                self.settings_ui.inputs[&EditField::Name]
                    .update(cx, |input, cx| input.focus(window, cx));
                return true;
            }
            // A second action must not reuse the previous editor's endpoint or scope.
            // Switching forms cancels the earlier edit; only Save can publish it.
            if !self.close_service_editor(window, cx) {
                self.settings_tab = 1;
                self.navigate(Page::Settings, cx);
                self.prepare_settings_view(window, cx);
                self.scrolls[Page::Settings as usize].set_offset(point(px(0.), px(0.)));
                return false;
            }
        }
        let title = if repair_task.is_some() {
            "修复服务并补做"
        } else {
            match (draft.protocol.purpose(), target.is_some()) {
                (ServicePurpose::Speech, true) => "为这次笔记设置语音服务",
                (ServicePurpose::Ai, true) => "为这次笔记设置 AI 服务",
                (ServicePurpose::Speech, false) => "设置语音服务",
                (ServicePurpose::Ai, false) => "设置 AI 服务",
            }
        };
        for (field, value) in [
            (EditField::Name, draft.name.clone()),
            (EditField::Address, draft.address.clone()),
            (EditField::Model, draft.model.clone()),
            (EditField::Key, String::new()),
        ] {
            self.settings_ui.inputs[&field]
                .update(cx, |input, cx| input.set_value(value, window, cx));
        }
        self.settings_ui.inputs[&EditField::Key]
            .update(cx, |input, cx| input.set_masked(true, window, cx));
        let kind = if draft.protocol.purpose() == ServicePurpose::Speech {
            TestKind::Speech
        } else {
            TestKind::Proofread
        };
        let evidence = draft.configuration().ok().and_then(|config| {
            self.preferences
                .test_evidence(&config, kind.contract())
                .cloned()
        });
        let inline = target.is_none() && repair_task.is_none();
        let defaults = self.preferences.default_refs();
        let default = match draft.protocol.purpose() {
            ServicePurpose::Speech => defaults.asr,
            ServicePurpose::Ai => defaults.llm,
        };
        let also_default = default
            .as_deref()
            .and_then(|id| self.preferences.version(id))
            .is_none_or(|version| {
                self.preferences
                    .service_retired_in_snapshot(&version.service_id)
            });
        let draft_protocol = draft.protocol;
        self.settings_ui.editor = Some(ServiceEditor {
            draft,
            models: Default::default(),
            target,
            repair_task,
            scroll: ScrollHandle::new(),
            also_default,
            return_focus: window.focused(cx),
            errors: Vec::new(),
            status: None,
            test_kind: kind,
            test_running: None,
            evidence,
            received_tests: BTreeMap::new(),
            pending_binding: None,
            saved_configuration: None,
            show_key: false,
            test_details_open: false,
            save_failed: false,
        });
        if draft_protocol == ServiceProtocol::CodexResponses {
            self.codex_refresh_status(crate::codex_ui::CodexSurface::Editor, cx);
        }
        if inline {
            self.settings_tab = 1;
            self.navigate(Page::Settings, cx);
            self.prepare_settings_view(window, cx);
            self.scrolls[Page::Settings as usize].set_offset(point(px(0.), px(0.)));
            self.settings_ui.inputs[&EditField::Name]
                .update(cx, |input, cx| input.focus(window, cx));
            cx.notify();
            return true;
        }
        let desktop = cx.entity();
        let content = cx.new(|cx| ServiceDialog {
            _observation: cx.observe(&desktop, |_, _, cx| cx.notify()),
            desktop,
        });
        let weak = cx.weak_entity();
        window.open_dialog(cx, move |sheet, window, _| {
            let closed = weak.clone();
            let cancel = weak.clone();
            let submit = weak.clone();
            sheet
                .title(title)
                .w((window.rem_size() * (620. / 14.)).min(window.viewport_size().width - px(48.)))
                .margin_top(task_dialog_top(window))
                .overlay_closable(false)
                .close_button(false)
                .child(content.clone())
                .on_ok(move |_, window, cx| {
                    let _ = submit.update(cx, |this, cx| this.publish_open_service(window, cx));
                    false
                })
                .on_cancel(move |_, window, cx| {
                    cancel
                        .update(cx, |this, cx| this.close_service_editor(window, cx))
                        .unwrap_or(true)
                })
                .on_close(move |_, window, cx| {
                    let _ = closed.update(cx, |this, cx| this.restore_service_focus(window, cx));
                })
        });
        self.settings_ui.inputs[&EditField::Address]
            .update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
        true
    }
    pub(super) fn service_editor_content(
        &self,
        inline: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(editor) = &self.settings_ui.editor else {
            return div().into_any_element();
        };
        let protocol = editor.draft.protocol;
        let auth = editor.draft.authentication;
        let busy = editor.test_running.is_some();
        let awaiting_binding = editor.pending_binding.is_some();
        let configuration_saved = editor.saved_configuration.is_some() || awaiting_binding;
        let repair_submit_label = if awaiting_binding {
            "重试补做"
        } else if configuration_saved {
            "开始补做"
        } else {
            "保存并补做"
        };
        let repair_refusal = self.repair_service_refusal(cx);
        let fixed_test_failure = if busy {
            None
        } else {
            repair_refusal.clone().or_else(|| {
                editor
                    .evidence
                    .as_ref()
                    .filter(|evidence| evidence.outcome != preferences::TestOutcome::Passed)
                    .map(|evidence| (editor.test_kind, evidence.clone()))
            })
        };
        let test_label = match editor.test_kind {
            TestKind::Speech => "语音",
            TestKind::Proofread => "校对",
            TestKind::Summary => "摘要",
            TestKind::Vision => "截图校对",
        };
        let test_outcome_unknown = editor
            .evidence
            .as_ref()
            .is_some_and(|evidence| evidence.outcome == preferences::TestOutcome::OutcomeUnknown);
        let mut view = v_flex()
            .id("service-editor-body")
            .w_full()
            .min_w_0()
            .min_h_0()
            .flex_1()
            .overflow_y_scroll()
            .track_scroll(&editor.scroll)
            .role(if inline { Role::Group } else { Role::Dialog })
            .aria_label(if protocol.purpose() == ServicePurpose::Speech {
                "设置语音服务"
            } else {
                "设置 AI 服务"
            })
            .gap_4()
            .px_1()
            // 滚动容器为末行字段的边框/焦点外绘留出空间（system.md：clipped container 需预留 outward paint）
            .pb_3();
        view = view.child(
            text(
                "service-editor-scope",
                if editor.repair_task.is_some() {
                    if configuration_saved {
                        format!(
                            "配置已保存。点击「{repair_submit_label}」后才重新发送这篇笔记的失败部分；默认服务保持不变。"
                        )
                    } else {
                        "仅保存不会开始补做；点击「保存并补做」后才重新发送这篇笔记的失败部分。默认服务保持不变。".to_owned()
                    }
                } else if editor.target.is_some() {
                    "保存后用于本次笔记。已提交的任务不受影响。".to_owned()
                } else {
                    "保存后设为默认服务。已提交的任务不受影响。".to_owned()
                },
            )
            .text_sm()
            .text_color(color(MUTED)),
        );
        view = view
            .child(
                settings_detail_group("service-connection-heading", icons::cloud(), "连接信息")
                    .child(self.setting_field(EditField::Name, "服务名称", cx)),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(setting_label(
                        "service-protocol-heading",
                        icons::cloud(),
                        if protocol.purpose() == ServicePurpose::Speech {
                            "接口类型"
                        } else {
                            "服务类型"
                        },
                    ))
                    .child(
                        self.setting_choices("service-protocol", "服务接口类型")
                            .options(
                                [
                                    ServiceProtocol::SpeechTranscriptions,
                                    ServiceProtocol::SpeechChat,
                                    ServiceProtocol::AiChat,
                                    ServiceProtocol::OllamaChat,
                                    ServiceProtocol::CodexResponses,
                                ]
                                .into_iter()
                                .filter(|candidate| candidate.purpose() == protocol.purpose())
                                .map(|candidate| {
                                    let short = if candidate.purpose() == ServicePurpose::Ai {
                                        candidate.ai_kind_label()
                                    } else {
                                        service_protocol_label(candidate)
                                    };
                                    (candidate.label(), short)
                                }),
                            )
                            .selected(protocol.label())
                            .disabled(awaiting_binding)
                            .on_change(cx.listener(move |this, selected: &SharedString, window, cx| {
                                let Some(candidate) = [
                                    ServiceProtocol::SpeechTranscriptions,
                                    ServiceProtocol::SpeechChat,
                                    ServiceProtocol::AiChat,
                                    ServiceProtocol::OllamaChat,
                                    ServiceProtocol::CodexResponses,
                                ]
                                .into_iter()
                                .find(|candidate| candidate.label() == selected.as_ref()) else {
                                    return;
                                };
                                this.service_protocol_switched(candidate, window, cx);
                            })),
                    ),
            );
        // Codex 端点固定、凭据由登录态持有：不展示地址与认证编辑
        if protocol != ServiceProtocol::CodexResponses {
            view = view.child(self.setting_field(EditField::Address, "服务地址", cx));
        } else {
            view = view.child(
                theme::supporting_info("service-codex-endpoint", crate::codex_ui::ENDPOINT_NOTE)
                    .w_full()
                    .min_w_0()
                    .whitespace_normal(),
            );
        }
        let address = self.setting_value(EditField::Address, cx);
        if protocol != ServiceProtocol::CodexResponses
            && let Ok(endpoint) = preferences::normalize_endpoint(&address, protocol)
            && endpoint != address.trim()
        {
            view = view.child(
                text("service-actual-endpoint", format!("将请求 {endpoint}"))
                    .text_sm()
                    .text_color(color(MUTED)),
            );
        }
        if !protocol.keyless() {
            view = view.child(
            v_flex()
                .gap_2()
                .child(setting_label("service-auth-heading", icons::shield(), "认证方式"))
                .child(
                    div().w_full().max_w(rems(560. / 14.)).min_w_0().child(
                        self.setting_choices("service-auth-mode", "服务认证方式")
                            .options([("api_key", "API Key"), ("none", "无需认证")])
                            .full_width()
                            .selected(if auth == Authentication::ApiKey {
                                "api_key"
                            } else {
                                "none"
                            })
                            .disabled(awaiting_binding)
                            .on_change(cx.listener(move |this, selected: &SharedString, _, cx| {
                                let mode = if selected.as_ref() == "api_key" {
                                    Authentication::ApiKey
                                } else {
                                    Authentication::None
                                };
                                if let Some(editor) = &mut this.settings_ui.editor {
                                    editor.draft.authentication = mode;
                                    editor.models.invalidate();
                                    editor.errors.clear();
                                    editor.evidence = None;
                                    editor.saved_configuration = None;
                                }
                                cx.notify();
                            })),
                    ),
                ),
        );
        if auth == Authentication::ApiKey {
            view = view.child(self.setting_field(EditField::Key, "API Key", cx));
            for (index, name) in
                preferences::Store::available_environment_credentials(protocol.purpose())
                    .into_iter()
                    .enumerate()
            {
                view = view.child(
                    control(("capture-environment-key", index))
                        .icon(icons::content_copy())
                        .label(format!("使用环境变量 {name}"))
                        .disabled(awaiting_binding)
                        .self_start()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            let Some(draft) = this
                                .settings_ui
                                .editor
                                .as_ref()
                                .map(|editor| editor.draft.clone())
                            else {
                                return;
                            };
                            match this.preferences.capture_environment_credential(draft, name) {
                                Ok(draft) => {
                                    if let Some(editor) = &mut this.settings_ui.editor {
                                        editor.draft = draft;
                                        editor.models.invalidate();
                                        editor.evidence = None;
                                        editor.saved_configuration = None;
                                        editor.save_failed = false;
                                        editor.status =
                                            Some("已使用环境变量中的密钥，点击保存后生效".into());
                                    }
                                    this.settings_ui.inputs[&EditField::Key]
                                        .update(cx, |input, cx| input.set_value("", window, cx));
                                }
                                Err(error) => {
                                    if let Some(editor) = &mut this.settings_ui.editor {
                                        editor.status = Some(format!("凭据尚未保存：{error:#}"));
                                    }
                                }
                            }
                            cx.notify();
                        })),
                );
            }
            if editor.draft.credential.is_some() {
                view = view.child(
                    text(
                        "service-key-existing",
                        "API Key 已安全保存。填写新值可替换，留空保留原密钥。",
                    )
                    .text_sm()
                    .text_color(color(MUTED)),
                );
            }
            view = view.child(preference(
                None,
                "显示输入的密钥",
                "开启后输入的密钥明文可见。",
                Switch::new("toggle-service-key")
                    .checked(editor.show_key)
                    .on_click(cx.listener(|this, value, window, cx| {
                        if let Some(editor) = &mut this.settings_ui.editor {
                            editor.show_key = *value;
                            let masked = !editor.show_key;
                            this.settings_ui.inputs[&EditField::Key]
                                .update(cx, |input, cx| input.set_masked(masked, window, cx));
                        }
                        cx.notify();
                    })),
            ));
        }
        if let Some(source) = &editor.draft.credential_source {
            view = view.child(
                text(
                    "service-credential-source",
                    format!("密钥已安全保存 · 来源：{source}"),
                )
                .text_sm(),
            );
        }
        }
        // Codex 账号连接：登录态决定模型目录与可用性
        if protocol == ServiceProtocol::CodexResponses {
            view = view.child(self.codex_account_section(
                crate::codex_ui::CodexSurface::Editor,
                awaiting_binding,
                cx,
            ));
        }
        let model_error = editor.errors.iter().find(|error| error.field == "model");
        view = view.child(
            self.reveal_setting(
                ("setting-field-reveal", EditField::Model as usize),
                v_flex()
                    .w_full()
                    .min_w_0()
                    .gap_2()
                    .child(setting_label(
                        ("setting-field-label", EditField::Model as usize),
                        icons::storage(),
                        "模型 ID",
                    ))
                    .child(crate::model_discovery::model_field_with_error(
                        "service-models",
                        &self.settings_ui.inputs[&EditField::Model],
                        &editor.models,
                        awaiting_binding,
                        model_error.map(|error| error.message.as_str()),
                        cx.listener(|this, _, _, cx| this.fetch_service_models(cx)),
                        cx,
                    )),
            ),
        );
        if editor.target.is_some() {
            view = view.child(
                self.setting_preference(
                    icons::check_circle(),
                    "同时设为默认服务",
                    "后续转换自动使用",
                    Switch::new("service-also-default")
                        .checked(editor.also_default)
                        .disabled(awaiting_binding)
                        .on_click(cx.listener(|this, value, _, cx| {
                            if let Some(editor) = &mut this.settings_ui.editor {
                                editor.also_default = *value;
                            }
                            cx.notify();
                        })),
                ),
            );
        }
        view = view.child(self.service_test_section(configuration_saved, window, cx));
        if let Some(status) = &editor.status
            && !busy
        {
            view = view.child(text("service-editor-status", status.clone()).text_sm());
        }
        let actions = h_flex()
            .w_full()
            .flex_shrink_0()
            .gap_2()
            .flex_wrap()
            .child(
                quiet("close-service-editor")
                    .icon(icons::close())
                    .label(if configuration_saved {
                        "关闭"
                    } else {
                        "取消"
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if this.close_service_editor(window, cx) && !inline {
                            window.close_dialog(cx);
                        }
                    })),
            )
            .child(div().flex_1())
            .child(
                outline_pill("run-service-test")
                    .icon(icons::science())
                    .label(if busy {
                        format!("正在测试{test_label}…")
                    } else {
                        match editor
                            .evidence
                            .as_ref()
                            .map(|evidence| evidence.outcome.clone())
                        {
                            Some(preferences::TestOutcome::Passed) => {
                                format!("{test_label}通过 · 重测")
                            }
                            Some(preferences::TestOutcome::OutcomeUnknown) => {
                                format!("{test_label}结果未确认 · 重试")
                            }
                            Some(
                                preferences::TestOutcome::AuthenticationRefused
                                | preferences::TestOutcome::ModelRefused
                                | preferences::TestOutcome::ContractMismatch
                                | preferences::TestOutcome::SampleMismatch,
                            ) => format!("{test_label}未通过 · 重试"),
                            _ => format!("测试{test_label}"),
                        }
                    })
                    .loading(busy)
                    .disabled(busy || awaiting_binding)
                    .on_click(
                        cx.listener(|this, _, window, cx| this.start_service_test(window, cx)),
                    ),
            )
            .when(editor.repair_task.is_some(), |row| {
                row.child(
                    quiet("save-repair-service-only")
                        .icon(icons::save())
                        .label("仅保存")
                        .disabled(busy || awaiting_binding || editor.saved_configuration.is_some())
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.publish_service_editor(false, window, cx)
                        })),
                )
            })
            .child(
                primary_pill("save-service")
                    .icon(icons::save())
                    .label(if editor.repair_task.is_some() {
                        repair_submit_label
                    } else if awaiting_binding {
                        "重试保存本次选择"
                    } else {
                        "保存"
                    })
                    .disabled(editor.repair_task.is_some() && (busy || repair_refusal.is_some()))
                    .on_click(
                        cx.listener(|this, _, window, cx| this.publish_open_service(window, cx)),
                    ),
            );
        let footer = self.service_editor_footer(
            actions,
            fixed_test_failure,
            repair_submit_label,
            test_outcome_unknown,
            cx,
        );
        v_flex()
            .w_full()
            .min_w_0()
            .min_h_0()
            .gap_3()
            .when(inline, |view| view.h_full().flex_1())
            .when(!inline, |view| {
                view.max_h(
                    (window.viewport_size().height
                        - task_dialog_top(window)
                        - window.rem_size() * 4.
                        - px(64.))
                    .max(px(120.)),
                )
            })
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .child(view)
                    .child(crate::backend::vertical_scrollbar(&editor.scroll)),
            )
            .child(footer)
            .into_any_element()
    }

    /// Test-kind choice, busy state, result card and expandable details.
    fn service_test_section(
        &self,
        configuration_saved: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let Some(editor) = &self.settings_ui.editor else {
            return div();
        };
        let protocol = editor.draft.protocol;
        let busy = editor.test_running.is_some();
        let awaiting_binding = editor.pending_binding.is_some();
        let mut testing = settings_detail_group("service-test-heading", icons::science(), "检查服务")
            .flex_shrink_0()
            .pt_2();
        if protocol.purpose() == ServicePurpose::Ai {
            testing = testing.child(
                div().w_full().max_w(rems(560. / 14.)).min_w_0().child(
                    self.setting_choices("service-test-purpose", "要测试的服务能力")
                        .options(
                            [TestKind::Proofread, TestKind::Summary, TestKind::Vision]
                                .into_iter()
                                .map(|kind| (kind.contract(), kind.label())),
                        )
                        .full_width()
                        .selected(editor.test_kind.contract())
                        .disabled(busy)
                        .on_change(cx.listener(move |this, selected: &SharedString, _, cx| {
                            let Some(kind) =
                                [TestKind::Proofread, TestKind::Summary, TestKind::Vision]
                                    .into_iter()
                                    .find(|kind| kind.contract() == selected.as_ref())
                            else {
                                return;
                            };
                            let evidence = this
                                .settings_ui
                                .editor
                                .as_ref()
                                .filter(|_| this.setting_value(EditField::Key, cx).is_empty())
                                .and_then(|editor| {
                                    let config = editor.draft.configuration().ok()?;
                                    editor
                                        .received_tests
                                        .get(&config.fingerprint(kind.contract()))
                                        .or_else(|| {
                                            this.preferences.test_evidence(&config, kind.contract())
                                        })
                                        .cloned()
                                });
                            if let Some(editor) = &mut this.settings_ui.editor {
                                editor.test_kind = kind;
                                editor.evidence = evidence;
                            }
                            cx.notify();
                        })),
                ),
            );
        }
        if busy {
            testing = testing.child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(crate::motion::spinner("service-test-busy", cx))
                    .child(
                        text(
                            "service-test-busy-label",
                            editor
                                .status
                                .clone()
                                .unwrap_or_else(|| "正在测试服务…".into()),
                        )
                        .text_sm(),
                    ),
            );
        }
        if let Some(evidence) = &editor.evidence {
            let (kind, label) = match evidence.outcome {
                preferences::TestOutcome::Passed => (
                    BadgeKind::Success,
                    format!("{}测试通过", editor.test_kind.label()),
                ),
                preferences::TestOutcome::OutcomeUnknown => {
                    (BadgeKind::Warning, "结果尚未确认".to_owned())
                }
                preferences::TestOutcome::NotSent => (BadgeKind::Neutral, "测试未发送".to_owned()),
                _ => (BadgeKind::Danger, "测试未通过".to_owned()),
            };
            let mut result = v_flex()
                .gap_3()
                .p_4()
                .rounded(RADIUS_CARD)
                .bg(color(match evidence.outcome {
                    preferences::TestOutcome::Passed => SUCCESS_BG,
                    preferences::TestOutcome::OutcomeUnknown => WARNING_BG,
                    preferences::TestOutcome::NotSent => INSET,
                    _ => DANGER_BG,
                }))
                .child(badge(kind).child(label))
                .when(
                    evidence.outcome != preferences::TestOutcome::Passed,
                    |view| {
                        view.child(text("service-test-result", evidence.message.clone()).text_sm())
                    },
                );
            if evidence.outcome == preferences::TestOutcome::Passed {
                result = result.child(
                    text(
                        "service-tested-save-state",
                        if editor.repair_task.is_some() {
                            if configuration_saved {
                                "配置已保存，尚未补做这篇笔记。"
                            } else {
                                "测试不会保存配置或开始补做。"
                            }
                        } else if awaiting_binding {
                            "配置已保存，本次笔记的选择仍需重试保存。"
                        } else {
                            "保存服务后使用这些更改。"
                        },
                    )
                    .text_sm(),
                );
            }
            result = result.child(
                text(
                    "service-test-time",
                    format!(
                        "测试时间：{}",
                        crate::reader_navigation::timestamp_local(evidence.tested_at * 1000)
                    ),
                )
                .text_size(TEXT_AUX)
                .text_color(color(MUTED)),
            );
            if !evidence.details.is_empty() {
                result = result.child(
                    quiet("service-test-details")
                        .icon(icons::info())
                        .self_start()
                        .label(if editor.test_details_open {
                            "收起测试详情"
                        } else {
                            "查看测试详情"
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(editor) = &mut this.settings_ui.editor {
                                editor.test_details_open = !editor.test_details_open;
                            }
                            cx.notify();
                        })),
                );
                let details = v_flex()
                    .gap_2()
                    .children(evidence.details.iter().enumerate().map(|(index, detail)| {
                        text(("service-test-detail", index), detail.clone())
                            .text_sm()
                            .text_color(color(MUTED))
                    }));
                result = result.child(crate::motion::disclosure(
                    "service-test-details-content",
                    editor.test_details_open,
                    details,
                    window,
                    cx,
                ));
            }
            testing = testing.child(crate::motion::enter(
                SharedString::from(format!(
                    "service-test-result-{}-{:?}",
                    evidence.tested_at, evidence.outcome
                )),
                result));
        }
        testing
    }


    /// Failure reason, saved-only hint, billing notice and the action row.
    fn service_editor_footer(
        &self,
        actions: Div,
        fixed_test_failure: Option<(TestKind, preferences::ServiceTestEvidence)>,
        repair_submit_label: &'static str,
        test_outcome_unknown: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let repair_refused = self.repair_service_refusal(cx).is_some();
        let (configuration_saved, repair_task) = self
            .settings_ui
            .editor
            .as_ref()
            .map(|editor| {
                (
                    editor.saved_configuration.is_some() || editor.pending_binding.is_some(),
                    editor.repair_task.is_some(),
                )
            })
            .unwrap_or((false, false));
        v_flex()
            .w_full()
            .min_w_0()
            .flex_shrink_0()
            .gap_2()
            .pt_3()
            .border_t_1()
            .border_color(color(CARD_LINE))
            .when_some(fixed_test_failure, |footer, (kind, evidence)| {
                footer.child(
                    text(
                        "service-test-fixed-reason",
                        format!(
                            "{}：{}{}",
                            kind.label(),
                            evidence.message,
                            if repair_refused {
                                "。修改配置或重新测试后再补做。"
                            } else {
                                ""
                            }
                        ),
                    )
                    .role(Role::Status)
                    .text_size(TEXT_AUX)
                    .text_color(color(if matches!(
                        evidence.outcome,
                        preferences::TestOutcome::OutcomeUnknown
                            | preferences::TestOutcome::NotSent
                    ) {
                        WARNING
                    } else {
                        DANGER
                    })),
                )
            })
            .when(configuration_saved && repair_task, |footer| {
                footer.child(
                    text("service-config-saved-only", "服务配置已保存，尚未开始补做。")
                        .text_size(TEXT_AUX)
                        .text_color(color(MUTED)),
                )
            })
            .child(
                theme::supporting_info(
                    "service-test-notice",
                    if test_outcome_unknown && repair_task {
                        format!("上次测试结果未确认。重试测试会发送新测试请求；点击「{repair_submit_label}」会重新发送失败部分，服务可能计费。")
                    } else if test_outcome_unknown {
                        "上次测试结果未确认。重试会发送新的内置测试内容，服务可能按其规则计费。".to_owned()
                    } else if repair_task {
                        format!("测试使用内置内容；点击「{repair_submit_label}」会重新发送失败部分。服务可能按其规则计费。")
                    } else {
                        service_test::TEST_NOTICE.to_owned()
                    },
                )
                .text_size(TEXT_AUX),
            )
            .child(actions)
    }


    fn live_service_model_draft(&self, cx: &App) -> Option<ServiceDraft> {
        let mut draft = self.settings_ui.editor.as_ref()?.draft.clone();
        draft.address = self.setting_value(EditField::Address, cx);
        Some(draft)
    }

    /// 服务类型切换：无密钥协议收起认证编辑，并按协议预填/替换固定地址。
    fn service_protocol_switched(
        &mut self,
        candidate: ServiceProtocol,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let codex_addresses = [
            preferences::CODEX_ADDRESS,
            course2md::provider::CODEX_RESPONSES_URL,
        ];
        let current = self.setting_value(EditField::Address, cx);
        let current = current.trim().trim_end_matches('/').to_string();
        if let Some(editor) = &mut self.settings_ui.editor {
            // 派生名称（空或与旧地址主机一致）跟随新类型重新生成；手动命名保留
            let previous_host = preferences::service_host(&current, editor.draft.protocol);
            if editor.draft.name.trim().is_empty()
                || Some(editor.draft.name.trim()) == previous_host.as_deref()
            {
                editor.draft.name.clear();
                self.settings_ui.inputs[&EditField::Name].update(cx, |input, cx| {
                    input.set_value("", window, cx);
                });
            }
            let kind_changed = editor.draft.protocol.purpose() == ServicePurpose::Ai
                && candidate.purpose() == ServicePurpose::Ai
                && editor.draft.protocol != candidate;
            editor.draft.protocol = candidate;
            if candidate.keyless() {
                editor.draft.authentication = Authentication::None;
            }
            if kind_changed {
                // 模型 ID 不跨服务类型携带；Codex 连接成功后由账号目录自动填充
                editor.draft.model.clear();
                self.settings_ui.inputs[&EditField::Model].update(cx, |input, cx| {
                    input.set_value("", window, cx);
                });
            }
            editor.models.invalidate();
            editor.errors.clear();
            editor.evidence = None;
            editor.saved_configuration = None;
        }
        let is_codex_address = codex_addresses.contains(&current.as_str());
        let next = match candidate {
            ServiceProtocol::CodexResponses => Some(preferences::CODEX_ADDRESS),
            ServiceProtocol::OllamaChat if current.is_empty() || is_codex_address => {
                Some(preferences::OLLAMA_DEFAULT_ADDRESS)
            }
            // 离开 Codex 时清掉固定地址（非用户数据），其余输入保留
            _ if is_codex_address => Some(""),
            _ => None,
        };
        if let Some(next) = next {
            self.settings_ui.inputs[&EditField::Address].update(cx, |input, cx| {
                input.set_value(next, window, cx);
            });
            // set_value 不触发 Change 事件，草稿在此同步
            if let Some(editor) = &mut self.settings_ui.editor {
                editor.draft.address = next.to_string();
            }
        }
        if candidate.keyless() {
            self.settings_ui.inputs[&EditField::Key].update(cx, |input, cx| {
                input.set_value("", window, cx);
            });
        }
        if candidate == ServiceProtocol::CodexResponses {
            self.codex_refresh_status(crate::codex_ui::CodexSurface::Editor, cx);
        }
        cx.notify();
    }

    fn fetch_service_models(&mut self, cx: &mut Context<Self>) {
        if self
            .settings_ui
            .editor
            .as_ref()
            .is_some_and(|editor| editor.draft.protocol == ServiceProtocol::CodexResponses)
        {
            self.codex_refresh_models(crate::codex_ui::CodexSurface::Editor, cx);
            return;
        }
        let Some(draft) = self.live_service_model_draft(cx) else {
            return;
        };
        let request = crate::model_discovery::Request::from_draft(
            &draft,
            Secret::new(self.setting_value(EditField::Key, cx)),
        );
        let Some(editor) = &mut self.settings_ui.editor else {
            return;
        };
        if editor.pending_binding.is_some() {
            return;
        }
        let request = match request {
            Ok(request) => request,
            Err(error) => {
                editor.models.reject(error);
                cx.notify();
                return;
            }
        };
        let ticket = editor.models.begin(&request);
        let editor_id = draft.id;
        let vault = self.preferences.vault();
        // discover() performs synchronous keychain reads and bounded HTTP; it must not run
        // on a GPUI executor task (see spawn_blocking_io).
        let result_rx = crate::spawn_blocking_io(move || {
            smol::block_on(crate::model_discovery::discover(request, vault))
        });
        cx.spawn(async move |this, cx| {
            let Ok(result) = result_rx.recv().await else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                let current = this.live_service_model_draft(cx).and_then(|draft| {
                    crate::model_discovery::RequestKey::from_draft(
                        &draft,
                        &this.setting_value(EditField::Key, cx),
                    )
                    .ok()
                });
                if let Some(editor) = &mut this.settings_ui.editor
                    && editor.draft.id == editor_id
                {
                    editor.models.complete(ticket, current.as_ref(), result);
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// codex_ui 的编辑器钩子：仅在编辑器仍在编辑 Codex 服务时落目录/错误。
    pub(crate) fn editor_codex_needs_catalog(&self) -> bool {
        self.settings_ui.editor.as_ref().is_some_and(|editor| {
            editor.draft.protocol == ServiceProtocol::CodexResponses
                && editor.models.models().is_empty()
                && !editor.models.loading()
        })
    }
    pub(crate) fn editor_codex_models_loading(&mut self) -> bool {
        let Some(editor) = &mut self.settings_ui.editor else {
            return false;
        };
        if editor.draft.protocol != ServiceProtocol::CodexResponses {
            return false;
        }
        editor.models.begin_external();
        true
    }
    pub(crate) fn editor_codex_models_failed(&mut self, message: String) {
        if let Some(editor) = &mut self.settings_ui.editor
            && editor.draft.protocol == ServiceProtocol::CodexResponses
        {
            editor
                .models
                .reject(crate::model_discovery::Error::Custom(message));
        }
    }
    pub(crate) fn editor_codex_catalog(
        &mut self,
        models: Vec<String>,
        suggested: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if let Some(editor) = &mut self.settings_ui.editor
            && editor.draft.protocol == ServiceProtocol::CodexResponses
        {
            // 只补充候选：自动填充仅落到空输入，不改变已保存配置与测试证据
            editor.models.prime(models);
            crate::codex_ui::autofill_input(
                self.settings_ui.inputs[&EditField::Model].clone(),
                suggested,
                cx,
            );
        }
    }
    pub(crate) fn editor_codex_logged_out(&mut self) {
        if let Some(editor) = &mut self.settings_ui.editor
            && editor.draft.protocol == ServiceProtocol::CodexResponses
        {
            editor.models.invalidate();
        }
    }

    fn validate_service_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.save_open_service_draft(true, window, cx) {
            return false;
        }
        let Some(editor) = &mut self.settings_ui.editor else {
            return false;
        };
        editor.errors = editor.draft.validate();
        if let Some(error) = editor.errors.first() {
            let field = match error.field {
                "address" => EditField::Address,
                "model" => EditField::Model,
                _ => EditField::Key,
            };
            self.settings_ui.inputs[&field].update(cx, |input, cx| input.focus(window, cx));
            cx.notify();
            false
        } else {
            true
        }
    }
    fn start_service_test(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .settings_ui
            .editor
            .as_ref()
            .is_some_and(|editor| editor.test_running.is_some())
        {
            return;
        }
        if !self.validate_service_editor(window, cx) {
            return;
        }
        let Some(editor) = &mut self.settings_ui.editor else {
            return;
        };
        let Ok(config) = editor.draft.configuration() else {
            return;
        };
        let id = editor.draft.id.clone();
        let kind = editor.test_kind;
        let cancel = Arc::new(AtomicBool::new(false));
        editor.test_running = Some(cancel.clone());
        editor.evidence = None;
        editor.status = Some(format!("正在测试{}…", kind.label()));
        // Codex 订阅服务走 CLI 方言（Responses/SSE），而非桌面自带的 chat/completions 测试
        let task = if config.protocol == ServiceProtocol::CodexResponses {
            crate::spawn_blocking_io(move || {
                service_test::test_codex_blocking(&config, kind, &cancel)
            })
        } else {
            let vault = self.preferences.vault();
            // 服务测试含同步 Keychain 读取与受限 HTTP；见 crate::spawn_blocking_io 的说明
            crate::spawn_blocking_io(move || {
                smol::block_on(service_test::test_service(config, kind, vault, cancel))
            })
        };
        cx.spawn(async move |this, cx| {
            let Ok(evidence) = task.recv().await else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                let persisted = this.preferences.record_test(evidence.clone()).is_ok();
                let key_unchanged = this.setting_value(EditField::Key, cx).is_empty();
                if let Some(editor) = &mut this.settings_ui.editor
                    && editor.draft.id == id
                {
                    editor.test_running = None;
                    editor.status = None;
                    if key_unchanged
                        && editor.draft.configuration().ok().is_some_and(|config| {
                            config.fingerprint(&evidence.contract) == evidence.fingerprint
                        })
                    {
                        editor
                            .received_tests
                            .insert(evidence.fingerprint.clone(), evidence.clone());
                        editor.evidence = Some(evidence);
                        if !persisted {
                            editor.status =
                                Some("测试结果已收到，但尚未保存到设置；不会自动重测".into());
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn publish_open_service(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.publish_service_editor(true, window, cx);
    }
    fn repair_service_refusal(
        &self,
        cx: &App,
    ) -> Option<(TestKind, preferences::ServiceTestEvidence)> {
        let editor = self.settings_ui.editor.as_ref()?;
        let (task_id, components) = editor.repair_task.as_ref()?;
        if !self.setting_value(EditField::Key, cx).trim().is_empty() {
            return None;
        }
        let mut draft = editor.draft.clone();
        draft.address = self.setting_value(EditField::Address, cx);
        draft.model = self.setting_value(EditField::Model, cx);
        let config = draft.configuration().ok()?;
        let uses_vision = self
            .workspace
            .as_ref()
            .and_then(|workspace| {
                workspace
                    .state
                    .tasks
                    .iter()
                    .find(|task| task.id == *task_id)
            })
            .is_some_and(|task| task.plan.options.vision);
        let required: Vec<_> = [TestKind::Proofread, TestKind::Summary, TestKind::Vision]
            .into_iter()
            .filter(|kind| match kind {
                TestKind::Proofread => components
                    .iter()
                    .any(|component| component == "proofreading"),
                TestKind::Vision => {
                    uses_vision
                        && components
                            .iter()
                            .any(|component| component == "proofreading")
                }
                TestKind::Summary => components.iter().any(|component| component == "summary"),
                TestKind::Speech => false,
            })
            .map(TestKind::contract)
            .collect();
        [TestKind::Proofread, TestKind::Summary, TestKind::Vision]
            .into_iter()
            .filter_map(|kind| {
                let evidence = editor
                    .received_tests
                    .get(&config.fingerprint(kind.contract()))
                    .or_else(|| self.preferences.test_evidence(&config, kind.contract()))?;
                service_test::blocks_reprocessing(evidence, &config, &required)
                    .then(|| (kind, evidence.clone()))
            })
            .max_by_key(|(_, evidence)| evidence.tested_at)
    }
    fn publish_service_editor(
        &mut self,
        continue_repair: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.validate_service_editor(window, cx) {
            return;
        }
        let Some(editor) = &self.settings_ui.editor else {
            return;
        };
        let target = editor.target.clone();
        let repair_task = editor.repair_task.clone();
        let repairing = repair_task.is_some();
        if repairing && editor.test_running.is_some() {
            return;
        }
        if repairing && continue_repair && self.repair_service_refusal(cx).is_some() {
            cx.notify();
            return;
        }
        let inline = target.is_none() && !repairing;
        let scope = if !repairing && (target.is_none() || editor.also_default) {
            BindingScope::Defaults
        } else {
            BindingScope::CurrentTask
        };
        let result = if let Some(version) = editor
            .pending_binding
            .as_ref()
            .or(editor.saved_configuration.as_ref())
        {
            Ok(version.clone())
        } else {
            self.preferences.publish_service(&editor.draft.id, scope)
        };
        let version = match result {
            Ok(version) => version,
            Err(error) => {
                if let Some(editor) = &mut self.settings_ui.editor {
                    editor.status = Some(format!("服务尚未保存：{error:#}"));
                    editor.save_failed = true;
                }
                cx.notify();
                return;
            }
        };
        if repairing && !continue_repair {
            if let Some(editor) = &mut self.settings_ui.editor {
                editor.draft = ServiceDraft::from_version(&version);
                editor.saved_configuration = Some(version);
                editor.pending_binding = None;
                editor.status = None;
                editor.save_failed = false;
            }
            self.refresh_dispatch_controls(cx);
            cx.notify();
            return;
        }
        if let Some(target) = target {
            let binding = self
                .workspace
                .as_mut()
                .ok_or_else(|| anyhow!("当前笔记信息暂时不可用"))
                .and_then(|workspace| {
                    workspace.transaction(|state| {
                        let draft = state
                            .drafts
                            .iter_mut()
                            .find(|draft| draft.id == target)
                            .context("原笔记已关闭；服务已保存，但未用于其他笔记")?;
                        let selected =
                            (scope == BindingScope::CurrentTask).then(|| version.id.clone());
                        match version.config.protocol.purpose() {
                            ServicePurpose::Speech => draft.asr_service = selected,
                            ServicePurpose::Ai => draft.ai_service = selected,
                        };
                        Ok(())
                    })
                });
            if let Err(error) = binding {
                if let Some(editor) = &mut self.settings_ui.editor {
                    editor.pending_binding = Some(version);
                    editor.status = Some(format!("服务已保存，本次选择尚未保存：{error:#}"));
                }
                cx.notify();
                return;
            }
        }
        if let Some((task_id, components)) = repair_task {
            if !self.reprocess_task_with_service(task_id, components, version.id.clone(), cx) {
                if let Some(editor) = &mut self.settings_ui.editor {
                    editor.pending_binding = Some(version);
                    editor.status = Some("服务已保存，补做任务尚未创建。请重试补做。".into());
                }
                cx.notify();
                return;
            }
        }
        self.refresh_dispatch_controls(cx);
        self.set_settings_feedback(PreferenceGroup::Services, "服务已保存".into(), false);
        self.restore_service_focus(window, cx);
        if !inline {
            window.close_dialog(cx);
        }
        if !repairing {
            self.advance_conversion_when_ready(cx);
        }
        cx.notify();
    }
    pub(crate) fn close_service_editor(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if let Some(editor) = &self.settings_ui.editor {
            // Tests can stage a private edit, but cancelling never publishes
            // it. A cleanup failure cannot make these hidden records active.
            let _ = self.preferences.discard_service_draft(&editor.draft.id);
        }
        self.restore_service_focus(window, cx);
        self.settings_ui.inputs[&EditField::Key]
            .update(cx, |input, cx| input.set_value("", window, cx));
        true
    }
    fn restore_service_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(editor) = self.settings_ui.editor.take() {
            if let Some(cancel) = editor.test_running {
                cancel.store(true, Ordering::Release);
            }
            if let Some(focus) = editor.return_focus {
                focus.focus(window, cx);
            }
        }
        cx.notify();
    }
    pub(super) fn confirm_default_service(
        &mut self,
        version_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(version) = self.preferences.version(&version_id) else {
            return;
        };
        let purpose = version.config.protocol.purpose();
        let defaults = self.preferences.default_refs();
        let previous = match purpose {
            ServicePurpose::Speech => defaults.asr.as_deref(),
            ServicePurpose::Ai => defaults.llm.as_deref(),
        }
        .and_then(|id| self.preferences.version(id))
        .map(|default| {
            format!(
                "当前默认：{} · v{} · {}",
                default.config.name, default.number, default.config.model
            )
        })
        .unwrap_or_else(|| "当前尚未设置默认服务".to_owned());
        let next = format!(
            "更新为：{} · v{} · {}",
            version.config.name, version.number, version.config.model
        );
        let weak = cx.weak_entity();
        window.open_alert_dialog(cx, move |dialog, _, _| {
            let weak = weak.clone();
            let id = version_id.clone();
            dialog
                .title("更新默认服务")
                .child(text("default-service-previous", previous.clone()))
                .child(text("default-service-next", next.clone()))
                .child(
                    text(
                        "default-service-consequence",
                        "以后生成的笔记将使用这份配置。已提交的任务和已有笔记保持不变，此操作不会开始补做。",
                    )
                    .text_sm()
                    .text_color(color(MUTED)),
                )
                .button_props(
                    gpui_component::dialog::DialogButtonProps::default()
                        .ok_text("用于以后生成")
                        .cancel_text("保留原默认")
                        .show_cancel(true),
                )
                .on_ok(move |_, _, cx| {
                    weak.update(cx, |this, cx| {
                        match this.preferences.set_default_service(purpose, Some(&id)) {
                            Ok(()) => this.set_settings_feedback(
                                PreferenceGroup::Services,
                                "已更新默认服务，以后生成的笔记将使用此配置".into(),
                                false,
                            ),
                            Err(error) => {
                                this.set_settings_feedback(
                                    PreferenceGroup::Services,
                                    "默认服务尚未更改，请重试。".into(),
                                    true,
                                );
                                this.settings_ui.feedback_details.insert(
                                    PreferenceGroup::Services,
                                    format!("{error:#}"),
                                );
                            }
                        }
                        cx.notify();
                        true
                    })
                    .unwrap_or(false)
                })
        });
    }
    pub(super) fn confirm_delete_service(
        &mut self,
        service_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let version_ids: Vec<_> = self
            .preferences
            .versions()
            .filter(|v| v.service_id == service_id)
            .map(|v| v.id.clone())
            .collect();
        let affected = self
            .workspace
            .as_ref()
            .map(|w| {
                w.state
                    .tasks
                    .iter()
                    .filter(|task| {
                        !task.state.finished()
                            && (task
                                .plan
                                .asr_service
                                .as_ref()
                                .is_some_and(|id| version_ids.contains(id))
                                || task
                                    .plan
                                    .ai_service
                                    .as_ref()
                                    .is_some_and(|id| version_ids.contains(id)))
                    })
                    .count()
            })
            .unwrap_or(0);
        let weak = cx.weak_entity();
        window.open_alert_dialog(cx, move |dialog, _, _| {
            let weak = weak.clone();
            let id = service_id.clone();
            dialog
                .title("删除此服务")
                .child(text(
                    "delete-service-consequence",
                    "将删除此服务配置。已经发出的请求无法撤回，收到的结果和已有笔记仍会保存。需要时可以重新添加。",
                ))
                .when(affected > 0, |dialog| {
                    dialog.child(text(
                        "delete-service-affected",
                        format!("有 {affected} 个未完成任务引用此服务；相关后续请求会停止派发。"),
                    ))
                })
                .button_props(
                    gpui_component::dialog::DialogButtonProps::default()
                        .ok_text("删除服务")
                        .cancel_text("保留服务")
                        .show_cancel(true),
                )
                .on_ok(move |_, _, cx| {
                    weak.update(cx, |this, cx| match this.preferences.delete_service(&id) {
                        Ok(()) => {
                            this.refresh_dispatch_controls(cx);
                            this.set_settings_feedback(
                                PreferenceGroup::Services,
                                "已删除此服务".into(),
                                false,
                            );
                            cx.notify();
                            true
                        }
                        Err(error) => {
                            // The retire marker is written before the group is persisted,
                            // so a mid-failure can leave dispatch blocked without a visible
                            // deletion; say so instead of claiming nothing happened.
                            let blocked = this.preferences.is_service_retired(&id);
                            this.set_settings_feedback(
                                PreferenceGroup::Services,
                                format!(
                                    "{}：{error:#}",
                                    if blocked {
                                        "已停止派发，删除记录尚未完整保存"
                                    } else {
                                        "尚未删除此服务"
                                    }
                                ),
                                true,
                            );
                            this.refresh_dispatch_controls(cx);
                            cx.notify();
                            true
                        }
                    })
                    .unwrap_or(false)
                })
        });
    }

    pub(crate) fn commit_generation(
        &mut self,
        next: GenerationPreferences,
        cx: &mut Context<Self>,
    ) -> bool {
        let previous = self
            .ordinary_preferences_submit_issue()
            .map(|issue| issue.message);
        match self.preferences.save_generation(next.clone()) {
            Ok(()) => {
                self.settings_ui.pending_generation = None;
                self.set_settings_feedback(PreferenceGroup::Generation, "已保存".into(), false);
                self.refresh_preference_defaults(cx);
                self.clear_resolved_generation_errors(previous);
                if !self.preference_defaults_pending {
                    self.advance_conversion_when_ready(cx);
                }
                true
            }
            Err(error) => {
                self.settings_ui.pending_generation = Some(next);
                self.set_settings_save_failure(PreferenceGroup::Generation, &error);
                cx.notify();
                false
            }
        }
    }
    pub(crate) fn refresh_preference_defaults(&mut self, cx: &mut Context<Self>) {
        let out = self.config.defaults.out.clone();
        self.config = self.preferences.defaults_config();
        self.config.defaults.out = out;
        self.settings_options = ConversionOptions::from_config(&self.config);
        self.desktop_settings = self.preferences.application().desktop.clone();
        let defaults = self.settings_options.clone();
        if let Some(workspace) = &mut self.workspace {
            match workspace.transaction(|state| {
                for draft in &mut state.drafts {
                    if draft.submitted_task.is_none() {
                        draft.inherit(&defaults);
                    }
                }
                Ok(())
            }) {
                Ok(()) => {
                    if let Some(draft) = workspace.state.draft() {
                        self.task_options = draft.options.clone();
                    }
                    if self.preference_defaults_pending {
                        self.preference_defaults_pending = false;
                        self.workspace_error = None;
                    }
                }
                Err(error) => {
                    self.preference_defaults_pending = true;
                    self.workspace_error =
                        Some(format!("默认设置已保存，当前视频的选项尚未同步：{error:#}"))
                }
            }
        }
        cx.notify();
    }
}
