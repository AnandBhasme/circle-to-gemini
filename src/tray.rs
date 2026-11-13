// System tray icon via tray-icon crate with ksni (KDE StatusNotifierItem)

use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub enum TrayCommand {
    Activate,
    OpenSettings,
    Quit,
}

/// Spawns the tray icon on a background thread and returns a receiver for commands.
/// The tray icon stays alive as long as the thread is running.
pub fn spawn_tray_icon() -> mpsc::UnboundedReceiver<TrayCommand> {
    let (tx, rx) = mpsc::unbounded_channel();

    std::thread::spawn(move || {
        run_tray_with_sender(tx);
    });

    rx
}

pub fn run_tray_with_sender(tx: mpsc::UnboundedSender<TrayCommand>) {
    use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{TrayIconBuilder, TrayIconEvent};

    // --- Build icon (32×32 Gemini-blue circle with sparkle) ---
    let icon = create_icon();

    // --- Build context menu ---
    let menu = Menu::new();
    let capture_item = MenuItem::new("Capture Screen", true, None);
    let settings_item = MenuItem::new("Settings", true, None);
    let quit_item = MenuItem::new("Quit", true, None);

    let capture_id = capture_item.id().clone();
    let settings_id = settings_item.id().clone();
    let quit_id = quit_item.id().clone();

    menu.append(&capture_item).ok();
    menu.append(&PredefinedMenuItem::separator()).ok();
    menu.append(&settings_item).ok();
    menu.append(&PredefinedMenuItem::separator()).ok();
    menu.append(&quit_item).ok();

    // --- Create tray icon (ksni backend talks D-Bus to KDE) ---
    let _tray = TrayIconBuilder::new()
        .with_icon(icon)
        .with_tooltip("Circle to Gemini - Click to capture")
        .with_menu(Box::new(menu))
        .with_title("Circle to Gemini")
        .build()
        .expect("Failed to create system tray icon");

    // --- Event receivers (crossbeam channels from tray-icon) ---
    let menu_rx = MenuEvent::receiver();
    let tray_rx = TrayIconEvent::receiver();

    loop {
        // Poll menu clicks
        if let Ok(event) = menu_rx.try_recv() {
            if event.id == capture_id {
                if tx.send(TrayCommand::Activate).is_err() {
                    break;
                }
            } else if event.id == settings_id {
                if tx.send(TrayCommand::OpenSettings).is_err() {
                    break;
                }
            } else if event.id == quit_id {
                tx.send(TrayCommand::Quit).ok();
                break;
            }
        }

        // Poll tray icon left-click (direct activation)
        if let Ok(event) = tray_rx.try_recv() {
            // On KDE/SNI, left-click fires the Activate signal
            // tray-icon surfaces this as a Click event
            if is_left_click(&event) {
                if tx.send(TrayCommand::Activate).is_err() {
                    break;
                }
            }
        }

        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// Check if a TrayIconEvent is a click.
fn is_left_click(event: &tray_icon::TrayIconEvent) -> bool {
    matches!(
        event,
        tray_icon::TrayIconEvent::Click { .. } | tray_icon::TrayIconEvent::DoubleClick { .. }
    )
}

/// 32×32 RGBA Gemini 4-pointed star icon with transparent background (no bg)
fn create_icon() -> tray_icon::Icon {
    let img_bytes = include_bytes!("../assets/gemini-icon-32.png");
    let img = ::image::load_from_memory(img_bytes)
        .expect("Failed to load Gemini icon")
        .to_rgba8();
    let (width, height) = img.dimensions();
    tray_icon::Icon::from_rgba(img.into_raw(), width, height)
        .expect("Failed to create tray icon")
}
