//! Slate gallery: every component and state, in a working TermiRust shell.
//!
//! ```text
//! cargo run -p termirust-slate --example gallery
//! ```
//!
//! Switch themes from the toolbar. For screenshots, start in a given state
//! with environment variables:
//!
//! - `SLATE_THEME` = `system` | `dark` | `light` | `hc` | `rec`
//! - `SLATE_TAB` = `0` (Home) through `3`
//! - `SLATE_OVERLAY` = `palette` | `dialog` | `menu`
//! - `SLATE_SCROLL` = pixels to scroll the Home page down

use gpui::{
    AnyElement, App, AppContext as _, Bounds, Context, DismissEvent, Entity, FocusHandle,
    Focusable, InteractiveElement as _, IntoElement, KeyBinding, MouseButton, MouseDownEvent,
    ParentElement as _, Pixels, Point, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement as _, Styled as _, TitlebarOptions, Window, WindowBounds,
    WindowOptions, actions, div, point, prelude::FluentBuilder as _, px, size,
};
use gpui_base::input::{InputEvent, InputState};
use gpui_platform::application;
use termirust_slate::{
    ALL_STATUSES, ActiveTheme as _, Banner, Breadcrumb, Button, ButtonWeight, Callout, ColumnWidth,
    CommandPalette, ContextMenu, ControlSize, Dialog, DialogKind, EmptyState, FilterTabs,
    FingerprintBlock, HostCard, HostGrid, HostGroup, IconButton, IconName, InputVariant, Inspector,
    KeyCap, KeyPlatform, Menu, MenuItem, MenuWidth, NavItem, PaletteItem, PaneDrag, PaneHeader,
    PopoverMenu, SearchTrigger, SectionHeader, Segment, Segmented, Sidebar, SidebarHeading,
    SlateAssets, SlateStyled as _, SlateTheme, SplitNode, SplitPanes, StatusBar, StatusBarItem,
    StatusGlyph, StatusKind, Table, TableCell, TableColumn, TableRow, TerminalPane, TextField,
    ThemeChoice, TitleBar, TitleBarTab, Toaster, Toggle, Tone, Toolbar, ToolbarButton, Tooltip,
    WorkspaceHeader, show_toast,
};

actions!(gallery, [OpenPalette, CloseOverlay]);

struct Host {
    name: &'static str,
    subtitle: &'static str,
    address: &'static str,
    user: &'static str,
    group: HostGroup,
    status: StatusKind,
    status_label: &'static str,
    action: &'static str,
    last: &'static str,
}

const HOSTS: &[Host] = &[
    Host {
        name: "prod-api-1",
        subtitle: "deploy · Production",
        address: "deploy@10.0.4.21:22",
        user: "deploy",
        group: HostGroup::Production,
        status: StatusKind::Done,
        status_label: "Live",
        action: "Open",
        last: "Now",
    },
    Host {
        name: "prod-api-2",
        subtitle: "deploy · Production",
        address: "deploy@10.0.4.22:22",
        user: "deploy",
        group: HostGroup::Production,
        status: StatusKind::Idle,
        status_label: "Idle",
        action: "Connect",
        last: "3h ago",
    },
    Host {
        name: "db-primary",
        subtitle: "ops · Production · via bastion",
        address: "ops@10.0.8.5:22",
        user: "ops",
        group: HostGroup::Production,
        status: StatusKind::Attention,
        status_label: "Host key changed",
        action: "Review",
        last: "Yesterday",
    },
    Host {
        name: "staging-web",
        subtitle: "deploy · Staging",
        address: "deploy@10.1.2.14:22",
        user: "deploy",
        group: HostGroup::Staging,
        status: StatusKind::Busy,
        status_label: "Connecting",
        action: "Open",
        last: "Just now",
    },
    Host {
        name: "gpu-lab-01",
        subtitle: "ops · Lab · port 2222",
        address: "ops@gpu-01.lab:2222",
        user: "ops",
        group: HostGroup::Lab,
        status: StatusKind::Offline,
        status_label: "Offline",
        action: "Connect",
        last: "2 days ago",
    },
    Host {
        name: "Local shell",
        subtitle: "zsh · starts in your home folder",
        address: "~",
        user: "you",
        group: HostGroup::Local,
        status: StatusKind::Idle,
        status_label: "Ready",
        action: "Open",
        last: "—",
    },
];

const TABS: [(&str, StatusKind); 3] = [
    ("prod-api-1", StatusKind::Done),
    ("staging-web", StatusKind::Busy),
    ("gpu-lab-01", StatusKind::Error),
];

const NAV: [(IconName, &str, Option<&str>); 7] = [
    (IconName::Home, "Home", None),
    (IconName::Activity, "Activity", None),
    (IconName::Projects, "Projects", None),
    (IconName::Sessions, "Sessions", Some("3")),
    (IconName::Sftp, "SFTP", None),
    (IconName::Devices, "Devices", None),
    (IconName::Settings, "Settings", None),
];

const TOOLS: [(IconName, &str); 5] = [
    (IconName::Presets, "Presets"),
    (IconName::Vault, "Vaults"),
    (IconName::Key, "Keys"),
    (IconName::Snippets, "Snippets"),
    (IconName::KnownHosts, "Known hosts"),
];

#[derive(Clone, Copy, PartialEq)]
enum Overlay {
    None,
    Palette,
    Dialog,
}

