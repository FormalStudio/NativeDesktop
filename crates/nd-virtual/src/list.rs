//! The app-facing API: [`ListState`] holds an app's keys over a [`Core`], and `list()`,
//! `table()`, `grid()` and `tree()` turn it into one native container node whose children are
//! the rows the model wants rendered.
//!
//! Each rendered row is its own view ([`RowView`]), created when the row enters the render range
//! and dropped when it leaves. A view is a boundary in `nd-ui`: the owner re-rendering does not
//! re-render it, so a row renders again only when its version changes or it is invalidated
//! (requirement g). Rows render outside the owner's render and outside any native callback, and
//! presenters answer native data sources from the rows already rendered (3.13).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::hash::Hash;
use std::path::PathBuf;
use std::rc::{Rc, Weak};

use nd_ui::prelude::*;
use nd_ui::{ContainerElement, EntityId, Key, Scope, StyleRefinement};

use crate::core::{Align, Alignment, Core, Follow, GridSpec, RowId};
pub use nd_gesture::RowEdge;
use nd_gesture::{RowSwipeEvent, TapEvent};
use nd_ui::Action;
use rustc_hash::FxHashMap;

/// What a list can be keyed by.
pub trait RowKey: Clone + Eq + Hash + fmt::Debug + 'static {}
impl<T: Clone + Eq + Hash + fmt::Debug + 'static> RowKey for T {}

/// The model, shared between an app's [`ListState`] and the backend's presenter. Compared by
/// identity in props.
#[derive(Clone)]
pub struct ListHandle(pub Rc<RefCell<Core>>);

impl PartialEq for ListHandle {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl fmt::Debug for ListHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ListHandle({:p})", Rc::as_ptr(&self.0))
    }
}

/// Which native container presents the rows.
#[derive(Clone, Debug, PartialEq)]
pub enum ListKind {
    /// A single column: `NSTableView` on macOS.
    List,
    /// Columns with a header: `NSTableView` on macOS.
    Table(Vec<Column>),
    /// Fixed-size cells in lanes: `NSCollectionView` on macOS.
    Grid,
    /// Rows with depth and disclosure: `NSOutlineView` on macOS.
    Tree,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SortDirection {
    #[default]
    Ascending,
    Descending,
}

/// A table column. Sorting stays the app's: a header click reports [`SortChanged`] and the app
/// re-sorts its keys.
#[derive(Clone, Debug, PartialEq)]
pub struct Column {
    pub id: SharedString,
    pub title: SharedString,
    pub width: f32,
    pub min_width: f32,
    pub resizable: bool,
    pub sortable: bool,
    pub sort: Option<SortDirection>,
}

pub fn column(id: impl Into<SharedString>, title: impl Into<SharedString>) -> Column {
    Column {
        id: id.into(),
        title: title.into(),
        width: 120.,
        min_width: 40.,
        resizable: true,
        sortable: false,
        sort: None,
    }
}

impl Column {
    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    pub fn min_width(mut self, width: f32) -> Self {
        self.min_width = width;
        self
    }

    pub fn fixed(mut self) -> Self {
        self.resizable = false;
        self
    }

    pub fn sortable(mut self) -> Self {
        self.sortable = true;
        self
    }

    pub fn sorted(mut self, direction: SortDirection) -> Self {
        self.sortable = true;
        self.sort = Some(direction);
        self
    }
}

/// How many rows the user can select.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Selection {
    #[default]
    None,
    Single,
    Multiple,
}

impl Selection {
    pub fn single() -> Self {
        Selection::Single
    }

    pub fn multiple() -> Self {
        Selection::Multiple
    }
}

/// Props of the list's container node.
#[derive(Clone, Debug, PartialEq)]
pub struct ListProps {
    pub handle: ListHandle,
    pub kind: ListKind,
    /// The model's generation when the owner last rendered: presenters re-read rows when it moves.
    pub generation: u64,
    pub selection: Selection,
    pub selected: Vec<RowId>,
    pub style: StyleRefinement,
    /// What the list takes part in on backends that recognise gestures themselves: vertical
    /// scrolling, through the window's scroll arbiter like any scroll view.
    pub gestures: nd_gesture::GestureSet,
    /// A sidebar's list: rows inset from the edges, the selection a rounded inset highlight
    /// (the source list style on macOS).
    pub inset: bool,
    /// Rows drag as this kind ([`List::drag_rows`]).
    pub drag_kind: Option<SharedString>,
    /// The drag id of each rendered or selected row that drags.
    pub drag_ids: HashMap<RowId, SharedString>,
    /// The file each rendered or selected row carries out of the app ([`List::drag_out_files`]).
    pub drag_files: HashMap<RowId, PathBuf>,
    /// The rendered rows a drop may land on; every row when `None` ([`List::drop_on`]).
    pub drop_on: Option<HashSet<RowId>>,
}

/// Widget kind of the list container.
pub struct ListWidget;

/// What a presenter shows for one row action.
#[derive(Clone, Debug, PartialEq)]
pub struct RowActionSpec {
    pub title: SharedString,
    /// A Lucide icon name, mapped per platform (SF Symbols on macOS).
    pub icon: Option<SharedString>,
    pub destructive: bool,
    pub tint: Option<Hsla>,
}

/// A row's actions as presenters see them: leading and trailing, in order from the row's edge.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RowActionsSpec {
    pub leading: Vec<RowActionSpec>,
    pub trailing: Vec<RowActionSpec>,
    /// Swiping this edge all the way runs its first action.
    pub full_swipe: Option<RowEdge>,
}

/// Hung on each rendered row's root by [`RowView`], so presenters know which row a child is.
#[derive(Clone, Debug, PartialEq)]
pub struct RowTag {
    pub row: RowId,
    /// The row's swipe actions, when the list has `row_actions`. On macOS the table draws them;
    /// elsewhere the row draws them under nd-gesture's row swipe.
    pub actions: Option<RowActionsSpec>,
    /// Tree rows: nesting depth, whether the row has children, whether they show.
    pub depth: u16,
    pub expandable: bool,
    pub expanded: bool,
    /// False for a row the list neither selects nor highlights under the pointer, such as a
    /// strip of cells that are each their own button.
    pub selectable: bool,
}

