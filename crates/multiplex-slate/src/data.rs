//! Data views: host cards, tables and lists, the inspector, callouts, and
//! empty states.

use std::rc::Rc;

use gpui::{
    AnyElement, App, ClickEvent, Div, ElementId, FontWeight, Hsla, InteractiveElement,
    Interactivity, IntoElement, ParentElement, Pixels, RenderOnce, SharedString, Stateful,
    StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder as _,
};

use crate::{
    StatusKind,
    button::{Button, ControlSize, IconButton},
    icon::{Icon, IconName, IconSize},
    status::StatusGlyph,
    theme::{ActiveTheme, SlateStyled, SlateTheme, focus_ring_shadows},
};

type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

/// The group a host belongs to. Its color tints the host tile and dot, and is
/// always shown with the group name.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostGroup {
    Production,
    Staging,
    Lab,
    Jump,
    /// The local shell profile.
    Local,
}

impl HostGroup {
    pub fn label(self) -> &'static str {
        match self {
            Self::Production => "Production",
            Self::Staging => "Staging",
            Self::Lab => "Lab",
            Self::Jump => "Jump",
            Self::Local => "Local",
        }
    }

    fn color(self, theme: &SlateTheme) -> Hsla {
        let c = &theme.colors;
        match self {
            Self::Production => c.group_production,
            Self::Staging => c.group_staging,
            Self::Lab => c.group_lab,
            Self::Jump => c.group_jump,
            Self::Local => c.text_secondary,
        }
    }

    /// Tile fill strength. Lab's hue reads lighter, so it gets a touch more.
    fn tint(self) -> f32 {
        match self {
            Self::Lab => 0.16,
            _ => 0.14,
        }
    }
}

/// The square mark at the start of a host card or row.
#[derive(IntoElement)]
pub struct HostTile {
    group: HostGroup,
    size: Option<Pixels>,
}

impl HostTile {
    pub fn new(group: HostGroup) -> Self {
        Self { group, size: None }
    }

    /// Overrides the tile size, for list rows.
    pub fn size(mut self, size: Pixels) -> Self {
        self.size = Some(size);
        self
    }
}

impl RenderOnce for HostTile {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.slate();
        let m = &theme.metrics;
        let side = self.size.unwrap_or(m.card_tile);
        let local = self.group == HostGroup::Local;
        let color = self.group.color(&theme);
        let (fill, glyph, icon) = if local {
            (
                theme.colors.control,
                theme.colors.text_secondary,
                IconName::Local,
            )
        } else {
            (color.opacity(self.group.tint()), color, IconName::Server)
        };
        div()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(side)
            .rounded(m.radius_panel)
            .bg(fill)
            .child(Icon::new(icon).size(IconSize::Medium).color(glyph))
    }
}

/// A host you can pick up and open. The only card in the system.
#[derive(IntoElement)]
pub struct HostCard {
    base: Stateful<Div>,
    id: ElementId,
    name: SharedString,
    subtitle: SharedString,
    group: HostGroup,
    status: Option<(StatusKind, SharedString)>,
    action: Option<(SharedString, ClickHandler)>,
    selected: bool,
}

impl HostCard {
    pub fn new(id: impl Into<ElementId>, name: impl Into<SharedString>, group: HostGroup) -> Self {
        let id = id.into();
        Self {
            base: div().id(id.clone()),
            id,
            name: name.into(),
            subtitle: SharedString::default(),
            group,
            status: None,
            action: None,
            selected: false,
        }
    }

    /// `user · group · via jump host`, and the port when it is not 22.
    pub fn subtitle(mut self, subtitle: impl Into<SharedString>) -> Self {
        self.subtitle = subtitle.into();
        self
    }

    /// Shown only when the host is not idle.
    pub fn status(mut self, status: StatusKind, label: impl Into<SharedString>) -> Self {
        if status != StatusKind::Idle {
            self.status = Some((status, label.into()));
        }
        self
    }

    /// The hover action, such as Open, Connect, or Review. It replaces the
    /// status while the card is hovered or selected.
    pub fn action(
        mut self,
        label: impl Into<SharedString>,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.action = Some((label.into(), Rc::new(handler)));
        self
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }
}

impl InteractiveElement for HostCard {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.base.interactivity()
    }
}

impl StatefulInteractiveElement for HostCard {}

