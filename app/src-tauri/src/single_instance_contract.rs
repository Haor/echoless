#[test]
fn single_instance_plugin_is_registered_first_and_routes_launch_intent() {
    let source = include_str!("lib.rs");
    let first_plugin = source.find(".plugin(").expect("builder has no plugins");
    let single_instance = source
        .find(".plugin(tauri_plugin_single_instance::init")
        .expect("single-instance plugin is not registered");

    assert_eq!(
        first_plugin, single_instance,
        "single-instance must be the first registered Tauri plugin"
    );
    assert!(
        source.contains("should_focus_existing_instance(&args)"),
        "second-instance callback must distinguish manual and delayed autostart launches"
    );
    assert!(
        source.contains("show_main_window(app)"),
        "manual second-instance launches must restore and focus the main window"
    );
    let autostart = source
        .find("tauri_plugin_autostart::Builder::new")
        .expect("autostart plugin is not registered");
    assert!(
        autostart > single_instance,
        "single-instance must remain ahead of autostart initialization"
    );
    assert!(
        source.contains("AUTOSTART_WATCHDOG_TIMEOUT")
            && source.contains("watchdog_pending()")
            && source.contains("settle_startup_launch"),
        "hidden autostart must have a bounded frontend handshake watchdog"
    );
    let logging_init = source
        .find("logging::init")
        .expect("desktop logging is not initialized");
    assert!(
        logging_init > single_instance,
        "shared log files must not be opened before the single-instance gate"
    );
}