struct Gallery {
    focus: FocusHandle,
    active_tab: usize,
    nav: usize,
    selected_host: Option<usize>,
    font_size: usize,
    view_mode: usize,
    copy_on_select: bool,
    keep_alive: bool,
    filter: usize,
    broadcast: bool,
    tree: SplitNode<u32>,
    focused_pane: u32,
    next_pane: u32,
    overlay: Overlay,
    palette_highlight: usize,
    context_menu: Option<Point<Pixels>>,
    quick_connect: Entity<InputState>,
    search: Entity<InputState>,
    name_field: Entity<InputState>,
    address_field: Entity<InputState>,
    error_field: Entity<InputState>,
    port_field: Entity<InputState>,
    palette_query: Entity<InputState>,
    toaster: Entity<Toaster>,
    tab_scroll: ScrollHandle,
    home_scroll: ScrollHandle,
}

fn input(
    window: &mut Window,
    cx: &mut Context<Gallery>,
    placeholder: &'static str,
    value: &'static str,
) -> Entity<InputState> {
    cx.new(|cx| {
        InputState::new(window, cx)
            .placeholder(placeholder)
            .default_value(value)
    })
}

impl Gallery {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let palette_query = input(window, cx, "Search hosts or run a command", "prod");
        cx.subscribe(
            &palette_query,
            |this: &mut Self, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.palette_highlight = 0;
                    cx.notify();
                }
            },
        )
        .detach();
        let mut tree = SplitNode::leaf(1);
        tree.split(&1, 2, termirust_slate::DropEdge::Right, 4);
        tree.split(&2, 3, termirust_slate::DropEdge::Bottom, 4);
        let overlay = match std::env::var("SLATE_OVERLAY").as_deref() {
            Ok("palette") => Overlay::Palette,
            Ok("dialog") => Overlay::Dialog,
            _ => Overlay::None,
        };
        let context_menu = (std::env::var("SLATE_OVERLAY").as_deref() == Ok("menu"))
            .then(|| point(px(260.), px(30.)));
        Self {
            focus: cx.focus_handle(),
            active_tab: std::env::var("SLATE_TAB")
                .ok()
                .and_then(|tab| tab.parse().ok())
                .unwrap_or(0)
                .min(TABS.len()),
            nav: 0,
            selected_host: Some(2),
            font_size: 1,
            view_mode: 0,
            copy_on_select: true,
            keep_alive: false,
            filter: 0,
            broadcast: true,
            tree,
            focused_pane: 1,
            next_pane: 4,
            overlay,
            palette_highlight: 0,
            context_menu,
            quick_connect: input(window, cx, "user@host or ssh user@host:port", ""),
            search: input(window, cx, "Filter hosts", ""),
            name_field: input(window, cx, "Name", "web-3"),
            address_field: input(window, cx, "user@host:port", "deploy@10.0.4.23:22"),
            error_field: input(window, cx, "user@host:port", "deploy@"),
            port_field: input(window, cx, "22", "22"),
            palette_query,
            toaster: cx.new(|_| Toaster::new()),
            tab_scroll: ScrollHandle::new(),
            home_scroll: {
                let handle = ScrollHandle::new();
                if let Some(offset) = std::env::var("SLATE_SCROLL")
                    .ok()
                    .and_then(|v| v.parse::<f32>().ok())
                {
                    handle.set_offset(point(px(0.), px(-offset)));
                }
                handle
            },
        }
    }

    fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.overlay == Overlay::Dialog {
            return;
        }
        self.overlay = Overlay::Palette;
        self.palette_highlight = 0;
        self.palette_query
            .update(cx, |query, cx| query.focus(window, cx));
        cx.notify();
    }

    fn close_overlay(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.overlay = Overlay::None;
        self.context_menu = None;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn set_theme(&mut self, choice: ThemeChoice, window: &mut Window, cx: &mut Context<Self>) {
        SlateTheme::install(choice, Some(window), cx);
        cx.notify();
    }

    fn split_focused(&mut self, edge: termirust_slate::DropEdge, cx: &mut Context<Self>) {
        let pane = self.next_pane;
        if self.tree.split(&self.focused_pane, pane, edge, 4) {
            self.next_pane += 1;
            self.focused_pane = pane;
            show_toast(&self.toaster, "Opened another session to prod-api-1.", cx);
        } else {
            show_toast(
                &self.toaster,
                "Split is off because this tab already has four panes.",
                cx,
            );
        }
        cx.notify();
    }

    fn palette_items(&self, cx: &App) -> Vec<PaletteItem> {
        let query = self.palette_query.read(cx).value().to_lowercase();
        let mut items: Vec<PaletteItem> = HOSTS
            .iter()
            .filter(|host| host.group != HostGroup::Local)
            .map(|host| {
                let verb = if host.status == StatusKind::Done {
                    "Open"
                } else {
                    "Connect to"
                };
                PaletteItem::new("Host", format!("{verb} {}", host.name)).hint(host.address)
            })
            .collect();
        items.push(
            PaletteItem::new("Snippet", "Service status").hint("systemctl status orders-api"),
        );
        items.push(PaletteItem::new("Snippet", "Tail access log").hint("tail -f access.log"));
        items.push(PaletteItem::new("Go to", "Projects"));
        items.push(PaletteItem::new("Go to", "Known hosts"));
        items.push(PaletteItem::new("Task", "New local terminal").hint("⌘T"));
        items
            .into_iter()
            .filter(|item| item.matches(&query))
            .collect()
    }
}

