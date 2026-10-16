//! Preserve the visual background while making modal dialogs the only accessible scope.
use gpui::*;

/// Opt-in diagnostics for validation builds launched through LaunchServices.
/// Do not capture field values or broad application logs. A panic records its
/// source location and backtrace; its payload may contain user data and is omitted.
pub fn init_validation_diagnostics() {
    #[cfg(all(debug_assertions, unix))]
    {
        use std::{
            fs::OpenOptions,
            io::Write,
            os::unix::fs::OpenOptionsExt,
            sync::{Arc, Mutex},
        };
        let Some(path) = std::env::var_os("COURSE2MD_VALIDATION_PANIC_LOG") else {
            return;
        };
        let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)
        else {
            return;
        };
        let _ = writeln!(
            file,
            "validation panic hook installed; pid={}",
            std::process::id()
        );
        let file = Arc::new(Mutex::new(file));
        std::panic::set_hook(Box::new(move |info| {
            if let Ok(mut file) = file.lock() {
                let _ = writeln!(file, "panic at {:?}", info.location());
                let _ = writeln!(file, "{}", std::backtrace::Backtrace::force_capture());
                let _ = file.flush();
            }
        }));
    }
}

pub struct ModalBackground {
    child: AnyElement,
    hidden: bool,
}

impl ModalBackground {
    pub fn new(child: impl IntoElement, hidden: bool) -> Self {
        Self {
            child: child.into_any_element(),
            hidden,
        }
    }
}

impl IntoElement for ModalBackground {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for ModalBackground {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        Some("modal-background".into())
    }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }
    fn a11y_role(&self) -> Option<Role> {
        Some(Role::Pane)
    }
    fn write_a11y_info(&self, node: &mut accesskit::Node) {
        if self.hidden {
            node.set_hidden();
        }
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.prepaint(window, cx);
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
    }
}