impl RenderOnce for HostCard {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.slate();
        let c = &theme.colors;
        let m = &theme.metrics;
        let group_name = SharedString::from(format!("host-card-{:?}", self.id));
        let hover_border = c.border;
        let ring = focus_ring_shadows(&theme);
        let selected = self.selected;

        self.base
            .group(group_name.clone())
            .relative()
            .flex()
            .items_center()
            .min_w_0()
            .h(m.card_height)
            .px(m.space[4])
            .gap(m.space[4])
            .rounded(m.radius_panel)
            .border(m.hairline)
            .cursor_pointer()
            .font_family(theme.typography.ui_family.clone())
            .map(|this| {
                if selected {
                    this.bg(c.selected).border_color(c.border_strong)
                } else {
                    this.bg(c.elevated)
                        .border_color(c.border_subtle)
                        .hover(move |style| style.border_color(hover_border))
                }
            })
            .focus_visible(move |style| style.shadow(ring))
            .child(HostTile::new(self.group))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_color(c.text)
                            .type_style(theme.typography.heading_small)
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(self.name),
                    )
                    .child(
                        div()
                            .text_color(c.text_muted)
                            .type_style(theme.typography.caption)
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(self.subtitle),
                    ),
            )
            .when_some(self.status, |this, (status, label)| {
                let status = div()
                    .flex_none()
                    .child(StatusGlyph::new(status).label(label));
                this.child(if self.action.is_some() {
                    status
                        .when(selected, |status| status.invisible())
                        .group_hover(group_name.clone(), |style| style.invisible())
                } else {
                    status
                })
            })
            .when_some(self.action, |this, (label, handler)| {
                this.child(
                    div()
                        .absolute()
                        .right(m.space[4])
                        .top_0()
                        .bottom_0()
                        .flex()
                        .items_center()
                        .when(!selected, |action| {
                            action
                                .invisible()
                                .group_hover(group_name, |style| style.visible())
                        })
                        .child(
                            Button::new("action")
                                .label(label)
                                .size(ControlSize::Small)
                                .on_click(move |event, window, cx| {
                                    cx.stop_propagation();
                                    handler(event, window, cx)
                                }),
                        ),
                )
            })
    }
}

/// Lays host cards out in equal columns with 8 px gaps.
#[derive(IntoElement)]
pub struct HostGrid {
    columns: u16,
    children: Vec<AnyElement>,
}

impl HostGrid {
    /// Three columns, or two while the inspector is open.
    pub fn new(columns: u16) -> Self {
        Self {
            columns: columns.max(1),
            children: Vec::new(),
        }
    }
}

impl ParentElement for HostGrid {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for HostGrid {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let m = &cx.slate().metrics;
        div()
            .grid()
            .grid_cols(self.columns)
            .gap(m.space[3])
            .w_full()
            .children(self.children)
    }
}

/// How wide a table column is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ColumnWidth {
    /// A share of the remaining width.
    Flex(f32),
    Fixed(Pixels),
}

/// A table column definition.
#[derive(Clone, Debug)]
pub struct TableColumn {
    title: SharedString,
    width: ColumnWidth,
    optional: bool,
}

impl TableColumn {
    pub fn new(title: impl Into<SharedString>, width: ColumnWidth) -> Self {
        Self {
            title: title.into(),
            width,
            optional: false,
        }
    }

    /// Dropped entirely in narrow mode, never squeezed to a sliver.
    pub fn optional(mut self) -> Self {
        self.optional = true;
        self
    }
}

/// The content of one table cell.
pub enum TableCell {
    /// Primary text; bold on the selected row.
    Primary(SharedString),
    /// Secondary column text.
    Muted(SharedString),
    /// A technical value in mono.
    Mono(SharedString),
    /// A status glyph and label.
    Status(StatusKind, SharedString),
    Element(AnyElement),
}

impl TableCell {
    pub fn primary(text: impl Into<SharedString>) -> Self {
        Self::Primary(text.into())
    }

    pub fn muted(text: impl Into<SharedString>) -> Self {
        Self::Muted(text.into())
    }

    pub fn mono(text: impl Into<SharedString>) -> Self {
        Self::Mono(text.into())
    }

    pub fn status(kind: StatusKind, label: impl Into<SharedString>) -> Self {
        Self::Status(kind, label.into())
    }

    pub fn element(element: impl IntoElement) -> Self {
        Self::Element(element.into_any_element())
    }
}

