//! Suprime a janela de console que o Windows abre ao spawnar um processo
//! filho (`adb`/`scrcpy`/`ffmpeg`). Em release o app roda sem console
//! próprio (`windows_subsystem = "windows"`, ver `main.rs`) — sem
//! `CREATE_NO_WINDOW` não há console pai pra herdar e cada spawn abre um
//! prompt novo. Como `device_manager::run_polling_loop` chama `adb` a cada
//! 500 ms, isso piscava sem parar. Em dev o processo já roda anexado a um
//! console existente, por isso o problema só aparecia em release.

#[cfg(windows)]
pub fn hide_console(mut cmd: tokio::process::Command) -> tokio::process::Command {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

#[cfg(not(windows))]
pub fn hide_console(cmd: tokio::process::Command) -> tokio::process::Command {
    cmd
}