impl Focusable for Gallery {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Gallery {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.slate();
        let c = &theme.colors;
        let entity = cx.entity();

        let body = if self.active_tab == 0 {
            self.render_home(window, cx).into_any_element()
        } else {
            self.render_terminal_tab(entity.clone(), cx)
                .into_any_element()
        };

        div()
            .id("gallery")
            .key_context("Gallery")
            .track_focus(&self.focus)
            .on_action(
                cx.listener(|this, _: &OpenPalette, window, cx| this.open_palette(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &CloseOverlay, window, cx| this.close_overlay(window, cx)),
            )
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(c.canvas)
            .text_color(c.text)
            .font_family(theme.typography.ui_family.clone())
            .type_style(theme.typography.body)
            .child(self.render_title_bar(cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .when(self.active_tab == 0, |this| {
                        this.child(self.render_sidebar(cx))
                    })
                    .child(div().flex().flex_col().flex_1().min_w_0().child(body)),
            )
            .child(self.render_status_bar(cx))
            .child(self.toaster.clone())
            .when(self.overlay == Overlay::Palette, |this| {
                this.child(self.render_palette(cx))
            })
            .when(self.overlay == Overlay::Dialog, |this| {
                this.child(self.render_dialog(cx))
            })
            .when_some(self.context_menu, |this, position| {
                this.child(ContextMenu::new(
                    position,
                    workspace_menu(),
                    cx.listener(|this, _: &DismissEvent, _, cx| {
                        this.context_menu = None;
                        cx.notify();
                    }),
                ))
            })
    }
}

fn workspace_menu() -> Menu {
    Menu::new()
        .width(MenuWidth::Workspace)
        .item(MenuItem::new("Duplicate").keystroke("cmd-d"))
        .item(MenuItem::new("Duplicate in new window"))
        .item(MenuItem::new("Rename"))
        .separator()
        .item(MenuItem::new("Split right").icon(IconName::SplitRight))
        .item(
            MenuItem::new("Split down")
                .icon(IconName::SplitDown)
                .disabled("4 panes max"),
        )
        .separator()
        .item(MenuItem::new("Close tab").keystroke("cmd-w").destructive())
}

impl Gallery {
    fn render_title_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let pane_count = self.tree.pane_count();
        TitleBar::new("title-bar")
            .track_scroll(&self.tab_scroll)
            .on_double_click({
                let toaster = self.toaster.clone();
                move |_, _, cx| show_toast(&toaster, "Opened a local terminal.", cx)
            })
            .tab(
                TitleBarTab::home("tab-home")
                    .active(self.active_tab == 0)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.active_tab = 0;
                        cx.notify();
                    })),
            )
            .tabs(TABS.iter().enumerate().map(|(index, (name, status))| {
                let tab = index + 1;
                TitleBarTab::new(("tab", tab), *name)
                    .status(*status)
                    .pane_count(if tab == 1 { pane_count } else { 1 })
                    .active(self.active_tab == tab)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.active_tab = tab;
                        cx.notify();
                    }))
                    .on_close(cx.listener(move |this, _, _, cx| {
                        show_toast(
                            &this.toaster,
                            "Tabs close in the app; the gallery keeps them.",
                            cx,
                        );
                    }))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|this, event: &MouseDownEvent, _, cx| {
                            this.context_menu = Some(event.position);
                            cx.notify();
                        }),
                    )
            }))
            .trailing(
                div().w(px(280.)).flex().child(
                    SearchTrigger::new("search", "Search hosts or run a command")
                        .keystroke("cmd-k")
                        .on_click(cx.listener(|this, _, window, cx| this.open_palette(window, cx))),
                ),
            )
    }

    fn render_sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        Sidebar::new()
            .header("Personal vault")
            .children(NAV.iter().enumerate().map(|(index, (icon, label, badge))| {
                NavItem::new(("nav", index), *icon, *label)
                    .active(self.nav == index)
                    .when_some(*badge, |item, badge| item.badge(badge))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.nav = index;
                        cx.notify();
                    }))
            }))
            .child(SidebarHeading::new("Tools"))
            .children(TOOLS.iter().enumerate().map(|(index, (icon, label))| {
                let nav = NAV.len() + index;
                NavItem::new(("tool", index), *icon, *label)
                    .tool(true)
                    .active(self.nav == nav)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.nav = nav;
                        cx.notify();
                    }))
            }))
            .footer(NavItem::new("logs", IconName::Logs, "Logs").tool(true))
    }

    fn render_status_bar(&mut self, _cx: &mut Context<Self>) -> impl IntoElement {
        StatusBar::new()
            .start(StatusBarItem::new("3 live").status(StatusKind::Done))
            .start(StatusBarItem::new("1 needs attention").status(StatusKind::Attention))
            .end(StatusBarItem::new("Vault synced"))
            .end(StatusBarItem::new(if self.keep_alive {
                "Keep-alive 30s"
            } else {
                "Keep-alive off"
            }))
    }

    fn render_home(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.slate();
        let c = &theme.colors;
        let m = &theme.metrics;
        let inspector_open = self.selected_host.is_some();
        let choices = ThemeChoice::ALL;
        let current = choices
            .iter()
            .position(|choice| *choice == theme.choice)
            .unwrap_or(0);

        let toolbar = Toolbar::new()
            .child(
                Breadcrumb::new("crumbs")
                    .link("Home", |_, _, _| {})
                    .item("All hosts"),
            )
            .child(
                FilterTabs::new("filters")
                    .tabs(["All", "Production", "Staging", "Lab"])
                    .selected(self.filter)
                    .on_change(cx.listener(|this, index, _, cx| {
                        this.filter = *index;
                        cx.notify();
                    })),
            )
            .trailing(
                Segmented::new("theme")
                    .segments(choices.iter().map(|choice| Segment::text(choice.label())))
                    .selected(current)
                    .on_change(cx.listener(move |this, index, window, cx| {
                        this.set_theme(choices[*index], window, cx);
                    })),
            )
            .trailing(
                PopoverMenu::new(
                    "more-menu",
                    IconButton::new("more", IconName::More, "More actions"),
                    |_, _| workspace_menu(),
                )
                .align_end(),
            );

        let content = div()
            .id("gallery-scroll")
            .track_scroll(&self.home_scroll)
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .overflow_y_scroll()
            .px(m.space[6])
            .py(m.space[5])
            .gap(m.space[7])
            .child(self.quick_connect_section(cx))
            .child(self.hosts_section(inspector_open, cx))
            .child(self.buttons_section(cx))
            .child(self.inputs_section(window, cx))
            .child(self.selection_section(cx))
            .child(self.status_section(cx))
            .child(self.table_section(inspector_open, cx))
            .child(self.feedback_section(cx))
            .child(self.overlays_section(cx))
            .child(self.tabs_section(cx))
            .child(div().h(m.space[6]));

        div()
            .flex()
            .flex_col()
            .size_full()
            .child(toolbar)
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(content)
                    .when_some(self.selected_host, |this, index| {
                        this.child(self.render_inspector(index, cx))
                    }),
            )
            .child(div().hidden().bg(c.canvas))
    }

    fn render_inspector(&mut self, index: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let host = &HOSTS[index];
        Inspector::new(host.name)
            .status(host.status)
            .on_close(cx.listener(|this, _, _, cx| {
                this.selected_host = None;
                cx.notify();
            }))
            .mono_property("Address", host.address)
            .property("User", host.user)
            .property("Group", host.group.label())
            .property("Last active", host.last)
            .when(host.subtitle.contains("bastion"), |this| this.property("Jump host", "bastion"))
            .action(
                Button::new("inspect-primary")
                    .label(host.action)
                    .strong()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.overlay = Overlay::Dialog;
                        cx.notify();
                    })),
            )
            .action(Button::new("inspect-edit").label("Edit"))
            .when(host.status == StatusKind::Attention, |this| {
                this.child(
                    Callout::new(
                        Tone::Attention,
                        "The key this host presented does not match the pinned key. Review it before connecting.",
                    )
                    .title("Host key changed"),
                )
            })
    }

    fn quick_connect_section(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        section(
            "Quick connect",
            "Type user@host or ssh user@host:port. Enter connects without saving.",
            cx,
        )
        .child(
            div().max_w(px(640.)).child(
                TextField::new("quick-connect", &self.quick_connect)
                    .variant(InputVariant::QuickConnect)
                    .icon(IconName::Terminal)
                    .trailing(
                        Button::new("quick-connect-go")
                            .label("Connect")
                            .size(ControlSize::Small)
                            .on_click(cx.listener(|this, _, _, cx| {
                                show_toast(&this.toaster, "Connected to root@203.0.113.9.", cx);
                            })),
                    ),
            ),
        )
    }

    fn hosts_section(&mut self, inspector_open: bool, cx: &mut Context<Self>) -> impl IntoElement {
        section(
            "Host cards",
            "Rest, hover, selected, live, offline, and the local profile. Hover a card for its action.",
            cx,
        )
        .child(
            HostGrid::new(if inspector_open { 2 } else { 3 }).children(HOSTS.iter().enumerate().map(
                |(index, host)| {
                    HostCard::new(("host", index), host.name, host.group)
                        .subtitle(host.subtitle)
                        .status(host.status, host.status_label)
                        .selected(self.selected_host == Some(index))
                        .action(
                            host.action,
                            cx.listener(move |this, _, _, cx| {
                                let host = &HOSTS[index];
                                show_toast(&this.toaster, format!("{} {}.", host.action, host.name), cx);
                            }),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.selected_host = Some(index);
                            cx.notify();
                        }))
                },
            )),
        )
    }

    fn buttons_section(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.slate();
        let m = &theme.metrics;
        section(
            "Buttons",
            "One neutral button in two weights, plus quiet toolbar and icon buttons. Hover and keyboard focus are live.",
            cx,
        )
        .child(
            row(m.space[4])
                .child(Button::new("b-default").label("Open Fleet"))
                .child(Button::new("b-strong").label("New Host").icon(IconName::Plus).strong())
                .child(Button::new("b-small").label("Connect").size(ControlSize::Small))
                .child(Button::new("b-large").label("Connect").size(ControlSize::Large).weight(ButtonWeight::Strong))
                .child(Button::new("b-danger").label("Close 3 sessions").danger(true))
                .child(
                    Button::new("b-pressed")
                        .label("Menu open")
                        .map(|button| gpui_base::Selectable::selected(button, true)),
                )
                .child(
                    Button::new("b-disabled")
                        .label("Add host")
                        .disabled(true)
                        .tooltip("Unlock the vault to add hosts."),
                ),
        )
        .child(
            row(m.space[4])
                .child(ToolbarButton::new("t-split").label("Split right").icon(IconName::SplitRight))
                .child(
                    ToolbarButton::new("t-broadcast")
                        .label("Broadcasting to 2 panes")
                        .icon(IconName::Broadcast)
                        .armed(self.broadcast)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.broadcast = !this.broadcast;
                            cx.notify();
                        })),
                )
                .child(ToolbarButton::new("t-disabled").label("Split down").icon(IconName::SplitDown).disabled(true))
                .child(IconButton::new("i-plain", IconName::Sidebar, "Toggle sidebar"))
                .child(IconButton::new("i-on", IconName::Grid, "Grid view").on(true))
                .child(
                    IconButton::new("i-tip", IconName::Refresh, "Reconnect")
                        .tooltip(Tooltip::new("Reconnect").keystroke("cmd-r")),
                )
                .child(IconButton::new("i-disabled", IconName::Upload, "Upload").disabled(true)),
        )
    }

    fn inputs_section(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.slate();
        let m = &theme.metrics;
        section(
            "Inputs",
            "Four fields share one anatomy. Click a field to see the focus border.",
            cx,
        )
        .child(
            row(m.space[5])
                .items_start()
                .child(
                    div().w(px(240.)).child(
                        TextField::new("filter", &self.search)
                            .icon(IconName::Search)
                            .keystroke("cmd-f"),
                    ),
                )
                .child(
                    div().w(px(280.)).child(
                        SearchTrigger::new("search-demo", "Search hosts or run a command")
                            .keystroke("cmd-k"),
                    ),
                ),
        )
        .child(
            row(m.space[5])
                .items_start()
                .child(
                    div().w(px(200.)).child(
                        TextField::new("f-name", &self.name_field)
                            .variant(InputVariant::Form)
                            .label("Name"),
                    ),
                )
                .child(
                    div().w(px(240.)).child(
                        TextField::new("f-address", &self.address_field)
                            .variant(InputVariant::Form)
                            .label("Address")
                            .hint("user@host or user@host:port"),
                    ),
                )
                .child(
                    div().w(px(260.)).child(
                        TextField::new("f-error", &self.error_field)
                            .variant(InputVariant::Form)
                            .label("Address")
                            .error("Enter the address as user@host or user@host:port."),
                    ),
                )
                .child(
                    div().w(px(100.)).child(
                        TextField::new("f-port", &self.port_field)
                            .variant(InputVariant::Form)
                            .label("Port")
                            .disabled(true),
                    ),
                ),
        )
    }

    fn selection_section(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.slate();
        let m = &theme.metrics;
        let c = &theme.colors;
        let label = |text: &'static str| {
            div()
                .text_color(c.text_muted)
                .type_style(theme.typography.caption)
                .child(text)
        };
        section(
            "Selection controls",
            "Segmented controls pick one option, toggles flip a setting now, filter tabs narrow a list, and key caps show a shortcut.",
            cx,
        )
        .child(
            row(m.space[6])
                .child(
                    Segmented::new("font-size")
                        .segments(["12", "13", "14", "15"].map(Segment::text))
                        .selected(self.font_size)
                        .on_change(cx.listener(|this, index, _, cx| {
                            this.font_size = *index;
                            cx.notify();
                        })),
                )
                .child(
                    Segmented::new("view-mode")
                        .segment(Segment::icon(IconName::Grid, "Grid"))
                        .segment(Segment::icon(IconName::List, "List"))
                        .selected(self.view_mode)
                        .on_change(cx.listener(|this, index, _, cx| {
                            this.view_mode = *index;
                            cx.notify();
                        })),
                )
                .child(Segmented::new("seg-disabled").segments(["On", "Off"].map(Segment::text)).disabled(true))
                .child(
                    row(m.space[3])
                        .child(
                            Toggle::new("copy-on-select", "Copy on select")
                                .checked(self.copy_on_select)
                                .on_change(cx.listener(|this, checked: &bool, _, cx| {
                                    this.copy_on_select = *checked;
                                    cx.notify();
                                })),
                        )
                        .child(label("Copy on select")),
                )
                .child(
                    row(m.space[3])
                        .child(
                            Toggle::new("keep-alive", "Keep-alive")
                                .checked(self.keep_alive)
                                .on_change(cx.listener(|this, checked: &bool, _, cx| {
                                    this.keep_alive = *checked;
                                    cx.notify();
                                })),
                        )
                        .child(label("Keep-alive")),
                )
                .child(Toggle::new("toggle-disabled", "Disabled").checked(true).disabled(true)),
        )
        .child(
            row(m.space[6])
                .child(
                    FilterTabs::new("filters-demo")
                        .tabs(["All", "Production", "Staging", "Lab"])
                        .selected(self.filter)
                        .on_change(cx.listener(|this, index, _, cx| {
                            this.filter = *index;
                            cx.notify();
                        })),
                )
                .child(
                    row(m.space[3])
                        .child(KeyCap::new("cmd-k"))
                        .child(KeyCap::new("cmd-shift-b"))
                        .child(KeyCap::new("alt-cmd-right"))
                        .child(KeyCap::new("cmd-shift-b").platform(KeyPlatform::Other)),
                ),
        )
    }

    fn status_section(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.slate();
        let m = &theme.metrics;
        section(
            "Status",
            "Every status is a fixed shape, a label, and its color, so state never depends on color alone.",
            cx,
        )
        .child(row(m.space[6]).children(ALL_STATUSES.map(|kind| StatusGlyph::new(kind).default_label())))
    }

    fn table_section(&mut self, inspector_open: bool, cx: &mut Context<Self>) -> impl IntoElement {
        section(
            "Tables and lists",
            "No zebra stripes and no row borders. The Group and Last active columns drop out while the inspector is open.",
            cx,
        )
        .child(SectionHeader::new("Remote"))
        .child(
            Table::new("hosts-table")
                .narrow(inspector_open)
                .column(TableColumn::new("Name", ColumnWidth::Flex(1.2)))
                .column(TableColumn::new("Address", ColumnWidth::Flex(1.6)))
                .column(TableColumn::new("Group", ColumnWidth::Flex(1.)).optional())
                .column(TableColumn::new("Status", ColumnWidth::Flex(1.3)))
                .column(TableColumn::new("Last active", ColumnWidth::Flex(1.)).optional())
                .rows(HOSTS.iter().enumerate().take(5).map(|(index, host)| {
                    TableRow::new(("row", index))
                        .selected(self.selected_host == Some(index))
                        .cell(TableCell::primary(host.name))
                        .cell(TableCell::mono(host.address))
                        .cell(TableCell::muted(host.group.label()))
                        .cell(TableCell::status(host.status, host.status_label))
                        .cell(TableCell::muted(host.last))
                        .action(Button::new(("row-action", index)).label(host.action).size(ControlSize::Small))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.selected_host = Some(index);
                            cx.notify();
                        }))
                })),
        )
    }

    fn feedback_section(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.slate();
        let m = &theme.metrics;
        let c = &theme.colors;
        section(
            "Callouts, banners, and empty states",
            "Callouts sit in a pane, banners report a state that is still true, and empty states offer the one action that fills the view.",
            cx,
        )
        .child(
            row(m.space[5])
                .items_start()
                .child(div().flex_1().child(Callout::new(
                    Tone::Attention,
                    "This host did not respond on its last attempt. Connecting will retry.",
                )))
                .child(div().flex_1().child(Callout::new(
                    Tone::Error,
                    "Permission denied for ops. Check the identity in the host editor.",
                ).title("Could not sign in"))),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .rounded(m.radius_panel)
                .overflow_hidden()
                .border(m.hairline)
                .border_color(c.border_subtle)
                .child(
                    Banner::failure("Could not connect to gpu-lab-01. The connection timed out.")
                        .detail("ssh: connect to host gpu-01.lab port 22: Operation timed out")
                        .action(Button::new("reconnect").label("Reconnect").size(ControlSize::Small)),
                )
                .child(Banner::ended("The session to staging-web ended."))
                .child(div().h(m.space[8]).bg(c.terminal)),
        )
        .child(
            div()
                .rounded(m.radius_panel)
                .border(m.hairline)
                .border_color(c.border_subtle)
                .pb(m.space[7])
                .child(
                    EmptyState::new("No open sessions")
                        .icon(IconName::Terminal)
                        .body("Open a host from Home, or double-click the top bar to open a local terminal.")
                        .action(Button::new("empty-action").label("New local terminal").icon(IconName::Plus)),
                ),
        )
    }

    fn overlays_section(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.slate();
        let m = &theme.metrics;
        section(
            "Overlays",
            "Open each overlay. Escape closes it, and ⌘K opens the palette from anywhere. Right-click a terminal tab for its menu.",
            cx,
        )
        .child(
            row(m.space[4])
                .child(
                    Button::new("open-palette")
                        .label("Command palette")
                        .icon(IconName::Search)
                        .on_click(cx.listener(|this, _, window, cx| this.open_palette(window, cx))),
                )
                .child(
                    Button::new("open-dialog")
                        .label("Security dialog")
                        .icon(IconName::KnownHosts)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.overlay = Overlay::Dialog;
                            cx.notify();
                        })),
                )
                .child(PopoverMenu::new(
                    "menu-demo",
                    Button::new("open-menu").label("Pane menu").icon(IconName::More),
                    |_, _| {
                        Menu::new()
                            .item(MenuItem::new("Move to its own tab").icon(IconName::Detach))
                            .item(MenuItem::new("Broadcast input").icon(IconName::Broadcast).keystroke("cmd-shift-b"))
                            .separator()
                            .item(MenuItem::new("Clear").keystroke("cmd-k"))
                            .item(MenuItem::new("Close pane").destructive())
                    },
                ))
                .child(
                    Button::new("toast")
                        .label("Show toast")
                        .on_click(cx.listener(|this, _, _, cx| {
                            show_toast(&this.toaster, "Pinned the host key for ci-runner-1 on first connection.", cx);
                        })),
                ),
        )
    }

    fn tabs_section(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        section(
            "Title bar tabs",
            "Home, an active tab with a pane count, idle tabs, and a tab showing the drop marker while a pane is dragged over it.",
            cx,
        )
        .child(
            div().child(
                TitleBar::new("title-bar-specimen")
                    .traffic_lights(false)
                    .tab(TitleBarTab::home("spec-home"))
                    .tab(
                        TitleBarTab::new("spec-active", "prod-api-1")
                            .status(StatusKind::Done)
                            .pane_count(3)
                            .active(true)
                            .on_close(|_, _, _| {}),
                    )
                    .tab(TitleBarTab::new("spec-idle", "staging-web").status(StatusKind::Busy).on_close(|_, _, _| {}))
                    .tab(
                        TitleBarTab::new("spec-drop", "gpu-lab-01")
                            .status(StatusKind::Offline)
                            .drop_target(true)
                            .on_close(|_, _, _| {}),
                    ),
            ),
        )
    }

    fn render_palette(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let items = self.palette_items(cx);
        let count = items.len();
        CommandPalette::new(gpui_base::Input::new(&self.palette_query))
            .items(items)
            .highlighted(self.palette_highlight)
            .on_highlight(cx.listener(|this, index, _, cx| {
                this.palette_highlight = *index;
                cx.notify();
            }))
            .on_select(cx.listener(move |this, index, window, cx| {
                let message = format!("Ran palette result {} of {count}.", *index + 1);
                this.close_overlay(window, cx);
                show_toast(&this.toaster, message, cx);
            }))
            .on_dismiss(
                cx.listener(|this, _: &DismissEvent, window, cx| this.close_overlay(window, cx)),
            )
    }

    fn render_dialog(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        Dialog::new("Host key changed for db-primary")
            .kind(DialogKind::Security)
            .description(
                "The key presented by 10.0.8.5 does not match the key you pinned on August 14. If nobody rotated this key, someone may be intercepting the connection.",
            )
            .body(
                FingerprintBlock::new()
                    .row("Pinned", "SHA256:b8Uo…T4kd")
                    .row("Presented", "SHA256:9fQe…Xa2L"),
            )
            .confirm(
                "Trust new key and connect",
                false,
                cx.listener(|this, _, window, cx| {
                    this.close_overlay(window, cx);
                    show_toast(&this.toaster, "Pinned the new key for db-primary.", cx);
                }),
            )
            .on_cancel(cx.listener(|this, _: &DismissEvent, window, cx| this.close_overlay(window, cx)))
    }

    fn render_terminal_tab(
        &mut self,
        entity: Entity<Self>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let (name, status) = TABS[self.active_tab - 1];
        let host = HOSTS
            .iter()
            .find(|host| host.name == name)
            .unwrap_or(&HOSTS[0]);
        let header = WorkspaceHeader::new(name)
            .detail(format!("{} · 2h 14m", host.address))
            .tool(
                ToolbarButton::new("ws-broadcast")
                    .label(if self.broadcast {
                        "Broadcasting"
                    } else {
                        "Broadcast"
                    })
                    .icon(IconName::Broadcast)
                    .armed(self.broadcast)
                    .tooltip(Tooltip::new("Broadcast input").keystroke("cmd-shift-b"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.broadcast = !this.broadcast;
                        cx.notify();
                    })),
            )
            .tool(
                ToolbarButton::new("ws-split-right")
                    .label("Split right")
                    .icon(IconName::SplitRight)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.split_focused(termirust_slate::DropEdge::Right, cx)
                    })),
            )
            .tool(
                ToolbarButton::new("ws-split-down")
                    .label("Split down")
                    .icon(IconName::SplitDown)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.split_focused(termirust_slate::DropEdge::Bottom, cx)
                    })),
            );

        let body = if self.active_tab == 1 {
            let focused = self.focused_pane;
            let count = self.tree.pane_count();
            let pane_entity = entity.clone();
            let resize_entity = entity.clone();
            let drop_entity = entity;
            SplitPanes::new("panes", &self.tree, move |pane, _, _| {
                terminal_pane(*pane, focused, count, status, pane_entity.clone())
            })
            .on_resize(move |resize, _, cx| {
                resize_entity.update(cx, |this, cx| {
                    this.tree.set_ratio(&resize.path, resize.ratio);
                    cx.notify();
                });
            })
            .on_drop(move |drop, _, cx| {
                drop_entity.update(cx, |this, cx| {
                    if this.tree.move_pane(&drop.pane, &drop.target, drop.edge) {
                        this.focused_pane = drop.pane;
                    }
                    cx.notify();
                });
            })
            .into_any_element()
        } else if status == StatusKind::Error {
            TerminalPane::new()
                .banner(
                    Banner::failure("Could not connect to gpu-lab-01. The connection timed out.")
                        .detail("ssh: connect to host gpu-01.lab port 22: Operation timed out")
                        .action(
                            Button::new("tab-reconnect")
                                .label("Reconnect")
                                .size(ControlSize::Small),
                        ),
                )
                .into_any_element()
        } else {
            TerminalPane::new()
                .child(terminal_line(
                    "deploy",
                    "staging-web",
                    "~",
                    "ssh deploy@10.1.2.14",
                ))
                .child(output("Connecting to 10.1.2.14 on port 22…"))
                .into_any_element()
        };

        div()
            .flex()
            .flex_col()
            .size_full()
            .child(header)
            .child(div().flex().flex_1().min_h_0().child(body))
    }
}