/// Hung on each cell of a table row, with the column's index.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellTag(pub usize);

// Events presenters emit on the list node.

/// The viewport moved or resized: the rows the owner should render may have changed.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ViewportChanged {
    pub at_start: bool,
    pub at_end: bool,
}

/// The user changed the selection: `rows` in list order, empty when a click on empty space
/// cleared it. `cursor` is the row the click or key landed on, which a Shift range grows from
/// the anchor towards.
#[derive(Clone, Debug, PartialEq)]
pub struct RowsSelected {
    pub rows: Vec<RowId>,
    pub cursor: Option<RowId>,
}

impl RowsSelected {
    /// `rows` with the last of them as the cursor.
    pub fn new(rows: Vec<RowId>) -> Self {
        let cursor = rows.last().copied();
        Self { rows, cursor }
    }
}

/// The user activated a row (double click, Return).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RowActivated(pub RowId);

/// The user expanded or collapsed a tree row.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RowDisclosed {
    pub row: RowId,
    pub expanded: bool,
}

/// The user picked a row action through the native table: the `index`th action on `edge`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RowActionInvoked {
    pub row: RowId,
    pub edge: RowEdge,
    pub index: usize,
}

type Run = Rc<dyn Fn(&mut Window, &mut App)>;

/// One swipe action on a list row.
#[derive(Clone)]
pub struct RowAction {
    spec: RowActionSpec,
    run: Option<Run>,
}

/// A row action titled `title`; `.dispatch(action)` or `.on_invoke(..)` says what it does.
pub fn row_action(title: impl Into<SharedString>) -> RowAction {
    RowAction { spec: RowActionSpec { title: title.into(), icon: None, destructive: false, tint: None }, run: None }
}

impl RowAction {
    pub fn icon(mut self, lucide: impl Into<SharedString>) -> Self {
        self.spec.icon = Some(lucide.into());
        self
    }

    /// Shown in the platform's destructive style (red on macOS).
    pub fn destructive(mut self) -> Self {
        self.spec.destructive = true;
        self
    }

    pub fn tint(mut self, color: impl Into<Hsla>) -> Self {
        self.spec.tint = Some(color.into());
        self
    }

    /// Dispatches `action` in the row's window, as a menu item or key binding would.
    pub fn dispatch<A: Action>(mut self, action: A) -> Self {
        self.run = Some(Rc::new(move |window, cx| {
            cx.dispatch_action(window.id(), &action);
        }));
        self
    }

    pub fn on_invoke(mut self, f: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.run = Some(Rc::new(f));
        self
    }
}

/// A row's leading and trailing swipe actions.
#[derive(Clone, Default)]
pub struct RowActions {
    leading: Vec<RowAction>,
    trailing: Vec<RowAction>,
    full_swipe: Option<RowEdge>,
}

impl RowActions {
    pub fn new() -> Self {
        Self::default()
    }

    /// Revealed by swiping right (in left-to-right layouts), nearest the leading edge first.
    pub fn leading(mut self, action: RowAction) -> Self {
        self.leading.push(action);
        self
    }

    /// Revealed by swiping left, nearest the trailing edge first.
    pub fn trailing(mut self, action: RowAction) -> Self {
        self.trailing.push(action);
        self
    }

    /// Swiping this edge all the way runs its first action.
    pub fn full_swipe(mut self, edge: RowEdge) -> Self {
        self.full_swipe = Some(edge);
        self
    }

    fn spec(&self) -> RowActionsSpec {
        RowActionsSpec {
            leading: self.leading.iter().map(|a| a.spec.clone()).collect(),
            trailing: self.trailing.iter().map(|a| a.spec.clone()).collect(),
            full_swipe: self.full_swipe,
        }
    }

    fn get(&self, edge: RowEdge, index: usize) -> Option<&RowAction> {
        match edge {
            RowEdge::Leading => self.leading.get(index),
            RowEdge::Trailing => self.trailing.get(index),
        }
    }

    fn is_empty(&self) -> bool {
        self.leading.is_empty() && self.trailing.is_empty()
    }
}

/// Width of one drawn row action, in points.
pub const ROW_ACTION_WIDTH: f32 = 74.;

/// The user resized a column (by index).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColumnResized {
    pub column: usize,
    pub width: f32,
}

/// The user clicked a sortable column's header.
#[derive(Clone, Debug, PartialEq)]
pub struct SortChanged {
    pub column: SharedString,
    pub direction: SortDirection,
}

// Events the app handles.

/// A drag the list accepts was dropped on it: on the row `key`, or on no row (empty space, or a
/// row [`List::drop_on`] turns down).
#[derive(Clone, Debug, PartialEq)]
pub struct RowDropped<K> {
    pub key: Option<K>,
    pub item: nd_ui::DragItem,
    /// Held at the release: Control (Option on macOS) asks for a copy, Shift (Command on macOS)
    /// for a move.
    pub modifiers: nd_gesture::KeyModifiers,
    /// From the list's top-left.
    pub local: nd_core::Point,
}

/// The user selected `key` (the row the click or key landed on); `keys` is the whole selection
/// in list order.
#[derive(Clone, Debug, PartialEq)]
pub struct SelectEvent<K> {
    pub key: K,
    pub keys: Vec<K>,
}

