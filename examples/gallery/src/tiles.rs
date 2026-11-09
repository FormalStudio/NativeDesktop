//! The tiles page: cards in a tile grid that dip while pressed, open on a click and reorder by
//! dragging, each title cut to one line and each summary to two, and boxes with corners of
//! different radii.

use nd::prelude::*;

const CARDS: [(&str, &str, &str); 6] = [
    ("inbox", "Inbox", "Everything that has not found a home yet, newest first."),
    ("ideas", "Ideas for the spring release and the redesign of the settings window", "Loose notes."),
    (
        "reading",
        "Reading",
        "Articles saved for later, papers on text layout and typesetting, and a long list of books \
         that keeps growing faster than it shrinks.",
    ),
    ("trips", "Trips", "Packing lists and bookings."),
    ("recipes", "Recipes", "Weeknight dinners, bread, and the cake from the market in Lisbon."),
    ("archive", "Archive", "Finished projects."),
];

pub struct TilesPage {
    order: Vec<&'static str>,
    pub last: SharedString,
}

impl TilesPage {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self { order: CARDS.iter().map(|c| c.0).collect(), last: "No card opened yet".into() }
    }
}

impl Render for TilesPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (raised, border, secondary) = (theme.surface.raised, theme.border.subtle, theme.text.secondary);
        let tiles =
            self.order.iter().filter_map(|id| CARDS.iter().find(|c| c.0 == *id)).map(|&(id, title, summary)| {
                let card = v_flex()
                    .id(format!("card-{id}"))
                    .size_full()
                    .p_3()
                    .gap_1()
                    .rounded_lg()
                    .bg(raised)
                    .border_1()
                    .border_color(border)
                    .shadow_sm()
                    .active(|p| p.scale(0.97))
                    .child(text(title).font_weight(FontWeight::SEMIBOLD).truncate())
                    .child(text(summary).text_color(secondary).line_clamp(2).text_ellipsis())
                    .on_tap(cx.listener(move |this, _: &TapEvent, _, cx| {
                        this.last = format!("Opened {title}").into();
                        cx.notify();
                    }));
                tile(id, card)
            });
        let shape = |id: &str| div().id(id.to_owned()).size(px(72.)).bg(theme.accent).border_2().border_color(border);
        v_flex()
            .gap_4()
            .w_full()
            .child(
                tile_grid("cards")
                    .max_columns(3)
                    .min_tile_width(150.)
                    .aspect(0.5)
                    .gap(12.)
                    .tiles(tiles)
                    .draggable_tiles("card")
                    .on_move(cx.listener(|this, e: &TileMoved, _, cx| {
                        if let Some(from) = this.order.iter().position(|id| *id == e.id.as_ref()) {
                            let id = this.order.remove(from);
                            this.order.insert(e.index.min(this.order.len()), id);
                        }
                        cx.notify();
                    })),
            )
            .child(text(self.last.clone()).id("tiles-last").text_color(secondary))
            .child(
                h_flex()
                    .gap_3()
                    .child(shape("corners-diagonal").rounded_tl(px(24.)).rounded_br(px(24.)))
                    .child(shape("corners-top").rounded_t(px(16.)))
                    .child(
                        shape("corners-mixed")
                            .rounded_tl(px(4.))
                            .rounded_tr(px(16.))
                            .rounded_br(px(32.))
                            .rounded_bl(px(8.))
                            .shadow_md(),
                    ),
            )
    }
}