fn terminal_pane(
    pane: u32,
    focused: u32,
    count: usize,
    status: StatusKind,
    entity: Entity<Gallery>,
) -> AnyElement {
    let label: SharedString = format!("prod-api-{pane}").into();
    let is_focused = pane == focused;
    let focus_entity = entity.clone();
    let close_entity = entity;
    TerminalPane::new()
        .focused(is_focused)
        .when(count > 1, |this| {
            this.header(
                PaneHeader::new(("pane-header", pane as usize), label.clone())
                    .status(status)
                    .duration(if pane == 1 { "2h 14m" } else { "4m" })
                    .focused(is_focused)
                    .on_drag(PaneDrag::new(pane, label.clone()), PaneDrag::preview)
                    .on_detach(|_, _, _| {})
                    .on_close(move |_, _, cx| {
                        close_entity.update(cx, |this, cx| {
                            if this.tree.remove(&pane) {
                                this.focused_pane = *this.tree.panes()[0];
                            }
                            cx.notify();
                        });
                    }),
            )
        })
        .child(
            div()
                .id(("pane-body", pane as usize))
                .flex()
                .flex_col()
                .size_full()
                .on_click(move |_, _, cx| {
                    focus_entity.update(cx, |this, cx| {
                        this.focused_pane = pane;
                        cx.notify();
                    });
                })
                .child(terminal_line(
                    "deploy",
                    &label,
                    "~",
                    "systemctl status orders-api --no-pager",
                ))
                .child(output("● orders-api.service - Orders API"))
                .child(output(
                    "     Active: active (running) since Tue 2026-09-08 09:14:02 UTC; 2 days ago",
                ))
                .child(terminal_line(
                    "deploy",
                    &label,
                    "~",
                    "tail -n 2 /var/log/orders-api/access.log",
                ))
                .child(output("10.0.2.31 POST /v1/checkout 201 42ms"))
                .child(output("10.0.2.31 GET /v1/cart/5521 404 6ms"))
                .child(terminal_line("deploy", &label, "~", "").cursor(is_focused)),
        )
        .into_any_element()
}