/// Any change the user made to the selection, an emptied one included: `keys` in list order,
/// `cursor` the row the click or key landed on.
#[derive(Clone, Debug, PartialEq)]
pub struct SelectionChanged<K> {
    pub keys: Vec<K>,
    pub cursor: Option<K>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ActivateEvent<K> {
    pub key: K,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExpandEvent<K> {
    pub key: K,
    pub expanded: bool,
}

/// One node of a tree: its key, its parent (`None` for a root) and a version.
#[derive(Clone, Debug, PartialEq)]
pub struct TreeNode<K> {
    pub key: K,
    pub parent: Option<K>,
    pub version: u64,
}

type ActionsFn<K> = Rc<dyn Fn(K, &App) -> RowActions>;
type SelectableFn<K> = Rc<dyn Fn(&K, &App) -> bool>;
type Handler<E> = Rc<dyn Fn(&E, &mut Window, &mut App)>;
type RenderFn<K> = Rc<dyn Fn(K, &mut Window, &mut App) -> AnyElement>;
type CellFn<K> = Rc<dyn Fn(K, usize, &mut Window, &mut App) -> AnyElement>;

#[derive(Clone)]
enum Renderer<K> {
    Row(RenderFn<K>),
    Cells(CellFn<K>, Vec<f32>),
}

#[derive(Clone, Copy, Default)]
struct Meta {
    depth: u16,
    expandable: bool,
}

/// What a key was last set with.
struct Row {
    id: RowId,
    version: Option<u64>,
    meta: Meta,
    /// The `apply` that last listed the key.
    pass: u64,
}

struct Inner<K: RowKey> {
    core: Rc<RefCell<Core>>,
    owner: EntityId,
    /// The keys in order, index for index with the core's rows.
    keys: Vec<K>,
    rows: FxHashMap<K, Row>,
    pass: u64,
    next_id: u64,
    /// The rendered rows: the model's render range, in order.
    window: Vec<RowId>,
    views: HashMap<RowId, Entity<RowView<K>>>,
    renderer: Option<Renderer<K>>,
    row_actions: Option<ActionsFn<K>>,
    selectable: Option<SelectableFn<K>>,
    /// Each rendered row's actions as its last render built them, to run when picked.
    actions: HashMap<RowId, RowActions>,
    selected: Vec<K>,
    /// Tree: every node, and the keys whose children show.
    tree: Option<Vec<TreeNode<K>>>,
    expanded: HashSet<K>,
    near: (bool, bool),
}

impl<K: RowKey> Inner<K> {
    fn id(&self, key: &K) -> Option<RowId> {
        self.rows.get(key).map(|r| r.id)
    }

    fn key(&self, row: RowId) -> Option<&K> {
        self.core.borrow().index_of(row).and_then(|i| self.keys.get(i))
    }
}

/// An app's list: its keys, scroll position, selection and rendered rows. Cheap to clone; every
/// clone is the same list.
pub struct ListState<K: RowKey> {
    inner: Rc<RefCell<Inner<K>>>,
}

impl<K: RowKey> Clone for ListState<K> {
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone() }
    }
}

