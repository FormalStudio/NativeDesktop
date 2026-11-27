//! `BROWSER_SCENE=toast`: an image in a page under a strip of the app's own, for Copy Image from
//! the page's native context menu. The status line names each [`ChromeToast`] the page reports
//! (`toast=ImageCopied`), and each is printed as `ND_TOAST ImageCopied`; Chromium's own toast
//! never shows (`gate_m8_webview::real_copy_image_toast_cef_chrome`, and
//! `real_webview::real_copy_image_toast_cef_chrome` on Linux).

use nd::prelude::*;
use nd::webview::{ChromeToast, TitleChanged, WebView, webview};

/// The image is drawn on a canvas, so the page needs no file or network; its title turns
/// "image" once the image has decoded.
const PAGE: &str = r##"<!doctype html><html><head><title>page</title></head>
<body style="margin:0;display:grid;place-items:center;height:100vh">
<img id="i" width="240" height="160" alt="">
<script>
  const c = document.createElement("canvas");
  c.width = 240; c.height = 160;
  const g = c.getContext("2d");
  g.fillStyle = "#d04030"; g.fillRect(0, 0, 240, 160);
  g.fillStyle = "#ffffff"; g.fillRect(60, 40, 120, 80);
  const i = document.getElementById("i");
  i.onload = () => { document.title = "image"; };
  i.src = c.toDataURL("image/png");
</script></body></html>"##;

struct Scene {
    page: WebView,
    title: String,
    toasts: Vec<String>,
}

impl Scene {
    fn status(&self) -> String {
        format!("title={} toast={}", self.title, self.toasts.join(","))
    }
}

impl Render for Scene {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let page = webview("web", &self.page)
            .flex_1()
            .min_h_0()
            .on_title_changed(cx.listener(|this, e: &TitleChanged, _, cx| {
                this.title = e.title.clone();
                cx.notify();
            }))
            .on_chrome_toast(cx.listener(|this, e: &ChromeToast, _, cx| {
                eprintln!("ND_TOAST {:?}", e.toast);
                this.toasts.push(format!("{:?}", e.toast));
                cx.notify();
            }));
        v_flex().size_full().child(div().h(px(48.)).p(px(12.)).child(text(self.status()).id("status"))).child(page)
    }
}

pub fn open(cx: &mut App) {
    let page = WebView::new();
    page.load_html(PAGE, None);
    let options = WindowOptions::main("main").title("Toast").size(size(px(720.), px(480.)));
    cx.open_window(options, move |_window, cx| {
        cx.new(move |_cx| Scene { page, title: String::new(), toasts: Vec::new() })
    });
}
