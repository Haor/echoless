// Echoless desktop GUI entrypoint。发布构建时隐藏 Windows 控制台。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    echoless_app_lib::run()
}