impl<K: RowKey> ListState<K> {
    /// A list owned by the view `cx` belongs to: it re-renders that view when the rows to show
    /// change.
    pub fn new<T: 'static>(cx: &mut Context<T>) -> Self {
        let owner = cx.entity_id();
        Self {
            inner: Rc::new(RefCell::new(Inner {
                core: Rc::new(RefCell::new(Core::new())),
                owner,
                keys: Vec::new(),
                rows: FxHashMap::default(),
                pass: 0,
                next_id: 1,
                window: Vec::new(),
                views: HashMap::new(),
                renderer: None,
                row_actions: None,
                selectable: None,
                actions: HashMap::new(),
                selected: Vec::new(),
                tree: None,
                expanded: HashSet::new(),
                near: (false, false),
            })),
        }
    }

    pub fn follow(self, follow: Follow) -> Self {
        self.core().borrow_mut().set_follow(follow);
        self
    }

    pub fn alignment(self, alignment: Alignment) -> Self {
        self.core().borrow_mut().set_alignment(alignment);
        if alignment == Alignment::Bottom {
            let follow = self.core().borrow().follow();
            self.core().borrow_mut().set_follow(follow);
        }
        self
    }

    /// Height assumed for rows not measured yet.
    pub fn estimated_row_height(self, height: Pixels) -> Self {
        self.core().borrow_mut().set_estimate(f64::from(height.as_f32()));
        self
    }

    /// Rows rendered beyond each edge of the viewport, in viewports (1 by default).
    pub fn overscan(self, viewports: f64) -> Self {
        self.core().borrow_mut().set_overscan(viewports);
        self
    }

    /// The model; presenters and tests read it.
    pub fn handle(&self) -> ListHandle {
        ListHandle(self.core())
    }

    fn core(&self) -> Rc<RefCell<Core>> {
        self.inner.borrow().core.clone()
    }

    pub fn len(&self) -> usize {
        self.inner.borrow().keys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.borrow().keys.is_empty()
    }

    /// The keys in order (a tree's visible rows).
    pub fn items(&self) -> Vec<K> {
        self.inner.borrow().keys.clone()
    }

    pub fn key_of(&self, row: RowId) -> Option<K> {
        self.inner.borrow().key(row).cloned()
    }

    pub fn row_of(&self, key: &K) -> Option<RowId> {
        self.inner.borrow().id(key)
    }

    /// Replaces the rows by key. Rows that stay keep their height and the scroll position stays
    /// on its row; there is no reset. Without versions every rendered row renders again; use
    /// [`Self::set_items_versioned`] so only changed rows do.
    pub fn set_items(&mut self, keys: impl IntoIterator<Item = K>, cx: &mut App) {
        self.apply(keys.into_iter().map(|k| (k, None, Meta::default())), cx);
    }

    /// As [`Self::set_items`], with a version per key: a rendered row renders again only when
    /// its version changed.
    pub fn set_items_versioned(&mut self, rows: impl IntoIterator<Item = (K, u64)>, cx: &mut App) {
        self.apply(rows.into_iter().map(|(k, v)| (k, Some(v), Meta::default())), cx);
    }

    /// A tree: nodes in display order among siblings, parents before children. Only rows under
    /// expanded parents are listed; expansion is kept here by key.
    pub fn set_tree(&mut self, nodes: impl IntoIterator<Item = TreeNode<K>>, cx: &mut App) {
        self.inner.borrow_mut().tree = Some(nodes.into_iter().collect());
        self.flatten_tree(cx);
    }

    pub fn is_expanded(&self, key: &K) -> bool {
        self.inner.borrow().expanded.contains(key)
    }

    pub fn set_expanded(&mut self, key: K, expanded: bool, cx: &mut App) {
        let changed = {
            let mut inner = self.inner.borrow_mut();
            if expanded { inner.expanded.insert(key) } else { inner.expanded.remove(&key) }
        };
        if changed {
            self.flatten_tree(cx);
            cx.notify(self.inner.borrow().owner);
        }
    }

    fn flatten_tree(&mut self, cx: &mut App) {
        let rows = {
            let inner = self.inner.borrow();
            let Some(nodes) = inner.tree.as_ref() else { return };
            let mut children: HashMap<Option<&K>, Vec<&TreeNode<K>>> = HashMap::new();
            for n in nodes {
                children.entry(n.parent.as_ref()).or_default().push(n);
            }
            let mut rows = Vec::new();
            let mut stack: Vec<(&TreeNode<K>, u16)> =
                children.get(&None).map(|r| r.iter().rev().map(|n| (*n, 0)).collect()).unwrap_or_default();
            while let Some((node, depth)) = stack.pop() {
                let kids = children.get(&Some(&node.key));
                let expandable = kids.is_some_and(|k| !k.is_empty());
                rows.push((node.key.clone(), Some(node.version), Meta { depth, expandable }));
                if expandable && inner.expanded.contains(&node.key) {
                    stack.extend(kids.into_iter().flatten().rev().map(|n| (*n, depth + 1)));
                }
            }
            rows
        };
        self.apply(rows, cx);
    }

    fn apply(&mut self, rows: impl IntoIterator<Item = (K, Option<u64>, Meta)>, cx: &mut App) {
        let mut stale = Vec::new();
        let mut meta_moved = false;
        {
            let mut guard = self.inner.borrow_mut();
            let inner = &mut *guard;
            inner.pass += 1;
            let pass = inner.pass;
            let tree = inner.tree.is_some();
            let rows = rows.into_iter();
            let mut ids = Vec::with_capacity(rows.size_hint().0);
            let mut keys = std::mem::take(&mut inner.keys);
            keys.clear();
            keys.reserve(ids.capacity());
            let mut meta = Vec::new();
            for (key, version, row_meta) in rows {
                let (id, changed, meta_changed) = match inner.rows.get_mut(&key) {
                    Some(row) => {
                        debug_assert!(row.pass != pass, "{key:?} is listed twice");
                        if row.pass == pass {
                            continue;
                        }
                        row.pass = pass;
                        let changed = match version {
                            Some(v) => row.version.replace(v).is_some_and(|old| old != v),
                            None => true,
                        };
                        let meta_changed =
                            row.meta.depth != row_meta.depth || row.meta.expandable != row_meta.expandable;
                        row.meta = row_meta;
                        (row.id, changed, meta_changed)
                    }
                    None => {
                        let id = RowId(inner.next_id);
                        inner.next_id += 1;
                        inner.rows.insert(key.clone(), Row { id, version, meta: row_meta, pass });
                        (id, version.is_none(), false)
                    }
                };
                meta_moved |= meta_changed;
                if (changed || meta_changed) && inner.views.contains_key(&id) {
                    stale.push(id);
                }
                if tree {
                    meta.push(crate::core::RowMeta {
                        depth: row_meta.depth,
                        expandable: row_meta.expandable,
                        expanded: inner.expanded.contains(&key),
                    });
                }
                ids.push(id);
                keys.push(key);
            }
            inner.keys = keys;
            // Every row listed is in `rows` once, so a longer map holds keys that left.
            if inner.rows.len() != inner.keys.len() {
                inner.rows.retain(|_, row| row.pass == pass);
                let rows = &inner.rows;
                inner.selected.retain(|k| rows.contains_key(k));
            }
            // The same rows at the same versions change nothing: an owner that hands its keys
            // over on every render must not get a new generation, and a re-render, back.
            if !meta_moved && stale.is_empty() && inner.core.borrow().ids() == ids.as_slice() {
                return;
            }
            inner.core.borrow_mut().set_rows(ids);
            let mut core = inner.core.borrow_mut();
            core.set_meta(meta);
            for id in &stale {
                core.invalidate(*id);
            }
        }
        for id in stale {
            let view = self.inner.borrow().views.get(&id).map(|v| v.entity_id());
            if let Some(view) = view {
                cx.notify(view);
            }
        }
        self.sync(cx);
        cx.notify(self.inner.borrow().owner);
    }

    /// Renders `key`'s row again and remeasures it, as after its content changed.
    pub fn invalidate(&mut self, key: &K, cx: &mut App) {
        let inner = self.inner.borrow();
        let Some(id) = inner.id(key) else { return };
        inner.core.borrow_mut().invalidate(id);
        if let Some(view) = inner.views.get(&id) {
            let view = view.entity_id();
            drop(inner);
            cx.notify(view);
        }
    }

    /// Renders every rendered row again, as after a setting their output depends on changed.
    pub fn invalidate_all(&mut self, cx: &mut App) {
        let views: Vec<EntityId> = {
            let inner = self.inner.borrow();
            let mut core = inner.core.borrow_mut();
            for id in inner.views.keys() {
                core.invalidate(*id);
            }
            inner.views.values().map(|v| v.entity_id()).collect()
        };
        for view in views {
            cx.notify(view);
        }
    }

    /// Scrolls `key` into place; `offset` moves the result (negative leaves room above it).
    /// Animated on the frame clock when the distance is short.
    pub fn scroll_to(&mut self, key: K, align: Align, offset: Pixels, cx: &mut App) {
        let Some(id) = self.row_of(&key) else { return };
        self.core().borrow_mut().scroll_to(id, align, f64::from(offset.as_f32()), true);
        self.sync(cx);
        cx.notify(self.inner.borrow().owner);
    }

    /// As [`Self::scroll_to`], without the animation.
    pub fn jump_to(&mut self, key: K, align: Align, offset: Pixels, cx: &mut App) {
        let Some(id) = self.row_of(&key) else { return };
        self.core().borrow_mut().scroll_to(id, align, f64::from(offset.as_f32()), false);
        self.sync(cx);
        cx.notify(self.inner.borrow().owner);
    }

    pub fn scroll_to_top(&mut self, cx: &mut App) {
        self.core().borrow_mut().scroll_to_start();
        self.sync(cx);
        cx.notify(self.inner.borrow().owner);
    }

    /// Jumps to the end; with `Follow::Tail` the list stays there as rows arrive.
    pub fn scroll_to_end(&mut self, cx: &mut App) {
        self.core().borrow_mut().scroll_to_end();
        self.sync(cx);
        cx.notify(self.inner.borrow().owner);
    }

    /// A user scroll by `dy` points, as a wheel would; tests and automation use it.
    pub fn scroll_by(&mut self, dy: f64, cx: &mut App) {
        self.core().borrow_mut().scroll_by(dy);
        self.sync(cx);
        cx.notify(self.inner.borrow().owner);
    }

    /// The viewport shows the end of the list.
    pub fn at_end(&self) -> bool {
        self.core().borrow().at_end()
    }

    pub fn is_following_tail(&self) -> bool {
        self.core().borrow().following()
    }

    pub fn select(&mut self, key: K, cx: &mut App) {
        self.set_selected(vec![key], cx);
    }

    pub fn set_selected(&mut self, keys: Vec<K>, cx: &mut App) {
        let mut inner = self.inner.borrow_mut();
        let keys: Vec<K> = keys.into_iter().filter(|k| inner.rows.contains_key(k)).collect();
        if inner.selected != keys {
            inner.selected = keys;
            let owner = inner.owner;
            drop(inner);
            cx.notify(owner);
        }
    }

    pub fn selected(&self) -> Vec<K> {
        self.inner.borrow().selected.clone()
    }

    /// The rows rendered now, in order.
    pub fn rendered(&self) -> Vec<K> {
        let inner = self.inner.borrow();
        inner.window.iter().filter_map(|id| inner.key(*id).cloned()).collect()
    }

    /// The row's view while it is rendered: its render count and node are the row's.
    pub fn row_view(&self, key: &K) -> Option<EntityId> {
        let inner = self.inner.borrow();
        inner.views.get(&inner.id(key)?).map(|v| v.entity_id())
    }

    /// Brings the rendered rows in line with the model's render range: creates views for rows
    /// that entered it, drops those that left, and re-renders the owner when the set changed.
    pub fn sync(&self, cx: &mut App) -> bool {
        let wanted: Vec<RowId> = {
            let inner = self.inner.borrow();
            let core = inner.core.borrow();
            core.render_range().filter_map(|i| core.row(i)).collect()
        };
        let mut inner = self.inner.borrow_mut();
        if inner.window == wanted {
            return false;
        }
        let keep: HashSet<RowId> = wanted.iter().copied().collect();
        inner.views.retain(|id, _| keep.contains(id));
        inner.actions.retain(|id, _| keep.contains(id));
        let weak = Rc::downgrade(&self.inner);
        let missing: Vec<(RowId, K)> = wanted
            .iter()
            .filter(|id| !inner.views.contains_key(id))
            .filter_map(|id| inner.key(*id).map(|k| (*id, k.clone())))
            .collect();
        inner.window = wanted;
        let owner = inner.owner;
        drop(inner);
        for (id, key) in missing {
            let list = weak.clone();
            let view = cx.new(|_| RowView { key, row: id, list, swipe: 0. });
            self.inner.borrow_mut().views.insert(id, view);
        }
        cx.notify(owner);
        true
    }

    /// The viewport moved: renders the rows it now needs and reports edges crossed.
    fn on_viewport(&self, event: &ViewportChanged, cx: &mut App) -> (bool, bool) {
        self.sync(cx);
        let mut inner = self.inner.borrow_mut();
        let (was_start, was_end) = inner.near;
        inner.near = (event.at_start, event.at_end);
        (event.at_start && !was_start, event.at_end && !was_end)
    }

    /// Runs the `index`th action on `edge` of `row`, as its last render built it.
    pub fn invoke_action(&self, row: RowId, edge: RowEdge, index: usize, window: &mut Window, cx: &mut App) {
        let run = self.inner.borrow().actions.get(&row).and_then(|a| a.get(edge, index)).and_then(|a| a.run.clone());
        if let Some(run) = run {
            run(window, cx);
        }
    }

    fn props(&self, kind: ListKind, selection: Selection, style: StyleRefinement, inset: bool) -> ListProps {
        let inner = self.inner.borrow();
        let generation = inner.core.borrow().generation();
        let selected = inner.selected.iter().filter_map(|k| inner.id(k)).collect();
        let gestures = nd_gesture::GestureSet {
            scroll: nd_gesture::Axes { horizontal: false, vertical: true },
            ..Default::default()
        };
        ListProps {
            handle: ListHandle(inner.core.clone()),
            kind,
            generation,
            selection,
            selected,
            style,
            gestures,
            inset,
            drag_kind: None,
            drag_ids: HashMap::new(),
            drag_files: HashMap::new(),
            drop_on: None,
        }
    }

    /// The rows a presenter can start a drag from or drop on: those rendered, and the selection.
    fn live_rows(&self) -> Vec<(RowId, K)> {
        let inner = self.inner.borrow();
        let selected = inner.selected.iter().filter_map(|k| Some((inner.id(k)?, k.clone())));
        let rendered = inner.window.iter().filter_map(|id| Some((*id, inner.key(*id)?.clone())));
        rendered.chain(selected).collect()
    }

    fn children(&self) -> Vec<AnyElement> {
        let inner = self.inner.borrow();
        inner.window.iter().filter_map(|id| inner.views.get(id)).map(|v| v.clone().into_any_element()).collect()
    }
}

