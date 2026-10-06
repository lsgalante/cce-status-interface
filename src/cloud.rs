//! Menu machinery: the in-surface menu model — fetched DBusMenu layouts and
//! bar-built menus alike are flattened into pages of plain-data rows that
//! ride a CustomEvent into the module's update loop, where the module's own
//! surface expands to show them.

use crate::CustomEvent;

#[zbus::proxy(
    interface = "com.canonical.dbusmenu",
    default_path = "/StatusNotifierItem/menu"
)]
pub(crate) trait DBusMenu {
    fn get_layout(
        &self,
        parent_id: i32,
        recursion_depth: i32,
        property_names: Vec<String>,
    ) -> zbus::Result<(u32, (i32, std::collections::HashMap<String, zbus::zvariant::OwnedValue>, Vec<zbus::zvariant::OwnedValue>))>;

    fn event(
        &self,
        id: i32,
        event_id: &str,
        data: &zbus::zvariant::Value<'_>,
        timestamp: u32,
    ) -> zbus::Result<()>;

    fn about_to_show(&self, id: i32) -> zbus::Result<bool>;
}

pub(crate) struct MenuItem {
    id: i32,
    label: String,
    enabled: bool,
    is_separator: bool,
    /// `toggle-type`: "checkmark", "radio", or "" for an item that is not
    /// a toggle.
    toggle_type: String,
    toggle_state: i32, // -1 if not toggleable, 0 if unchecked, 1 if checked
    children: Vec<MenuItem>,
}

pub(crate) fn parse_menu_item(
    id: i32,
    mut properties: std::collections::HashMap<String, zbus::zvariant::OwnedValue>,
    children_vals: Vec<zbus::zvariant::OwnedValue>,
) -> Option<MenuItem> {
    let type_: String = properties.remove("type")
        .and_then(|v| {
            let s: Result<String, _> = v.try_into();
            s.ok()
        })
        .unwrap_or_default();
    let is_separator = type_ == "separator";

    let label: String = properties.remove("label")
        .and_then(|v| {
            let s: Result<String, _> = v.try_into();
            s.ok()
        })
        .unwrap_or_default();

    let enabled: bool = properties.remove("enabled")
        .and_then(|v| {
            let b: Result<bool, _> = v.try_into();
            b.ok()
        })
        .unwrap_or(true);

    let toggle_type: String = properties.remove("toggle-type")
        .and_then(|v| {
            let s: Result<String, _> = v.try_into();
            s.ok()
        })
        .unwrap_or_default();

    let toggle_state: i32 = properties.remove("toggle-state")
        .and_then(|v| {
            let i: Result<i32, _> = v.try_into();
            i.ok()
        })
        .unwrap_or(-1);

    let mut children = Vec::new();
    for child_val in children_vals {
        let child_val_inner = zbus::zvariant::Value::from(child_val);
        if let Ok(child) = <(i32, std::collections::HashMap<String, zbus::zvariant::OwnedValue>, Vec<zbus::zvariant::OwnedValue>)>::try_from(child_val_inner) {
            if let Some(parsed) = parse_menu_item(child.0, child.1, child.2) {
                children.push(parsed);
            }
        }
    }

    Some(MenuItem {
        id,
        label,
        enabled,
        is_separator,
        toggle_type,
        toggle_state,
        children,
    })
}

/// One row of an in-surface menu — plain data so a fetched DBusMenu can ride
/// a `CustomEvent` into the module process's update loop.
#[derive(Debug, Clone)]
pub(crate) struct MenuRow {
    pub label: String,
    pub enabled: bool,
    pub separator: bool,
    pub action: MenuRowAction,
}

#[derive(Debug, Clone)]
pub(crate) enum MenuRowAction {
    /// DBusMenu item: send "clicked" to the menu's owner on click.
    Item(i32),
    /// Navigate to a submenu page (in-surface pagination).
    Submenu(usize),
    /// Navigate back to the parent page.
    Back(usize),
    /// Dispatch a bar-internal event (the module context menu's rows).
    Dispatch(CustomEvent),
    /// Run ccectl with these args, detached (window picker rows).
    Ccectl(Vec<String>),
    /// Non-interactive (separators).
    Inert,
}

#[derive(Debug, Clone)]
pub(crate) struct MenuPage {
    pub title: String,
    pub rows: Vec<MenuRow>,
}

/// The toolkit's leading mark (`cce_ui::widget::context_menu::MARK_*`) for a
/// DBusMenu toggle: a checked checkmark item wears the check, a radio item
/// the filled or the outlined circle, and an unchecked checkmark item and a
/// plain item nothing.
pub(crate) fn toggle_mark(toggle_type: &str, toggle_state: i32) -> &'static str {
    use cce_ui::widget::context_menu::{MARK_CHECK, MARK_OFF, MARK_ON};
    match (toggle_type, toggle_state) {
        ("radio", 1) => MARK_ON,
        ("radio", 0) => MARK_OFF,
        (_, 1) => MARK_CHECK,
        _ => "",
    }
}