/// One table row.
pub struct TableRow {
    id: ElementId,
    cells: Vec<TableCell>,
    selected: bool,
    action: Option<AnyElement>,
    on_click: Option<ClickHandler>,
}

impl TableRow {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            cells: Vec::new(),
            selected: false,
            action: None,
            on_click: None,
        }
    }

    pub fn cell(mut self, cell: TableCell) -> Self {
        self.cells.push(cell);
        self
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// A small button, visible while the row is hovered or selected.
    pub fn action(mut self, action: impl IntoElement) -> Self {
        self.action = Some(action.into_any_element());
        self
    }

    /// Row activation. Check `event.click_count()` for double-click.
    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }
}

/// Rows separated by fills, not borders, under a single header hairline.
#[derive(IntoElement)]
pub struct Table {
    id: ElementId,
    columns: Vec<TableColumn>,
    rows: Vec<TableRow>,
    narrow: bool,
    list_rows: bool,
}

impl Table {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            columns: Vec::new(),
            rows: Vec::new(),
            narrow: false,
            list_rows: false,
        }
    }

    pub fn column(mut self, column: TableColumn) -> Self {
        self.columns.push(column);
        self
    }

    pub fn row(mut self, row: TableRow) -> Self {
        self.rows.push(row);
        self
    }

    pub fn rows(mut self, rows: impl IntoIterator<Item = TableRow>) -> Self {
        self.rows.extend(rows);
        self
    }

    /// Drops optional columns, for when the inspector is open.
    pub fn narrow(mut self, narrow: bool) -> Self {
        self.narrow = narrow;
        self
    }

    /// Uses the taller 32 px list row instead of the 30 px table row.
    pub fn list_rows(mut self, list_rows: bool) -> Self {
        self.list_rows = list_rows;
        self
    }
}

fn sized_cell(width: ColumnWidth) -> Div {
    let cell = div().flex().items_center().min_w_0().overflow_hidden();
    match width {
        ColumnWidth::Flex(grow) => cell
            .flex_grow(grow)
            .flex_shrink(1.)
            .flex_basis(gpui::relative(0.)),
        ColumnWidth::Fixed(width) => cell.flex_none().w(width),
    }
}

impl RenderOnce for Table {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.slate();
        let c = &theme.colors;
        let m = &theme.metrics;
        let visible: Vec<usize> = self
            .columns
            .iter()
            .enumerate()
            .filter(|(_, column)| !(self.narrow && column.optional))
            .map(|(index, _)| index)
            .collect();
        let row_height = if self.list_rows {
            m.list_row
        } else {
            m.table_row
        };
        let row_padding = |row: Div| row.pl(m.space[5]).pr(m.space[4]);
        let hover_bg = c.hover;

        let header = row_padding(div())
            .flex()
            .flex_none()
            .items_center()
            .h(m.control_default + m.space[1])
            .border_b(m.hairline)
            .border_color(c.border_subtle)
            .text_color(c.text_faint)
            .type_style(theme.typography.caption)
            .children(visible.iter().map(|&index| {
                let column = &self.columns[index];
                sized_cell(column.width)
                    .pr(m.space[4])
                    .whitespace_nowrap()
                    .child(column.title.clone())
            }))
            .child(div().flex_none().w(m.space[9]));

        let columns = self.columns;
        let rows = self.rows.into_iter().map(|mut row| {
            let group = SharedString::from(format!("table-row-{:?}", row.id));
            let selected = row.selected;
            let mut cells: Vec<Option<TableCell>> = row.cells.drain(..).map(Some).collect();
            let action = row.action.take();
            row_padding(div())
                .id(row.id.clone())
                .group(group.clone())
                .flex()
                .flex_none()
                .items_center()
                .h(row_height)
                .rounded(m.radius_control)
                .type_style(theme.typography.body)
                .text_color(c.text)
                .cursor_pointer()
                .map(|this| {
                    if selected {
                        this.bg(c.selected)
                    } else {
                        this.hover(move |style| style.bg(hover_bg))
                    }
                })
                .children(visible.iter().map(|&index| {
                    let cell = cells.get_mut(index).and_then(Option::take);
                    sized_cell(columns[index].width)
                        .pr(m.space[4])
                        .whitespace_nowrap()
                        .children(cell.map(|cell| render_cell(cell, selected, &theme)))
                }))
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .justify_end()
                        .w(m.space[9])
                        .when(!selected, |this| {
                            this.invisible().group_hover(group, |style| style.visible())
                        })
                        .children(action),
                )
                .when_some(row.on_click, |this, handler| {
                    this.on_click(move |event, window, cx| handler(event, window, cx))
                })
        });

        div()
            .id(self.id)
            .flex()
            .flex_col()
            .w_full()
            .font_family(theme.typography.ui_family.clone())
            .child(header)
            .children(rows)
    }
}