/// One rendered row: calls the list's row closure for its key and tags the output with the row's
/// id. Re-renders only when notified, never because its owner did.
pub struct RowView<K: RowKey> {
    key: K,
    row: RowId,
    list: Weak<RefCell<Inner<K>>>,
    /// The row content's horizontal displacement while a swipe reveals its drawn actions.
    swipe: f32,
}

impl<K: RowKey> Render for RowView<K> {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(list) = self.list.upgrade() else { return div().into_any_element() };
        let (renderer, meta, expanded, actions_fn, selectable_fn) = {
            let inner = list.borrow();
            let meta = inner.rows.get(&self.key).filter(|r| r.id == self.row).map(|r| r.meta).unwrap_or_default();
            (
                inner.renderer.clone(),
                meta,
                inner.expanded.contains(&self.key),
                inner.row_actions.clone(),
                inner.selectable.clone(),
            )
        };
        let selectable = selectable_fn.is_none_or(|f| f(&self.key, cx));
        let actions = actions_fn.map(|f| f(self.key.clone(), cx)).filter(|a| !a.is_empty());
        let spec = actions.as_ref().map(RowActions::spec);
        if let Some(actions) = actions.clone() {
            list.borrow_mut().actions.insert(self.row, actions);
        } else {
            list.borrow_mut().actions.remove(&self.row);
        }
        let tag = RowTag {
            row: self.row,
            actions: spec.clone(),
            depth: meta.depth,
            expandable: meta.expandable,
            expanded,
            selectable,
        };
        let swipe = self.swipe;
        let this = cx.weak_entity();
        let row = div().w_full().flex().flex_col();
        let app: &mut App = cx;
        let content = match renderer {
            Some(Renderer::Row(render)) => row.child(render(self.key.clone(), window, app)),
            Some(Renderer::Cells(render, widths)) => {
                let cells = widths.iter().enumerate().map(|(i, w)| {
                    div()
                        .id(format!("cell-{i}"))
                        .scope(Scope::new(CellTag(i)))
                        .w(px(*w))
                        .flex_shrink_0()
                        .overflow_hidden()
                        .child(render(self.key.clone(), i, window, app))
                        .into_any_element()
                });
                row.flex_row().children(cells.collect::<Vec<_>>())
            }
            None => row,
        };
        let (Some(spec), Some(actions)) = (spec, actions) else {
            return content.scope(Scope::new(tag)).into_any_element();
        };
        // Every backend but AppKit: the row draws its actions under the content and follows
        // nd-gesture's row swipe. AppKit's table draws them itself and never sends the swipe.
        let width = |n: usize| n as f32 * ROW_ACTION_WIDTH;
        let config = nd_gesture::RowSwipeConfig {
            leading: width(spec.leading.len()),
            trailing: width(spec.trailing.len()),
            full_swipe: spec.full_swipe,
        };
        let row = self.row;
        let list_weak = self.list.clone();
        let wrapper = div().scope(Scope::new(tag)).relative().w_full().overflow_hidden().on_row_swipe(
            config,
            move |e: &RowSwipeEvent, window, cx| {
                this.update(cx, |this: &mut RowView<K>, cx| {
                    this.swipe = e.offset;
                    cx.notify();
                });
                if e.full
                    && e.phase == nd_gesture::GesturePhase::Ended
                    && let Some(edge) = e.open
                    && let Some(list) = list_weak.upgrade()
                {
                    let state = ListState { inner: list };
                    state.invoke_action(row, edge, 0, window, cx);
                }
            },
        );
        let wrapper = if swipe == 0. {
            wrapper
        } else {
            let button = |a: &RowAction, edge: RowEdge, index: usize| {
                let list_weak = self.list.clone();
                let bg = a.spec.tint.unwrap_or(if a.spec.destructive {
                    hsla(0., 0.75, 0.5, 1.)
                } else {
                    hsla(0., 0., 0.55, 1.)
                });
                div()
                    .w(px(ROW_ACTION_WIDTH))
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(bg)
                    .text_color(hsla(0., 0., 1., 1.))
                    .child(text(a.spec.title.clone()))
                    .on_tap(move |_: &TapEvent, window, cx| {
                        if let Some(list) = list_weak.upgrade() {
                            ListState { inner: list }.invoke_action(row, edge, index, window, cx);
                        }
                    })
                    .into_any_element()
            };
            let leading: Vec<AnyElement> =
                actions.leading.iter().enumerate().map(|(i, a)| button(a, RowEdge::Leading, i)).collect();
            let trailing: Vec<AnyElement> =
                actions.trailing.iter().enumerate().rev().map(|(i, a)| button(a, RowEdge::Trailing, i)).collect();
            wrapper.child(
                h_flex()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .justify_between()
                    .child(h_flex().h_full().children(leading))
                    .child(h_flex().h_full().children(trailing)),
            )
        };
        let content = content.relative().left(px(swipe));
        wrapper.child(content).into_any_element()
    }
}

