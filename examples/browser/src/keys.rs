//! `BROWSER_SCENE=keys`: a text field with the keyboard over an empty pane. The field's first
//! keystroke mounts a Chrome-style page in the pane, so the keys typed right after it arrive
//! while Chromium creates the page's window and activates it. The main thread stays busy for a
//! while after, as a browser's is when a load starts, so those keys queue up and are dispatched
//! in the turns Chromium's window holds key. Every key is the field's
//! (`gate_m8_webview::real_keys_survive_a_new_page_cef_chrome`).

use std::time::{Duration, Instant};

use nd::prelude::*;
use nd::webview::{WebView, webview};

/// Its title is "page" and every key the page gets, so a key that went to it shows.
const PAGE: &str = "<!doctype html><title>page</title>\
<body style=\"margin:0\" onkeydown=\"document.title += event.key\"></body>";

struct Scene {
    text: SharedString,
    page: Option<WebView>,
}

impl Render for Scene {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let field =
            text_field("field").text(self.text.clone()).on_change(cx.listener(|this, e: &TextChanged, _, cx| {
                this.text = e.text.clone();
                if this.page.is_none() {
                    let page = WebView::new();
                    page.load_html(PAGE, None);
                    this.page = Some(page);
                    busy(Instant::now() + Duration::from_millis(1500));
                }
                cx.notify();
            }));
        let page = self.page.as_ref().map(|page| webview("web", page).size_full());
        v_flex().size_full().child(field).child(div().flex_1().min_h_0().children(page))
    }
}

/// Blocks the main thread in 40 ms slices with 10 ms gaps until `until`.
fn busy(until: Instant) {
    nd::system::after(Duration::from_millis(10), move |_| {
        std::thread::sleep(Duration::from_millis(40));
        if Instant::now() < until {
            busy(until);
        }
    });
}

pub fn open(cx: &mut App) {
    let options = WindowOptions::main("main").title("Keys").size(size(px(720.), px(480.)));
    cx.open_window(options, move |_window, cx| cx.new(move |_cx| Scene { text: SharedString::default(), page: None }));
}
