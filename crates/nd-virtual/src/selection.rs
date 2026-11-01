//! What a press, a key or a marquee does to a list's selection on backends that draw the list
//! (GPUI), written to match what `NSTableView` and `NSCollectionView` do on macOS: a plain click
//! selects one row and moves the anchor to it, the secondary key (Command on macOS, Control
//! elsewhere) toggles a row, Shift selects the range from the anchor, Shift with the secondary key
//! adds that range. Keys move the cursor and select, Shift extending from the anchor. Pure logic
//! over row order, so tier 1 tests read it without a window.

use crate::core::RowId;
use crate::list::Selection;

/// The modifiers that change what a click or a key selects.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Extend {
    /// Shift: a range from the anchor.
    pub range: bool,
    /// The secondary key: toggle, or add a range to what is selected.
    pub toggle: bool,
}

impl From<nd_gesture::KeyModifiers> for Extend {
    fn from(m: nd_gesture::KeyModifiers) -> Self {
        Extend { range: m.shift, toggle: m.secondary() }
    }
}

/// A selection after an input: the rows in list order, the anchor ranges grow from and the cursor
/// (the row the last click or key landed on, which keys move from).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Picked {
    pub selected: Vec<RowId>,
    pub anchor: Option<RowId>,
    pub cursor: Option<RowId>,
}

/// A key that moves the cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nav {
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
}

fn in_order(rows: &[RowId], picked: impl IntoIterator<Item = RowId>) -> Vec<RowId> {
    let picked: std::collections::HashSet<RowId> = picked.into_iter().collect();
    rows.iter().copied().filter(|r| picked.contains(r)).collect()
}

fn range(rows: &[RowId], from: RowId, to: RowId) -> Vec<RowId> {
    let (Some(a), Some(b)) = (rows.iter().position(|r| *r == from), rows.iter().position(|r| *r == to)) else {
        return vec![to];
    };
    rows[a.min(b)..=a.max(b)].to_vec()
}

/// A click on `row`. `rows` is the list's order, `selected` what is selected now.
pub fn click(
    rows: &[RowId],
    selected: &[RowId],
    anchor: Option<RowId>,
    row: RowId,
    mode: Selection,
    extend: Extend,
) -> Picked {
    if mode != Selection::Multiple || (!extend.range && !extend.toggle) {
        return Picked { selected: vec![row], anchor: Some(row), cursor: Some(row) };
    }
    let anchor = anchor.filter(|a| rows.contains(a));
    match (extend.range, anchor) {
        (true, Some(anchor)) => {
            let span = range(rows, anchor, row);
            let selected = if extend.toggle { in_order(rows, selected.iter().copied().chain(span)) } else { span };
            Picked { selected, anchor: Some(anchor), cursor: Some(row) }
        }
        _ if extend.toggle => {
            let selected = if selected.contains(&row) {
                selected.iter().copied().filter(|r| *r != row).collect()
            } else {
                in_order(rows, selected.iter().copied().chain([row]))
            };
            Picked { selected, anchor: Some(row), cursor: Some(row) }
        }
        _ => Picked { selected: vec![row], anchor: Some(row), cursor: Some(row) },
    }
}

/// Where `nav` takes the cursor: an index into `rows`, or `None` when it stays. `lanes` is a
/// grid's column count (1 in a list), `page` the rows one viewport holds. `selectable` says which
/// indices may take the cursor; the move skips the others in its direction.
pub fn step(
    len: usize,
    lanes: usize,
    cursor: Option<usize>,
    nav: Nav,
    page: usize,
    selectable: impl Fn(usize) -> bool,
) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let lanes = lanes.max(1);
    let last = len - 1;
    let page = page.max(1) * lanes;
    let target = match (nav, cursor) {
        (Nav::Home, _) => 0,
        (Nav::End, _) => last,
        (Nav::Left | Nav::Right, _) if lanes == 1 => return None,
        (Nav::Up | Nav::Left | Nav::PageUp, None) => last,
        (_, None) => 0,
        (Nav::Up, Some(i)) => i.checked_sub(lanes)?,
        (Nav::Down, Some(i)) => (i + lanes <= last).then_some(i + lanes)?,
        (Nav::Left, Some(i)) => i.checked_sub(1)?,
        (Nav::Right, Some(i)) => (i < last).then_some(i + 1)?,
        (Nav::PageUp, Some(i)) => i.saturating_sub(page),
        (Nav::PageDown, Some(i)) => (i + page).min(last),
    };
    if selectable(target) {
        return Some(target);
    }
    // Past an unselectable row (a section header) in the move's direction, then back.
    let forward = matches!(nav, Nav::Down | Nav::Right | Nav::PageDown | Nav::Home)
        || (cursor.is_none() && !matches!(nav, Nav::Up | Nav::Left | Nav::PageUp | Nav::End));
    let ahead: Box<dyn Iterator<Item = usize>> =
        if forward { Box::new(target + 1..len) } else { Box::new((0..target).rev()) };
    let behind: Box<dyn Iterator<Item = usize>> =
        if forward { Box::new((0..target).rev()) } else { Box::new(target + 1..len) };
    ahead.chain(behind).find(|i| selectable(*i) && Some(*i) != cursor)
}

/// The cursor moved to `target` by a key: it alone is selected, or with Shift the range from the
/// anchor to it.
pub fn key(rows: &[RowId], anchor: Option<RowId>, target: RowId, mode: Selection, range: bool) -> Picked {
    let anchor = anchor.filter(|a| rows.contains(a));
    match anchor {
        Some(anchor) if range && mode == Selection::Multiple => {
            Picked { selected: self::range(rows, anchor, target), anchor: Some(anchor), cursor: Some(target) }
        }
        _ => Picked { selected: vec![target], anchor: Some(target), cursor: Some(target) },
    }
}

/// A marquee over `swept` that started with `base` selected: with the secondary key it toggles
/// what it sweeps, with Shift it adds, without either it replaces.
pub fn marquee(rows: &[RowId], base: &[RowId], swept: &[RowId], extend: Extend) -> Vec<RowId> {
    if extend.toggle {
        let swept: std::collections::HashSet<RowId> = swept.iter().copied().collect();
        let base: std::collections::HashSet<RowId> = base.iter().copied().collect();
        rows.iter().copied().filter(|r| base.contains(r) != swept.contains(r)).collect()
    } else if extend.range {
        in_order(rows, base.iter().chain(swept).copied())
    } else {
        in_order(rows, swept.iter().copied())
    }
}