/// The row that turns a page back to `parent`: the word "Back", drawn after
/// the chevron-left glyph.
pub(crate) fn back_row(parent: usize) -> MenuRow {
    MenuRow {
        label: "Back".to_string(),
        enabled: true,
        separator: false,
        action: MenuRowAction::Back(parent),
    }
}

/// How a menu row is drawn around its text: the glyph at its left (a mark
/// the label leads with, or the back chevron), the label without the mark,
/// and the glyph at its right end (the chevron of a row that leads to a
/// page). The same conventions as cce-ui's context menus, laid out here
/// because the bar draws its menus in its own surface.
pub(crate) fn menu_row_glyphs(row: &MenuRow) -> (Option<&'static str>, &str, Option<&'static str>) {
    let (mark, text) = cce_ui::widget::context_menu::split_mark(&row.label);
    match row.action {
        MenuRowAction::Back(_) => (Some("chevron-left"), text, None),
        MenuRowAction::Submenu(_) => (mark, text, Some("chevron-right")),
        _ => (mark, text, None),
    }
}

/// Fetch a tray icon's DBusMenu and flatten it into in-surface pages: page 0
/// is the root; each enabled submenu becomes its own page (capped at 16)
/// reached by a `Submenu` row and left by a `Back` row. Separators and
/// disabled items are kept as rows for visual fidelity; toggle states become
/// the toolkit's leading marks (`toggle_mark`), which the menu draws as
/// glyphs, as it draws the page and back chevrons — see `menu_row_glyphs`.
pub(crate) async fn fetch_tray_menu_pages(
    conn: &zbus::Connection,
    destination: &str,
    menu_path: &str,
) -> Result<Vec<MenuPage>, Box<dyn std::error::Error + Send + Sync>> {
    let menu_proxy = DBusMenuProxy::builder(conn)
        .destination(destination)?
        .path(menu_path)?
        .build()
        .await?;

    let _ = menu_proxy.about_to_show(0).await;
    let (_, layout) = menu_proxy.get_layout(0, 5, vec![]).await?;
    let root = match parse_menu_item(layout.0, layout.1, layout.2) {
        Some(item) => item,
        None => return Ok(Vec::new()),
    };

    fn build(
        item: &MenuItem,
        page: usize,
        parent: Option<usize>,
        pages: &mut Vec<MenuPage>,
    ) {
        let mut rows = Vec::new();
        if let Some(parent_page) = parent {
            rows.push(back_row(parent_page));
        }
        // Reserve this page's slot before recursing so child pages number
        // depth-first after it.
        pages[page].title = if item.label.is_empty() && page == 0 {
            "Tray Menu".to_string()
        } else {
            item.label.clone()
        };
        for child in &item.children {
            if child.is_separator {
                rows.push(MenuRow {
                    label: String::new(),
                    enabled: false,
                    separator: true,
                    action: MenuRowAction::Inert,
                });
                continue;
            }
            let label = format!("{}{}", toggle_mark(&child.toggle_type, child.toggle_state), child.label);
            if !child.children.is_empty() && child.enabled && pages.len() < 16 {
                let child_page = pages.len();
                pages.push(MenuPage { title: String::new(), rows: Vec::new() });
                rows.push(MenuRow {
                    label,
                    enabled: true,
                    separator: false,
                    action: MenuRowAction::Submenu(child_page),
                });
                build(child, child_page, Some(page), pages);
            } else {
                rows.push(MenuRow {
                    label,
                    enabled: child.enabled,
                    separator: false,
                    action: MenuRowAction::Item(child.id),
                });
            }
        }
        pages[page].rows = rows;
    }

    let mut pages = vec![MenuPage { title: String::new(), rows: Vec::new() }];
    build(&root, 0, None, &mut pages);
    if pages[0].rows.is_empty() {
        return Ok(Vec::new());
    }
    Ok(pages)
}

/// Fire a DBusMenu "clicked" event for an in-surface menu row, detached —
/// the click handler must not block on D-Bus.
pub(crate) fn send_tray_menu_event(destination: String, menu_path: String, id: i32) {
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
            Ok(rt) => rt,
            Err(_) => return,
        };
        rt.block_on(async move {
            let res: Result<(), Box<dyn std::error::Error + Send + Sync>> = async {
                let conn = zbus::Connection::session().await?;
                let proxy = DBusMenuProxy::builder(&conn)
                    .destination(destination.as_str())?
                    .path(menu_path.as_str())?
                    .build()
                    .await?;
                let timestamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as u32;
                let val = zbus::zvariant::Value::from("");
                proxy.event(id, "clicked", &val, timestamp).await?;
                Ok(())
            }
            .await;
            if let Err(e) = res {
                log::warn!("[tray-menu] clicked event failed: {:?}", e);
            }
        });
    });
}
