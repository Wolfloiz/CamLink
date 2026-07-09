pub mod camera_controller;
pub mod config;
pub mod device_manager;
pub mod error;
pub mod model;
pub mod raw_manager;
pub mod rtsp_manager;
pub mod secrets;
pub mod stream_manager;
pub mod virtualcam;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