/// A virtualized container of any [`ListKind`] under construction.
pub struct List<K: RowKey> {
    id: SharedString,
    state: ListState<K>,
    kind: ListKind,
    renderer: Renderer<K>,
    style: StyleRefinement,
    selection: Selection,
    inset: bool,
    empty: Option<AnyElement>,
    on_select: Option<Handler<SelectEvent<K>>>,
    on_selection_change: Option<Handler<SelectionChanged<K>>>,
    on_activate: Option<Handler<ActivateEvent<K>>>,
    on_expand: Option<Handler<ExpandEvent<K>>>,
    on_near_start: Option<Handler<()>>,
    on_near_end: Option<Handler<()>>,
    on_column_resize: Option<Handler<ColumnResized>>,
    on_sort: Option<Handler<SortChanged>>,
    drag: Option<(SharedString, DragIdFn<K>)>,
    drag_files: Option<DragFileFn<K>>,
    accepts: Vec<SharedString>,
    drop_on: Option<DropOnFn<K>>,
    on_drop: Option<Handler<RowDropped<K>>>,
}

type DragIdFn<K> = Rc<dyn Fn(&K) -> Option<SharedString>>;
type DragFileFn<K> = Rc<dyn Fn(&K) -> Option<PathBuf>>;
type DropOnFn<K> = Rc<dyn Fn(&K) -> bool>;

fn build<K: RowKey>(id: SharedString, state: &ListState<K>, kind: ListKind, renderer: Renderer<K>) -> List<K> {
    List {
        id,
        state: state.clone(),
        kind,
        renderer,
        style: StyleRefinement::default(),
        selection: Selection::None,
        inset: false,
        empty: None,
        on_select: None,
        on_selection_change: None,
        on_activate: None,
        on_expand: None,
        on_near_start: None,
        on_near_end: None,
        on_column_resize: None,
        on_sort: None,
        drag: None,
        drag_files: None,
        accepts: Vec::new(),
        drop_on: None,
        on_drop: None,
    }
}

fn row_fn<K: RowKey, E: IntoElement>(render: impl Fn(K, &mut Window, &mut App) -> E + 'static) -> RenderFn<K> {
    Rc::new(move |k, w, cx| render(k, w, cx).into_any_element())
}

/// A virtualized list: `render_row` runs only for rows the model renders, outside the owner's
/// render and outside native callbacks.
pub fn list<K: RowKey, E: IntoElement>(
    id: impl Into<SharedString>,
    state: &ListState<K>,
    render_row: impl Fn(K, &mut Window, &mut App) -> E + 'static,
) -> List<K> {
    build(id.into(), state, ListKind::List, Renderer::Row(row_fn(render_row))).size_full()
}