fn render_cell(cell: TableCell, selected: bool, theme: &SlateTheme) -> AnyElement {
    let c = &theme.colors;
    let text = |content: SharedString| {
        div()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .child(content)
    };
    match cell {
        TableCell::Primary(content) => text(content)
            .when(selected, |this| this.font_weight(FontWeight::SEMIBOLD))
            .into_any_element(),
        TableCell::Muted(content) => text(content).text_color(c.text_muted).into_any_element(),
        TableCell::Mono(content) => text(content)
            .text_color(c.text_secondary)
            .font_family(theme.typography.mono_family.clone())
            .type_style(theme.typography.body_small)
            .into_any_element(),
        TableCell::Status(kind, label) => StatusGlyph::new(kind).label(label).into_any_element(),
        TableCell::Element(element) => element,
    }
}

/// A caption row that starts a group of rows.
#[derive(IntoElement)]
pub struct SectionHeader {
    label: SharedString,
}

impl SectionHeader {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
        }
    }
}

impl RenderOnce for SectionHeader {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.slate();
        let m = &theme.metrics;
        div()
            .pt(m.space_compact + m.space[2])
            .pb(m.space_dense)
            .pl(m.space[5])
            .text_color(theme.colors.text_muted)
            .type_style(theme.typography.caption.weight(FontWeight::SEMIBOLD))
            .child(self.label)
    }
}

/// The right-hand pane for the selected object: read first, edited second.
#[derive(IntoElement)]
pub struct Inspector {
    title: SharedString,
    status: Option<StatusKind>,
    on_close: Option<ClickHandler>,
    properties: Vec<(SharedString, SharedString, bool)>,
    actions: Vec<AnyElement>,
    children: Vec<AnyElement>,
}

impl Inspector {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            status: None,
            on_close: None,
            properties: Vec::new(),
            actions: Vec::new(),
            children: Vec::new(),
        }
    }

    pub fn status(mut self, status: StatusKind) -> Self {
        self.status = Some(status);
        self
    }

    pub fn on_close(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_close = Some(Rc::new(handler));
        self
    }

    /// A label and value pair.
    pub fn property(
        mut self,
        label: impl Into<SharedString>,
        value: impl Into<SharedString>,
    ) -> Self {
        self.properties.push((label.into(), value.into(), false));
        self
    }

    /// A label and value pair whose value is technical, drawn in mono.
    pub fn mono_property(
        mut self,
        label: impl Into<SharedString>,
        value: impl Into<SharedString>,
    ) -> Self {
        self.properties.push((label.into(), value.into(), true));
        self
    }

    /// Adds an action button. Make the first action strong.
    pub fn action(mut self, action: impl IntoElement) -> Self {
        self.actions.push(action.into_any_element());
        self
    }
}

