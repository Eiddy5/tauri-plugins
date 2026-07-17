use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock, Weak},
};

use objc2::{define_class, msg_send, rc::Retained, DefinedClass, MainThreadOnly};
use objc2_app_kit::{
    NSBezierPath, NSColor, NSCompositingOperation, NSEvent, NSGraphicsContext, NSLineCapStyle,
    NSView,
};
use objc2_foundation::{MainThreadMarker, NSObjectProtocol, NSPoint, NSRect};

use crate::{annotation::AnnotationSession, models::AnnotationTool};

#[derive(Clone, Copy)]
pub(crate) struct AnnotationViewIvars {
    session_id: u64,
}

define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[ivars = AnnotationViewIvars]
    pub(crate) struct AnnotationView;

    unsafe impl NSObjectProtocol for AnnotationView {}

    impl AnnotationView {
        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            self.handle_point(event, PointerPhase::Down);
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            self.handle_point(event, PointerPhase::Dragged);
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) {
            self.handle_point(event, PointerPhase::Up);
        }

        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty_rect: NSRect) {
            self.draw_annotations();
        }
    }
);

#[derive(Clone, Copy)]
enum PointerPhase {
    Down,
    Dragged,
    Up,
}

impl AnnotationView {
    pub(crate) fn new(session_id: u64, frame: NSRect, mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(AnnotationViewIvars { session_id });
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    fn handle_point(&self, event: &NSEvent, phase: PointerPhase) {
        let Some(session) = annotation_session(self.ivars().session_id) else {
            return;
        };
        let bounds = self.bounds();
        if bounds.size.width <= 0.0 || bounds.size.height <= 0.0 {
            return;
        }
        let location = self.convertPoint_fromView(event.locationInWindow(), None);
        let point = normalized_point(location, bounds);
        let result = match phase {
            PointerPhase::Down => {
                let state = session.state();
                let width = match state.tool {
                    AnnotationTool::Pen { width, .. } | AnnotationTool::Eraser { width } => width,
                };
                let normalized_width = width / bounds.size.width.min(bounds.size.height) as f32;
                session.begin_stroke(point, normalized_width.clamp(f32::EPSILON, 1.0))
            }
            PointerPhase::Dragged => session.append_point(point),
            PointerPhase::Up => session
                .append_point(point)
                .and_then(|()| session.end_stroke().map(|_| ())),
        };
        if let Err(error) = result {
            tracing::debug!(%error, "macOS annotation pointer event was ignored");
        }
        self.setNeedsDisplay(true);
    }

    fn draw_annotations(&self) {
        let Some(session) = annotation_session(self.ivars().session_id) else {
            return;
        };
        let bounds = self.bounds();
        let snapshot = session.snapshot();
        let Some(context) = NSGraphicsContext::currentContext() else {
            return;
        };
        context.saveGraphicsState();
        context.setShouldAntialias(true);
        context.setCompositingOperation(NSCompositingOperation::Clear);
        NSBezierPath::fillRect(bounds);
        for operation in snapshot.operations() {
            let stroke = operation.stroke();
            let width =
                f64::from(stroke.normalized_width()) * bounds.size.width.min(bounds.size.height);
            context.setCompositingOperation(if operation.is_eraser() {
                NSCompositingOperation::Clear
            } else {
                NSCompositingOperation::SourceOver
            });
            if let Some(color) = stroke.color() {
                NSColor::colorWithSRGBRed_green_blue_alpha(
                    f64::from(color.red) / 255.0,
                    f64::from(color.green) / 255.0,
                    f64::from(color.blue) / 255.0,
                    f64::from(color.alpha) / 255.0,
                )
                .setStroke();
            }
            let path = NSBezierPath::bezierPath();
            path.setLineWidth(width.max(1.0));
            path.setLineCapStyle(NSLineCapStyle::Round);
            path.setLineJoinStyle(objc2_app_kit::NSLineJoinStyle::Round);
            let mut points = stroke.points().iter().map(|point| {
                NSPoint::new(
                    bounds.origin.x + f64::from(point.x) * bounds.size.width,
                    bounds.origin.y + (1.0 - f64::from(point.y)) * bounds.size.height,
                )
            });
            if let Some(first) = points.next() {
                path.moveToPoint(first);
                let mut has_segment = false;
                for point in points {
                    path.lineToPoint(point);
                    has_segment = true;
                }
                if !has_segment {
                    path.lineToPoint(first);
                }
                path.stroke();
            }
        }
        context.restoreGraphicsState();
    }
}

fn normalized_point(location: NSPoint, bounds: NSRect) -> crate::annotation::NormalizedPoint {
    crate::annotation::NormalizedPoint::new(
        ((location.x - bounds.origin.x) / bounds.size.width) as f32,
        (1.0 - (location.y - bounds.origin.y) / bounds.size.height) as f32,
    )
}

static SESSIONS: OnceLock<Mutex<HashMap<u64, Weak<AnnotationSession>>>> = OnceLock::new();

pub(crate) fn register_annotation_session(id: u64, session: Arc<AnnotationSession>) {
    sessions()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(id, Arc::downgrade(&session));
}

pub(crate) fn unregister_annotation_session(id: u64) {
    sessions()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(&id);
}

fn annotation_session(id: u64) -> Option<Arc<AnnotationSession>> {
    sessions()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&id)
        .and_then(Weak::upgrade)
}

fn sessions() -> &'static Mutex<HashMap<u64, Weak<AnnotationSession>>> {
    SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}