#[derive(IntoElement)]
struct PromptLine {
    user: SharedString,
    host: SharedString,
    path: SharedString,
    command: SharedString,
    /// `Some(focused)` draws the cursor after the command.
    cursor: Option<bool>,
}

fn terminal_line(user: &str, host: &str, path: &str, command: &str) -> PromptLine {
    PromptLine {
        user: user.to_string().into(),
        host: host.to_string().into(),
        path: path.to_string().into(),
        command: command.to_string().into(),
        cursor: None,
    }
}

impl PromptLine {
    fn cursor(mut self, focused: bool) -> Self {
        self.cursor = Some(focused);
        self
    }
}

impl gpui::RenderOnce for PromptLine {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.slate();
        let ansi = theme.colors.ansi;
        // A block cursor is 0.6 em wide and one line tall. Unfocused panes
        // show it hollow.
        let cursor = self.cursor.map(|focused| {
            let block = div()
                .w(theme.typography.terminal.size * 0.6)
                .h(theme.typography.terminal.line_height);
            if focused {
                block.bg(theme.colors.terminal_cursor)
            } else {
                block
                    .border(theme.metrics.hairline)
                    .border_color(theme.colors.terminal_cursor)
            }
        });
        div()
            .flex()
            .whitespace_nowrap()
            .child(
                div()
                    .text_color(ansi[2])
                    .child(format!("{}@{}", self.user, self.host)),
            )
            .child(":")
            .child(div().text_color(ansi[4]).child(self.path))
            .child("$\u{a0}")
            .child(self.command)
            .children(cursor)
    }
}