impl ParentElement for Inspector {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

/// Width of the label column in inspector property rows.
const PROPERTY_LABEL_WIDTH: f32 = 92.;

impl RenderOnce for Inspector {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.slate();
        let c = &theme.colors;
        let m = &theme.metrics;
        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(m.inspector_width)
            .h_full()
            .bg(c.surface)
            .border_l(m.hairline)
            .border_color(c.border)
            .font_family(theme.typography.ui_family.clone())
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(m.space[3])
                    .h(m.toolbar_height)
                    .pl(m.space[5])
                    .pr(m.space[3])
                    .border_b(m.hairline)
                    .border_color(c.border_subtle)
                    .when_some(self.status, |this, status| {
                        this.child(StatusGlyph::new(status))
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_color(c.text)
                            .type_style(theme.typography.heading_small)
                            .child(self.title),
                    )
                    .when_some(self.on_close, |this, on_close| {
                        this.child(
                            IconButton::new("inspector-close", IconName::Close, "Close inspector")
                                .on_click(move |event, window, cx| on_close(event, window, cx)),
                        )
                    }),
            )
            .child(
                div()
                    .id("inspector-body")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .py(m.space[3])
                    .children(self.properties.into_iter().map(|(label, value, mono)| {
                        div()
                            .flex()
                            .items_center()
                            .gap(m.space_compact)
                            .py(m.space_dense)
                            .px(m.space[5])
                            .type_style(theme.typography.body)
                            .child(
                                div()
                                    .flex_none()
                                    .w(gpui::px(PROPERTY_LABEL_WIDTH))
                                    .text_color(c.text_faint)
                                    .child(label),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .text_color(c.text)
                                    .when(mono, |this| {
                                        this.font_family(theme.typography.mono_family.clone())
                                            .type_style(theme.typography.body_small)
                                    })
                                    .child(value),
                            )
                    }))
                    .when(!self.actions.is_empty(), |this| {
                        this.child(
                            div()
                                .flex()
                                .flex_wrap()
                                .gap(m.space[3])
                                .py(m.space[4])
                                .px(m.space[5])
                                .children(self.actions),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(m.space[3])
                            .px(m.space[5])
                            .children(self.children),
                    ),
            )
    }
}

/// The tone of a callout or banner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Tone {
    Attention,
    Error,
}

/// A problem inside a pane, and what to do about it.
#[derive(IntoElement)]
pub struct Callout {
    tone: Tone,
    title: Option<SharedString>,
    body: SharedString,
    action: Option<AnyElement>,
}

impl Callout {
    pub fn new(tone: Tone, body: impl Into<SharedString>) -> Self {
        Self {
            tone,
            title: None,
            body: body.into(),
            action: None,
        }
    }

    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn action(mut self, action: impl IntoElement) -> Self {
        self.action = Some(action.into_any_element());
        self
    }
}

impl RenderOnce for Callout {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.slate();
        let m = &theme.metrics;
        let color = match self.tone {
            Tone::Attention => theme.colors.status_attention,
            Tone::Error => theme.colors.status_error,
        };
        div()
            .flex()
            .flex_col()
            .gap(m.space[2])
            .w_full()
            .py(m.space_compact)
            .px(m.space[4])
            .rounded(m.radius_control)
            .bg(color.opacity(0.08))
            .border(m.hairline)
            .border_color(color.opacity(0.28))
            .text_color(color)
            .type_style(theme.typography.body_relaxed)
            .when_some(self.title, |this, title| {
                this.child(div().font_weight(FontWeight::SEMIBOLD).child(title))
            })
            .child(self.body)
            .when_some(self.action, |this, action| {
                this.child(div().pt(m.space[2]).child(action))
            })
    }
}

/// What will appear here, and the one action that fills it.
#[derive(IntoElement)]
pub struct EmptyState {
    icon: Option<IconName>,
    title: SharedString,
    body: Option<SharedString>,
    action: Option<AnyElement>,
}

impl EmptyState {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            icon: None,
            title: title.into(),
            body: None,
            action: None,
        }
    }

    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn body(mut self, body: impl Into<SharedString>) -> Self {
        self.body = Some(body.into());
        self
    }

    /// One default button.
    pub fn action(mut self, action: impl IntoElement) -> Self {
        self.action = Some(action.into_any_element());
        self
    }
}

/// Body copy stops at this width so lines stay readable.
const EMPTY_BODY_MAX_WIDTH: f32 = 420.;

impl RenderOnce for EmptyState {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.slate();
        let c = &theme.colors;
        let m = &theme.metrics;
        div()
            .flex()
            .flex_col()
            .items_center()
            .w_full()
            .pt(m.space[8] + m.space[3])
            .gap(m.space[3])
            .font_family(theme.typography.ui_family.clone())
            .when_some(self.icon, |this, icon| {
                this.child(Icon::new(icon).size(IconSize::Large).color(c.text_faint))
            })
            .child(
                div()
                    .text_color(c.text)
                    .type_style(theme.typography.heading)
                    .child(self.title),
            )
            .when_some(self.body, |this, body| {
                this.child(
                    div()
                        .max_w(gpui::px(EMPTY_BODY_MAX_WIDTH))
                        .text_center()
                        .text_color(c.text_muted)
                        .type_style(theme.typography.body_relaxed)
                        .child(body),
                )
            })
            .when_some(self.action, |this, action| {
                this.child(div().pt(m.space[2]).child(action))
            })
    }
}