/// A table: one cell per column, `render_cell(key, column_index, ..)`.
pub fn table<K: RowKey, E: IntoElement>(
    id: impl Into<SharedString>,
    state: &ListState<K>,
    columns: Vec<Column>,
    render_cell: impl Fn(K, usize, &mut Window, &mut App) -> E + 'static,
) -> List<K> {
    let widths = columns.iter().map(|c| c.width).collect();
    let cell: CellFn<K> = Rc::new(move |k, c, w, cx| render_cell(k, c, w, cx).into_any_element());
    build(id.into(), state, ListKind::Table(columns), Renderer::Cells(cell, widths)).size_full()
}

/// Fixed-size cells in as many lanes as fit the width.
pub fn grid<K: RowKey, E: IntoElement>(
    id: impl Into<SharedString>,
    state: &ListState<K>,
    item_size: (Pixels, Pixels),
    render_item: impl Fn(K, &mut Window, &mut App) -> E + 'static,
) -> List<K> {
    let (w, h) = (f64::from(item_size.0.as_f32()), f64::from(item_size.1.as_f32()));
    {
        let core = state.core();
        let mut core = core.borrow_mut();
        let gap = core.grid().map_or(8., |g| g.gap);
        core.set_grid(Some(GridSpec { item_width: w, item_height: h, gap }));
    }
    build(id.into(), state, ListKind::Grid, Renderer::Row(row_fn(render_item))).size_full()
}

/// A tree over [`ListState::set_tree`]; disclosure and indentation are the presenter's.
pub fn tree<K: RowKey, E: IntoElement>(
    id: impl Into<SharedString>,
    state: &ListState<K>,
    render_row: impl Fn(K, &mut Window, &mut App) -> E + 'static,
) -> List<K> {
    build(id.into(), state, ListKind::Tree, Renderer::Row(row_fn(render_row))).size_full()
}

impl<K: RowKey> List<K> {
    pub fn selection(mut self, selection: Selection) -> Self {
        self.selection = selection;
        self
    }

    /// Shown in place of the rows while the list is empty.
    pub fn empty(mut self, empty: impl IntoElement) -> Self {
        self.empty = Some(empty.into_any_element());
        self
    }

    /// Gap between grid cells.
    pub fn gap(self, gap: Pixels) -> Self {
        {
            let core = self.state.core();
            let mut core = core.borrow_mut();
            if let Some(mut g) = core.grid() {
                g.gap = f64::from(gap.as_f32());
                core.set_grid(Some(g));
            }
        }
        self
    }

    pub fn estimated_row_height(self, height: Pixels) -> Self {
        self.state.core().borrow_mut().set_estimate(f64::from(height.as_f32()));
        self
    }

    /// Swipe actions per row. On macOS the table draws them in the row view; elsewhere the row
    /// draws them and follows nd-gesture's row swipe.
    pub fn row_actions(self, f: impl Fn(K, &App) -> RowActions + 'static) -> Self {
        self.state.inner.borrow_mut().row_actions = Some(Rc::new(f));
        self
    }

    /// Which rows take selection and the pointer's highlight; every row when unset.
    pub fn selectable(self, f: impl Fn(&K, &App) -> bool + 'static) -> Self {
        self.state.inner.borrow_mut().selectable = Some(Rc::new(f));
        self
    }

    /// A sidebar's list: the platform's inset rows and rounded selection (`NSTableView`'s source
    /// list style on macOS, a rounded inset highlight elsewhere).
    pub fn inset(mut self) -> Self {
        self.inset = true;
        self
    }

    pub fn on_select(mut self, f: impl Fn(&SelectEvent<K>, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Rc::new(f));
        if self.selection == Selection::None {
            self.selection = Selection::Single;
        }
        self
    }

    /// Every change the user makes to the selection, including a click on empty space that
    /// clears it, which [`Self::on_select`] does not see.
    pub fn on_selection_change(mut self, f: impl Fn(&SelectionChanged<K>, &mut Window, &mut App) + 'static) -> Self {
        self.on_selection_change = Some(Rc::new(f));
        if self.selection == Selection::None {
            self.selection = Selection::Single;
        }
        self
    }

    pub fn on_activate(mut self, f: impl Fn(&ActivateEvent<K>, &mut Window, &mut App) + 'static) -> Self {
        self.on_activate = Some(Rc::new(f));
        self
    }

    pub fn on_expand(mut self, f: impl Fn(&ExpandEvent<K>, &mut Window, &mut App) + 'static) -> Self {
        self.on_expand = Some(Rc::new(f));
        self
    }

    /// The viewport reached the first rows: load older ones (infinite scroll up).
    pub fn on_near_start(mut self, f: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_near_start = Some(Rc::new(move |_: &(), w: &mut Window, cx: &mut App| f(w, cx)));
        self
    }

    pub fn on_near_end(mut self, f: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_near_end = Some(Rc::new(move |_: &(), w: &mut Window, cx: &mut App| f(w, cx)));
        self
    }

    pub fn on_column_resize(mut self, f: impl Fn(&ColumnResized, &mut Window, &mut App) + 'static) -> Self {
        self.on_column_resize = Some(Rc::new(f));
        self
    }

    pub fn on_sort(mut self, f: impl Fn(&SortChanged, &mut Window, &mut App) + 'static) -> Self {
        self.on_sort = Some(Rc::new(f));
        self
    }

    /// Rows drag as `kind`, each carrying `id(key)`; a row whose id is `None` does not drag.
    /// Dragging a selected row drags the whole selection in list order; an unselected row drags
    /// alone and leaves the selection as it was. A press on a row that drags selects it on
    /// release, so a drag never reports a selection. Targets anywhere in the app's windows take
    /// it with `accepts_drops([kind])`; the GPUI presenter draws the row under the pointer with a
    /// count badge for several, AppKit's dragging session the rows' own image.
    pub fn drag_rows(
        mut self,
        kind: impl Into<SharedString>,
        id: impl Fn(&K) -> Option<SharedString> + 'static,
    ) -> Self {
        self.drag = Some((kind.into(), Rc::new(id)));
        self
    }

