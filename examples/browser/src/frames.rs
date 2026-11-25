//! `BROWSER_SCENE=frames`: user scripts, script messages and `execute_javascript_in_frame` reach
//! a cross-site frame, which site isolation runs in a renderer of its own. A page on
//! `127.0.0.1` holds a frame from `[::1]`; an all-frames script in an isolated world posts from
//! both. The frame's message names its frame; script run in that frame through the id without
//! a gesture changes its document and gives it no user activation, and with one it does.
//! Prints `ND_FRAMES_OK`, or `ND_FRAMES_FAILED` and why, and exits.

use nd::prelude::*;
use nd::webview::{ScriptMessage, UserGesture, UserScript, WebView, webview};

use crate::server;

const WORLD: &str = "ndframes";
const HANDLER: &str = "frames";
const POST: &str = "window.webkit.messageHandlers.frames.postMessage(location.host)";

fn fail(why: impl std::fmt::Display) -> ! {
    eprintln!("ND_FRAMES_FAILED {why}");
    std::process::exit(1);
}

struct Scene {
    page: WebView,
    main: Option<String>,
    inner: bool,
}

impl Scene {
    fn message(&mut self, e: &ScriptMessage) {
        eprintln!("ND_FRAMES message from {} main={} frame={}", e.origin, e.main_frame, e.frame);
        if e.world != WORLD || e.frame.is_empty() {
            fail(format!("the message came from world {:?}, frame {:?}", e.world, e.frame));
        }
        if e.main_frame {
            self.main = Some(e.frame.clone());
            return;
        }
        if self.inner || !e.origin.starts_with("http://[::1]") {
            return;
        }
        self.inner = true;
        if self.main.as_deref() == Some(e.frame.as_str()) {
            fail("the frame's message carries the main frame's id");
        }
        let (page, frame) = (self.page.clone(), e.frame.clone());
        let change = "document.body.textContent = 'changed in ' + location.host; navigator.userActivation.isActive";
        let target = Some(e.frame.as_str());
        self.page.execute_javascript_in_frame(change, Some(WORLD), target, UserGesture::Without, move |r, _| {
            match r {
                Ok(serde_json::Value::Bool(false)) => {}
                other => fail(format!("without a gesture, the frame's world answered {other:?}")),
            }
            let read = "[document.body.textContent, navigator.userActivation.isActive]";
            let id = frame.clone();
            page.execute_javascript_in_frame(read, None, Some(&id), UserGesture::With, move |r, _| {
                let text = match &r {
                    Ok(v) if v[1] == true => v[0].as_str().unwrap_or_default().to_owned(),
                    other => fail(format!("with a gesture, the frame answered {other:?}")),
                };
                if !text.starts_with("changed in [::1]") {
                    fail(format!("the frame's document reads {text:?}"));
                }
                eprintln!("ND_FRAMES_OK frame={frame} text={text:?}");
                std::process::exit(0);
            });
        });
    }
}

impl Render for Scene {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        webview("web", &self.page)
            .size_full()
            .on_script_message(cx.listener(|this, e: &ScriptMessage, _, _| this.message(e)))
    }
}

pub fn open(cx: &mut App) {
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_secs(60));
        fail("no verdict within 60 s");
    });
    let base = server::start();
    let page = WebView::new();
    page.register_script_message(HANDLER, Some(WORLD), ["http://127.0.0.1:*", "http://[::1]:*"]);
    page.add_user_script(UserScript::new("frames", POST).in_world(WORLD).all_frames());
    page.load(format!("{base}/frames"));
    let options = WindowOptions::main("main").title("Frames").size(size(px(720.), px(480.)));
    cx.open_window(options, move |_window, cx| cx.new(move |_cx| Scene { page, main: None, inner: false }));
}