fn output(text: &'static str) -> impl IntoElement {
    div().whitespace_nowrap().overflow_hidden().child(text)
}

fn section(title: &'static str, description: &'static str, cx: &App) -> gpui::Div {
    let theme = cx.slate();
    let c = &theme.colors;
    let m = &theme.metrics;
    div().flex().flex_col().gap(m.space[4]).child(
        div()
            .flex()
            .flex_col()
            .gap(m.space[1])
            .child(
                div()
                    .text_color(c.text)
                    .type_style(theme.typography.heading)
                    .child(title),
            )
            .child(
                div()
                    .max_w(px(640.))
                    .text_color(c.text_muted)
                    .type_style(theme.typography.body_relaxed)
                    .child(description),
            ),
    )
}

fn row(gap: Pixels) -> gpui::Div {
    div().flex().flex_wrap().items_center().gap(gap)
}

fn theme_from_env() -> ThemeChoice {
    match std::env::var("SLATE_THEME").as_deref() {
        Ok("light") => ThemeChoice::Light,
        Ok("hc") | Ok("high-contrast") => ThemeChoice::HighContrast,
        Ok("rec") | Ok("recording") => ThemeChoice::RecordingFriendly,
        Ok("system") => ThemeChoice::System,
        _ => ThemeChoice::Dark,
    }
}

fn main() {
    application()
        .with_assets(SlateAssets::new())
        .run(|cx: &mut App| {
            termirust_slate::init(theme_from_env(), cx);
            cx.bind_keys([
                KeyBinding::new("cmd-k", OpenPalette, None),
                KeyBinding::new("ctrl-k", OpenPalette, None),
            ]);
            let bounds = Bounds::centered(None, size(px(1280.), px(840.)), cx);
            let window = cx
                .open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(bounds)),
                        titlebar: Some(TitlebarOptions {
                            title: Some("Slate gallery".into()),
                            appears_transparent: true,
                            traffic_light_position: Some(point(px(-200.), px(8.))),
                        }),
                        window_min_size: Some(size(px(960.), px(640.))),
                        ..Default::default()
                    },
                    |window, cx| {
                        let view = cx.new(|cx| Gallery::new(window, cx));
                        window.focus(&view.focus_handle(cx), cx);
                        view
                    },
                )
                .expect("the gallery window opens");
            let _ = window;
            cx.activate(true);
        });
}