    /// Rows dragged by [`drag_rows`](Self::drag_rows) out of the app's windows carry
    /// `path(key)` to the app they land in (a browser, a chat app, a file manager) as a
    /// `text/uri-list` of file URIs; rows with `None` leave nothing there. Inside the app the drag
    /// is the same `drag_rows` drag. Only where `nd::platform::has(Feature::DragFilesOut)`: the
    /// GPUI backend under Wayland. Elsewhere the rows still drag inside the app.
    pub fn drag_out_files(mut self, path: impl Fn(&K) -> Option<PathBuf> + 'static) -> Self {
        self.drag_files = Some(Rc::new(path));
        self
    }

    /// The list takes drops of these kinds: on the row under the pointer, highlighted while the
    /// drag is over it, or on empty space. [`DragItem::FILES`](nd_ui::DragItem::FILES) takes
    /// files dragged in from outside the app.
    pub fn accepts_drops(mut self, kinds: impl IntoIterator<Item = impl Into<SharedString>>) -> Self {
        self.accepts = kinds.into_iter().map(Into::into).collect();
        self
    }

    /// Only these rows take a drop on themselves (a file manager's folders); a drop over any other
    /// lands on no row.
    pub fn drop_on(mut self, f: impl Fn(&K) -> bool + 'static) -> Self {
        self.drop_on = Some(Rc::new(f));
        self
    }

    pub fn on_drop(mut self, f: impl Fn(&RowDropped<K>, &mut Window, &mut App) + 'static) -> Self {
        self.on_drop = Some(Rc::new(f));
        self
    }
}

impl<K: RowKey> Styled for List<K> {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl<K: RowKey> FluentBuilder for List<K> {}

impl<K: RowKey> IntoElement for List<K> {
    fn into_any_element(self) -> AnyElement {
        let state = self.state;
        if state.is_empty()
            && let Some(empty) = self.empty
        {
            let mut d = div().id(format!("{}-empty", self.id)).flex().flex_col().items_center().justify_center();
            *d.style() = self.style.clone();
            return d.flex().flex_col().items_center().justify_center().child(empty).into_any_element();
        }
        state.inner.borrow_mut().renderer = Some(self.renderer);
        let mut props = state.props(self.kind, self.selection, self.style.clone(), self.inset);
        if self.drag.is_some() || self.drop_on.is_some() {
            let live = state.live_rows();
            if let Some((kind, id)) = &self.drag {
                props.drag_kind = Some(kind.clone());
                props.drag_ids = live.iter().filter_map(|(r, k)| Some((*r, id(k)?))).collect();
                if let Some(path) = &self.drag_files {
                    props.drag_files = live.iter().filter_map(|(r, k)| Some((*r, path(k)?))).collect();
                }
            }
            if let Some(f) = &self.drop_on {
                props.drop_on = Some(live.iter().filter(|(_, k)| f(k)).map(|(r, _)| *r).collect());
            }
        }
        let mut el = ContainerElement::new::<ListWidget>(Key::Id(self.id), props);
        // A list that selects takes keyboard focus as NSTableView does: `window.focus(id)` and a
        // press reach it, and the bindings of the contexts around it match while it holds it.
        if self.selection != Selection::None {
            el = el.focusable();
        }
        if !self.accepts.is_empty() {
            el = el.accepts_drops(self.accepts);
        }
        *el.style() = self.style;
        let s = state.clone();
        let (near_start, near_end) = (self.on_near_start, self.on_near_end);
        let el = el.children(state.children()).on(move |e: &ViewportChanged, window, cx| {
            let (start, end) = s.on_viewport(e, cx);
            if start && let Some(f) = &near_start {
                f(&(), window, cx);
            }
            if end && let Some(f) = &near_end {
                f(&(), window, cx);
            }
        });
        let s = state.clone();
        let (on_select, on_change) = (self.on_select, self.on_selection_change);
        let el = el.on(move |e: &RowsSelected, window, cx| {
            let selectable = s.inner.borrow().selectable.clone();
            let keys: Vec<K> = e
                .rows
                .iter()
                .filter_map(|r| s.key_of(*r))
                .filter(|k| selectable.as_ref().is_none_or(|f| f(k, cx)))
                .collect();
            let cursor = e.cursor.and_then(|r| s.key_of(r)).filter(|k| keys.contains(k));
            s.clone().set_selected(keys.clone(), cx);
            if let Some(f) = &on_change {
                f(&SelectionChanged { keys: keys.clone(), cursor: cursor.clone() }, window, cx);
            }
            if let (Some(f), Some(key)) = (&on_select, cursor.or_else(|| keys.last().cloned())) {
                f(&SelectEvent { key, keys }, window, cx);
            }
        });
        let s = state.clone();
        let on_activate = self.on_activate;
        let el = el.on(move |e: &RowActivated, window, cx| {
            if let (Some(f), Some(key)) = (&on_activate, s.key_of(e.0)) {
                f(&ActivateEvent { key }, window, cx);
            }
        });
        let s = state.clone();
        let on_expand = self.on_expand;
        let el = el.on(move |e: &RowDisclosed, window, cx| {
            let Some(key) = s.key_of(e.row) else { return };
            s.clone().set_expanded(key.clone(), e.expanded, cx);
            if let Some(f) = &on_expand {
                f(&ExpandEvent { key, expanded: e.expanded }, window, cx);
            }
        });
        let s = state.clone();
        let el = el.on(move |e: &RowActionInvoked, window, cx| s.invoke_action(e.row, e.edge, e.index, window, cx));
        let on_resize = self.on_column_resize;
        let el = el.on(move |e: &ColumnResized, window, cx| {
            if let Some(f) = &on_resize {
                f(e, window, cx);
            }
        });
        let s = state.clone();
        let on_drop = self.on_drop;
        let el = el.on(move |e: &nd_ui::Dropped, window, cx| {
            let Some(f) = &on_drop else { return };
            let key = e.slot.as_ref().and_then(|slot| slot.on.as_ref()).and_then(|on| {
                let row = RowId(on.parse().ok()?);
                s.key_of(row)
            });
            f(&RowDropped { key, item: e.item.clone(), modifiers: e.modifiers, local: e.local }, window, cx);
        });
        let on_sort = self.on_sort;
        el.on(move |e: &SortChanged, window, cx| {
            if let Some(f) = &on_sort {
                f(e, window, cx);
            }
        })
        .into_any_element()
    }
}
