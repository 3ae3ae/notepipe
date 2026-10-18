//! Ollama 本地服务发现：探测本机服务与已下载模型，作为 Ollama 服务表单的地址与模型候选。
//! Ollama 的 OpenAI 兼容端点无需凭据，「登录」即服务发现 + 配置（见 core login/ollama）。
use super::*;
use crate::preferences::{ServiceProtocol, ServicePurpose};
use crate::settings_ui::service_form::{FormField, FormOwner};

#[derive(Default)]
pub(crate) struct OllamaUi {
    generation: u64,
    working: bool,
    /// Ok((地址, 模型列表)) 已检测到；Err(说明) 未检测到；None 尚未检测
    status: Option<Result<(String, Vec<String>), String>>,
}

impl OllamaUi {
    /// 已经有过检测结果（或正在检测）：进入设置页时不重复发起
    pub(crate) fn checked(&self) -> bool {
        self.working || self.status.is_some()
    }
}

impl Desktop {
    /// 探测本机 Ollama 服务。阻塞网络请求不进 GPUI 执行器（见 spawn_blocking_io）。
    pub(crate) fn ollama_refresh(&mut self, cx: &mut Context<Self>) {
        if self.ollama.working {
            return;
        }
        self.ollama.working = true;
        self.ollama.generation = self.ollama.generation.wrapping_add(1);
        let generation = self.ollama.generation;
        // The engine error names `ollama serve`; the page offers its own action.
        let task = crate::spawn_blocking_io(|| {
            course2md::login::ollama::discover_for_desktop().map_err(|_| {
                course2md::tr!("未检测到 Ollama，请安装并打开 Ollama 后重新检查").to_owned()
            })
        });
        cx.spawn(async move |this, cx| {
            let result = task
                .recv()
                .await
                .unwrap_or_else(|_| Err(crate::interrupted().to_owned()));
            let _ = this.update(cx, |this, cx| {
                if this.ollama.generation != generation {
                    return;
                }
                this.ollama.working = false;
                this.ollama.status = Some(result);
                this.ollama_apply_catalog(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// The detected local address and model count, once a check found one.
    pub(crate) fn ollama_detected(&self) -> Option<(String, usize)> {
        match &self.ollama.status {
            Some(Ok((host, models))) => Some((host.clone(), models.len())),
            _ => None,
        }
    }

    /// Ollama forms read the local service: a check runs once, later forms
    /// reuse its result.
    pub(crate) fn ollama_prepare_form(&mut self, cx: &mut Context<Self>) {
        if self.ollama.checked() {
            self.ollama_apply_catalog(cx);
        } else {
            self.ollama_refresh(cx);
        }
    }

    /// Detected models become candidates on every form still editing an Ollama
    /// service; an empty address or model is filled, typed values stay.
    pub(crate) fn ollama_apply_catalog(&mut self, cx: &mut Context<Self>) {
        let Some(Ok((host, models))) = self.ollama.status.clone() else {
            return;
        };
        let window = self.window;
        for owner in [FormOwner::Editor, FormOwner::Setup(ServicePurpose::Ai)] {
            let Some(form) = self
                .service_form_mut(owner)
                .filter(|form| form.draft.protocol == ServiceProtocol::OllamaChat)
            else {
                continue;
            };
            form.models.prime(models.clone());
            let address = form.input(FormField::Address).clone();
            let model = form.input(FormField::Model).clone();
            crate::codex_ui::autofill_input(window, address, Some(host.clone()), cx);
            crate::codex_ui::autofill_input(window, model, models.first().cloned(), cx);
        }
    }
}
